use content_inspector::{inspect, ContentType};
use fancy_regex::Regex as FancyRegex;
use ignore::{WalkBuilder, WalkParallel};
use log::warn;
use regex::Regex;
use std::path::{Path, PathBuf};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::{fs::File, io::BufReader};

use crate::{
    app::{BackgroundProcessingEvent, SearchResult},
    utils::relative_path_from,
};

pub fn replace_whole_word_if_match(
    haystack: &str,
    search: &str,
    replacement: &str,
) -> Option<String> {
    if haystack.is_empty() || search.is_empty() {
        return None;
    }
    let pattern = format!(
        r"(?<![a-zA-Z0-9_]){}(?![a-zA-Z0-9_])",
        fancy_regex::escape(search)
    );
    let re = FancyRegex::new(&pattern).unwrap();
    match re.is_match(haystack) {
        Ok(true) => Some(re.replace_all(haystack, replacement).to_string()),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub enum SearchType {
    Pattern(Regex),
    PatternAdvanced(FancyRegex),
    Fixed(String),
}

#[derive(Clone, Debug)]
pub struct ParsedFields {
    search_pattern: SearchType,
    replace_string: String,
    whole_word: bool,
    path_pattern: Option<SearchType>,
    // TODO: `root_dir` and `include_hidden` are duplicated across this and App
    root_dir: PathBuf,
    include_hidden: bool,

    background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
}

impl ParsedFields {
    pub fn new(
        search_pattern: SearchType,
        replace_string: String,
        whole_word: bool,
        path_pattern: Option<SearchType>,
        root_dir: PathBuf,
        include_hidden: bool,
        background_processing_sender: UnboundedSender<BackgroundProcessingEvent>,
    ) -> Self {
        Self {
            search_pattern,
            replace_string,
            whole_word,
            path_pattern,
            root_dir,
            include_hidden,
            background_processing_sender,
        }
    }

    pub async fn handle_path(&self, path: &Path) {
        if let Some(ref p) = self.path_pattern {
            let relative_path = relative_path_from(&self.root_dir, path);
            let relative_path = relative_path.as_str();

            let matches_pattern = match p {
                SearchType::Pattern(ref p) => p.is_match(relative_path),
                SearchType::PatternAdvanced(ref p) => p.is_match(relative_path).unwrap(),
                SearchType::Fixed(ref s) => relative_path.contains(s),
            };
            if !matches_pattern {
                return;
            }
        }

        match File::open(path).await {
            Ok(file) => {
                let reader = BufReader::new(file);

                let mut lines = reader.lines();
                let mut line_number = 0;
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => {
                            if let ContentType::BINARY = inspect(line.as_bytes()) {
                                continue;
                            }
                            if let Some(result) = self.replacement_if_match(
                                path.to_path_buf(),
                                line.clone(),
                                line_number,
                            ) {
                                let send_result = self
                                    .background_processing_sender
                                    .send(BackgroundProcessingEvent::AddSearchResult(result));
                                if send_result.is_err() {
                                    // likely state reset, thread about to be killed
                                    return;
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(err) => {
                            warn!("Error retrieving line {} of {:?}: {err}", line_number, path);
                        }
                    }
                    line_number += 1;
                }
            }
            Err(err) => {
                warn!("Error opening file {:?}: {err}", path);
            }
        }
    }

    fn replacement_if_match(
        &self,
        path: PathBuf,
        line: String,
        line_number: usize,
    ) -> Option<SearchResult> {
        #[allow(clippy::collapsible_else_if)]
        let maybe_replacement = match self.search_pattern {
            SearchType::Fixed(ref s) => {
                if self.whole_word {
                    replace_whole_word_if_match(&line, s, &self.replace_string)
                } else {
                    if line.contains(s) {
                        Some(line.replace(s, &self.replace_string))
                    } else {
                        None
                    }
                }
            }
            SearchType::Pattern(ref p) => {
                if p.is_match(&line) {
                    Some(p.replace_all(&line, &self.replace_string).to_string())
                } else {
                    None
                }
            }
            SearchType::PatternAdvanced(ref p) => match p.is_match(&line) {
                Ok(true) => Some(p.replace_all(&line, &self.replace_string).to_string()),
                _ => None,
            },
        };

        maybe_replacement.map(|replacement| SearchResult {
            path,
            line_number: line_number + 1,
            line: line.clone(),
            replacement,
            included: true,
            replace_result: None,
        })
    }

    pub(crate) fn build_walker(&self) -> WalkParallel {
        WalkBuilder::new(&self.root_dir)
            .hidden(!self.include_hidden)
            .filter_entry(|entry| entry.file_name() != ".git")
            .build_parallel()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_replacement() {
        assert_eq!(
            replace_whole_word_if_match("hello world", "world", "earth"),
            Some("hello earth".to_string())
        );
    }

    #[test]
    fn test_multiple_replacements() {
        assert_eq!(
            replace_whole_word_if_match("world hello world", "world", "earth"),
            Some("earth hello earth".to_string())
        );
    }

    #[test]
    fn test_no_match() {
        assert_eq!(
            replace_whole_word_if_match("worldwide", "world", "earth"),
            None
        );
        assert_eq!(
            replace_whole_word_if_match("_world_", "world", "earth"),
            None
        );
    }

    #[test]
    fn test_word_boundaries() {
        assert_eq!(
            replace_whole_word_if_match(",world-", "world", "earth"),
            Some(",earth-".to_string())
        );
        assert_eq!(
            replace_whole_word_if_match("world-word", "world", "earth"),
            Some("earth-word".to_string())
        );
        assert_eq!(
            replace_whole_word_if_match("Hello-world!", "world", "earth"),
            Some("Hello-earth!".to_string())
        );
    }

    #[test]
    fn test_case_sensitive() {
        assert_eq!(
            replace_whole_word_if_match("Hello WORLD", "world", "earth"),
            None
        );
        assert_eq!(
            replace_whole_word_if_match("Hello world", "wOrld", "earth"),
            None
        );
    }

    #[test]
    fn test_empty_strings() {
        assert_eq!(replace_whole_word_if_match("", "world", "earth"), None);
        assert_eq!(
            replace_whole_word_if_match("hello world", "", "earth"),
            None
        );
    }

    #[test]
    fn test_substring_no_match() {
        assert_eq!(
            replace_whole_word_if_match("worldwide web", "world", "earth"),
            None
        );
        assert_eq!(
            replace_whole_word_if_match("underworld", "world", "earth"),
            None
        );
    }

    #[test]
    fn test_special_regex_chars() {
        assert_eq!(
            replace_whole_word_if_match("hello (world)", "(world)", "earth"),
            Some("hello earth".to_string())
        );
        assert_eq!(
            replace_whole_word_if_match("hello world.*", "world.*", "ea+rth"),
            Some("hello ea+rth".to_string())
        );
    }
}
