use std::io::Cursor;

use crate::{
    line_reader::BufReadExt,
    replace::replacement_if_match,
    search::{FileSearcher, ParsedDirConfig, ParsedSearchConfig},
    validation::{
        DirConfig, SearchConfig, SimpleErrorHandler, ValidationResult,
        validate_search_configuration,
    },
};

// Perform a find-and-replace recursively in a given directory
pub fn find_and_replace(
    search_config: SearchConfig<'_>,
    dir_config: DirConfig<'_>,
) -> anyhow::Result<String> {
    let (parsed_search_config, parsed_dir_config) = parse_config(search_config, Some(dir_config))?;
    let searcher = FileSearcher::new(
        parsed_search_config,
        parsed_dir_config.expect("Found None dir_config when search_type is Files"),
    );
    let num_files_replaced = searcher.walk_files_and_replace(None);

    Ok(format!(
        "Success: {num_files_replaced} file{prefix} updated\n",
        prefix = if num_files_replaced != 1 { "s" } else { "" },
    ))
}

/// Perform a find-and-replace in a string slice
pub fn find_and_replace_text(
    content: &str,
    search_config: SearchConfig<'_>,
) -> anyhow::Result<String> {
    let (parsed_search_config, _) = parse_config(search_config, None)?;
    let mut result = String::with_capacity(content.len());

    let cursor = Cursor::new(content);

    for line_result in cursor.lines_with_endings() {
        let (line_bytes, line_ending) = line_result?;

        let line = String::from_utf8(line_bytes)?;

        if let Some(replaced_line) = replacement_if_match(
            &line,
            &parsed_search_config.search,
            &parsed_search_config.replace,
        ) {
            result.push_str(&replaced_line);
        } else {
            result.push_str(&line);
        }

        result.push_str(line_ending.as_str());
    }

    Ok(result)
}

fn parse_config(
    search_config: SearchConfig<'_>,
    dir_config: Option<DirConfig<'_>>,
) -> anyhow::Result<(ParsedSearchConfig, Option<ParsedDirConfig>)> {
    let mut error_handler = SimpleErrorHandler::new();

    match validate_search_configuration(search_config, dir_config, &mut error_handler)? {
        ValidationResult::Success(parsed) => Ok(parsed),
        ValidationResult::ValidationErrors => Err(anyhow::anyhow!(
            "{}",
            error_handler
                .errors_str()
                .unwrap_or_else(|| "Unknown validation error".to_string())
        )),
    }
}
