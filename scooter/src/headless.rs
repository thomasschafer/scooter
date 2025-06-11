use anyhow::bail;
use frep_core::search::SearchResult;
use frep_core::validation::{
    validate_search_configuration, SearchConfiguration, SimpleErrorHandler, ValidationResult,
};
use ignore::WalkState;
use std::sync::mpsc;

use crate::replace::{calculate_statistics, format_replacement_results};

pub fn run_headless(search_config: SearchConfiguration) -> anyhow::Result<String> {
    let mut error_handler = SimpleErrorHandler::new();
    let result = validate_search_configuration(search_config, &mut error_handler)?;
    let searcher = match result {
        ValidationResult::Success(searcher) => searcher,
        ValidationResult::ValidationErrors => {
            bail!("{}", error_handler.errors_str().unwrap());
        }
    };

    let (results_sender, results_receiver) = mpsc::channel::<Vec<SearchResult>>();

    let sender_clone = results_sender.clone();
    searcher.walk_files(None, move || {
        let sender = sender_clone.clone();
        Box::new(move |mut results| {
            if let Err(file_err) = frep_core::replace::replace_in_file(&mut results) {
                log::error!("Found error when performing replacement: {file_err}");
            }
            if sender.send(results).is_err() {
                // Channel closed, likely due to early termination
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        })
    });

    drop(results_sender);

    let all_results = results_receiver.into_iter().flatten();
    let stats = calculate_statistics(all_results);

    for error in &stats.errors {
        let (path, error) = error.display_error();
        log::error!("Error when replacing {path}: {error}");
    }
    let results_output = format_replacement_results(stats.num_successes, None, None);

    Ok(results_output)
}
