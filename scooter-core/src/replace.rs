use futures::future;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::{mpsc, Semaphore};

use frep_core::replace::{replace_in_file, ReplaceResult};
use frep_core::search::SearchResult;

pub fn split_results(results: Vec<SearchResult>) -> (Vec<SearchResult>, usize) {
    let (included, excluded): (Vec<_>, Vec<_>) = results.into_iter().partition(|res| res.included);
    let num_ignored = excluded.len();
    (included, num_ignored)
}

fn group_results(included: Vec<SearchResult>) -> HashMap<PathBuf, Vec<SearchResult>> {
    let mut path_groups = HashMap::<PathBuf, Vec<SearchResult>>::new();
    for res in included {
        path_groups.entry(res.path.clone()).or_default().push(res);
    }
    path_groups
}

pub fn spawn_replace_included(
    search_results: Vec<SearchResult>,
    cancelled: Arc<AtomicBool>,
    replacements_completed: Arc<AtomicUsize>,
) -> (mpsc::UnboundedReceiver<SearchResult>, usize) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (included, num_ignored) = split_results(search_results);

    tokio::spawn(async move {
        let path_groups = group_results(included);
        let semaphore = Arc::new(Semaphore::new(8));
        let mut file_tasks = vec![];

        for (_path, mut results) in path_groups {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }

            let semaphore = semaphore.clone();
            let replacements_completed_clone = replacements_completed.clone();
            let tx = tx.clone();

            let task = tokio::spawn(async move {
                let permit = semaphore.acquire_owned().await.unwrap();
                if let Err(file_err) = replace_in_file(&mut results) {
                    for res in &mut results {
                        res.replace_result = Some(ReplaceResult::Error(file_err.to_string()));
                    }
                }
                replacements_completed_clone.fetch_add(results.len(), Ordering::Relaxed);

                for result in results {
                    tx.send(result).unwrap();
                }

                drop(permit);
            });
            file_tasks.push(task);
        }

        future::join_all(file_tasks).await;
    });

    (rx, num_ignored)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use frep_core::{line_reader::LineEnding, replace::ReplaceResult, search::SearchResult};

    use crate::replace;

    fn create_search_result(
        path: &str,
        line_number: usize,
        line: &str,
        replacement: &str,
        included: bool,
        replace_result: Option<ReplaceResult>,
    ) -> SearchResult {
        SearchResult {
            path: PathBuf::from(path),
            line_number,
            line: line.to_string(),
            line_ending: LineEnding::Lf,
            replacement: replacement.to_string(),
            included,
            replace_result,
        }
    }

    #[test]
    fn test_split_results_all_included() {
        let result1 = create_search_result("file1.txt", 1, "line1", "repl1", true, None);
        let result2 = create_search_result("file2.txt", 2, "line2", "repl2", true, None);
        let result3 = create_search_result("file3.txt", 3, "line3", "repl3", true, None);

        let search_results = vec![result1.clone(), result2.clone(), result3.clone()];

        let (included, num_ignored) = replace::split_results(search_results);
        assert_eq!(num_ignored, 0);
        assert_eq!(included, vec![result1, result2, result3]);
    }

    #[test]
    fn test_split_results_mixed() {
        let result1 = create_search_result("file1.txt", 1, "line1", "repl1", true, None);
        let result2 = create_search_result("file2.txt", 2, "line2", "repl2", false, None);
        let result3 = create_search_result("file3.txt", 3, "line3", "repl3", true, None);
        let result4 = create_search_result("file4.txt", 4, "line4", "repl4", false, None);

        let search_results = vec![result1.clone(), result2, result3.clone(), result4];

        let (included, num_ignored) = replace::split_results(search_results);
        assert_eq!(num_ignored, 2);
        assert_eq!(included, vec![result1, result3]);
        assert!(included.iter().all(|r| r.included));
    }
}
