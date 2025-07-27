use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
};

use frep_core::{
    replace::{replace_in_file, replacement_if_match, ReplaceResult},
    search::{FileSearcherConfig, SearchResultWithReplacement},
};
use rayon::prelude::{IntoParallelIterator, ParallelIterator};

pub fn split_results(
    results: Vec<SearchResultWithReplacement>,
) -> (Vec<SearchResultWithReplacement>, usize) {
    let (included, excluded): (Vec<_>, Vec<_>) = results
        .into_iter()
        .partition(|res| res.search_result.included);
    let num_ignored = excluded.len();
    (included, num_ignored)
}

fn group_results(
    included: Vec<SearchResultWithReplacement>,
) -> HashMap<PathBuf, Vec<SearchResultWithReplacement>> {
    let mut path_groups = HashMap::<PathBuf, Vec<SearchResultWithReplacement>>::new();
    for res in included {
        path_groups
            .entry(res.search_result.path.clone())
            .or_default()
            .push(res);
    }
    path_groups
}

pub fn spawn_replace_included<T: Fn(SearchResultWithReplacement) + Send + Sync + 'static>(
    search_results: Vec<SearchResultWithReplacement>,
    cancelled: Arc<AtomicBool>,
    replacements_completed: Arc<AtomicUsize>,
    validation_search_config: Option<FileSearcherConfig>,
    on_completion: T,
) -> usize {
    let (included, num_ignored) = split_results(search_results);

    thread::spawn(move || {
        let path_groups = group_results(included);

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .unwrap();

        pool.install(|| {
            path_groups
                .into_par_iter()
                .for_each(|(_path, mut results)| {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }

                    if let Some(config) = &validation_search_config {
                        validate_search_result_correctness(config, &results);
                    }
                    if let Err(file_err) = replace_in_file(&mut results) {
                        for res in &mut results {
                            res.replace_result = Some(ReplaceResult::Error(file_err.to_string()));
                        }
                    }
                    replacements_completed.fetch_add(results.len(), Ordering::Relaxed);

                    for result in results {
                        on_completion(result);
                    }
                });
        });
    });

    num_ignored
}

fn validate_search_result_correctness(
    validation_search_config: &FileSearcherConfig,
    results: &Vec<SearchResultWithReplacement>,
) {
    for res in results {
        let expected = replacement_if_match(
            &res.search_result.line,
            &validation_search_config.search,
            &validation_search_config.replace,
        );
        let actual = &res.replacement;
        assert_eq!(
            expected.as_ref(),
            Some(actual),
            "Expected replacement does not match actual"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use frep_core::{
        line_reader::LineEnding,
        replace::ReplaceResult,
        search::{SearchResult, SearchResultWithReplacement},
    };

    use crate::replace;

    fn create_search_result_with_replacement(
        path: &str,
        line_number: usize,
        line: &str,
        replacement: &str,
        included: bool,
        replace_result: Option<ReplaceResult>,
    ) -> SearchResultWithReplacement {
        SearchResultWithReplacement {
            search_result: SearchResult {
                path: PathBuf::from(path),
                line_number,
                line: line.to_string(),
                line_ending: LineEnding::Lf,
                included,
            },
            replacement: replacement.to_string(),
            replace_result,
        }
    }

    #[test]
    fn test_split_results_all_included() {
        let result1 =
            create_search_result_with_replacement("file1.txt", 1, "line1", "repl1", true, None);
        let result2 =
            create_search_result_with_replacement("file2.txt", 2, "line2", "repl2", true, None);
        let result3 =
            create_search_result_with_replacement("file3.txt", 3, "line3", "repl3", true, None);

        let search_results = vec![result1.clone(), result2.clone(), result3.clone()];

        let (included, num_ignored) = replace::split_results(search_results);
        assert_eq!(num_ignored, 0);
        assert_eq!(included, vec![result1, result2, result3]);
    }

    #[test]
    fn test_split_results_mixed() {
        let result1 =
            create_search_result_with_replacement("file1.txt", 1, "line1", "repl1", true, None);
        let result2 =
            create_search_result_with_replacement("file2.txt", 2, "line2", "repl2", false, None);
        let result3 =
            create_search_result_with_replacement("file3.txt", 3, "line3", "repl3", true, None);
        let result4 =
            create_search_result_with_replacement("file4.txt", 4, "line4", "repl4", false, None);

        let search_results = vec![result1.clone(), result2, result3.clone(), result4];

        let (included, num_ignored) = replace::split_results(search_results);
        assert_eq!(num_ignored, 2);
        assert_eq!(included, vec![result1, result3]);
        assert!(included.iter().all(|r| r.search_result.included));
    }
}
