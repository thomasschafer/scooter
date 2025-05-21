use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use insta::assert_debug_snapshot;
use serial_test::serial;
use std::cmp::max;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

use scooter::{
    test_with_both_regex_modes, App, AppError, EventHandlingResult, FieldValue,
    PerformingReplacementState, Popup, ReplaceResult, ReplaceState, Screen, SearchCompleteState,
    SearchFieldValues, SearchFields, SearchInProgressState, SearchResult, SearchState,
};

mod utils;

#[tokio::test]
async fn test_replace_state() {
    let mut state = ReplaceState {
        num_successes: 2,
        num_ignored: 1,
        errors: (1..3)
            .map(|n| SearchResult {
                path: PathBuf::from(format!("error-{n}.txt")),
                line_number: 1,
                line: format!("line {n}"),
                replacement: format!("error replacement {n}"),
                included: true,
                replace_result: Some(ReplaceResult::Error(format!("Test error {n}"))),
            })
            .collect::<Vec<_>>(),
        replacement_errors_pos: 0,
    };

    state.scroll_replacement_errors_down();
    assert_eq!(state.replacement_errors_pos, 1);
    state.scroll_replacement_errors_down();
    assert_eq!(state.replacement_errors_pos, 0);
    state.scroll_replacement_errors_up();
    assert_eq!(state.replacement_errors_pos, 1);
    state.scroll_replacement_errors_up();
    assert_eq!(state.replacement_errors_pos, 0);
}

#[tokio::test]
async fn test_app_reset() {
    let (mut app, _app_event_receiver) =
        App::new_with_receiver(None, false, false, &SearchFieldValues::default());
    app.current_screen = Screen::Results(ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![],
        replacement_errors_pos: 0,
    });

    app.reset();

    assert!(matches!(app.current_screen, Screen::SearchFields));
}

#[tokio::test]
async fn test_back_from_results() {
    let (mut app, _app_event_receiver) =
        App::new_with_receiver(None, false, false, &SearchFieldValues::default());
    let (_sender, receiver) = mpsc::unbounded_channel();
    app.current_screen = Screen::SearchComplete(SearchCompleteState::new(
        SearchState::new(receiver),
        Instant::now(),
    ));
    app.search_fields = SearchFields::with_values(SearchFieldValues {
        search: FieldValue::new("foo", false),
        replace: FieldValue::new("bar", false),
        fixed_strings: FieldValue::new(true, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("pattern", false),
        exclude_files: FieldValue::new("", false),
    });

    let res = app.handle_key_event(&KeyEvent {
        code: KeyCode::Char('o'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert!(res != EventHandlingResult::Exit);
    assert_eq!(app.search_fields.search().text, "foo");
    assert_eq!(app.search_fields.replace().text, "bar");
    assert!(app.search_fields.fixed_strings().checked);
    assert_eq!(app.search_fields.include_files().text, "pattern");
    assert_eq!(app.search_fields.exclude_files().text, "");
    assert!(matches!(app.current_screen, Screen::SearchFields));
}

fn test_error_popup_invalid_input_impl(search_fields: SearchFieldValues<'_>) {
    let (mut app, _app_event_receiver) =
        App::new_with_receiver(None, false, false, &SearchFieldValues::default());
    app.current_screen = Screen::SearchFields;
    app.search_fields = SearchFields::with_values(search_fields);

    let res = app.perform_search_if_valid();
    assert!(res != EventHandlingResult::Exit);
    assert!(matches!(app.current_screen, Screen::SearchFields));
    assert!(matches!(app.popup(), Some(Popup::Error)));

    let res = app.handle_key_event(&KeyEvent {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert!(res != EventHandlingResult::Exit);
    assert!(!matches!(app.popup(), Some(Popup::Error)));

    let res = app.handle_key_event(&KeyEvent {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert_eq!(res, EventHandlingResult::Exit);
}

#[tokio::test]
async fn test_error_popup_invalid_search() {
    test_error_popup_invalid_input_impl(SearchFieldValues {
        search: FieldValue::new("search invalid regex(", false),
        replace: FieldValue::new("replacement", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("", false),
        exclude_files: FieldValue::new("", false),
    });
}

#[tokio::test]
async fn test_error_popup_invalid_include_files() {
    test_error_popup_invalid_input_impl(SearchFieldValues {
        search: FieldValue::new("search", false),
        replace: FieldValue::new("replacement", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("foo{", false),
        exclude_files: FieldValue::new("", false),
    });
}

#[tokio::test]
async fn test_error_popup_invalid_exclude_files() {
    test_error_popup_invalid_input_impl(SearchFieldValues {
        search: FieldValue::new("search", false),
        replace: FieldValue::new("replacement", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("", false),
        exclude_files: FieldValue::new("bar{", false),
    });
}

pub fn wait_until<F>(condition: F, timeout: Duration) -> bool
where
    F: Fn() -> bool,
{
    let start = Instant::now();
    let sleep_duration = max(timeout / 50, Duration::from_millis(1));
    while !condition() && start.elapsed() <= timeout {
        sleep(sleep_duration);
    }
    condition()
}

async fn process_bp_events(app: &mut App) {
    let timeout = Duration::from_secs(5);
    let start = Instant::now();

    while let Some(event) = app.background_processing_recv().await {
        app.handle_background_processing_event(event);
        #[allow(clippy::manual_assert)]
        if start.elapsed() > timeout {
            panic!("Couldn't process background events in a reasonable time");
        }
    }
}

macro_rules! wait_for_screen {
    ($app:expr, $variant:path) => {
        wait_until(
            || matches!($app.current_screen, $variant(_)),
            Duration::from_secs(1),
        )
    };
}

fn setup_app(temp_dir: &TempDir, search_fields: SearchFields, include_hidden: bool) -> App {
    let (mut app, _app_event_receiver) = App::new_with_receiver(
        Some(temp_dir.path().to_path_buf()),
        include_hidden,
        false,
        &SearchFieldValues::default(),
    );
    app.search_fields = search_fields;
    app
}

// TODO: simplify this test - it is somewhat tied to the current implementation
async fn search_and_replace_test(
    temp_dir: &TempDir,
    search_fields: SearchFields,
    include_hidden: bool,
    expected_matches: Vec<(&Path, usize)>,
) {
    let total_num_expected_matches = expected_matches
        .iter()
        .map(|(_, count)| count)
        .sum::<usize>();

    let mut app = setup_app(temp_dir, search_fields, include_hidden);
    let res = app.perform_search_if_valid();
    assert!(res != EventHandlingResult::Exit);

    process_bp_events(&mut app).await;
    assert!(wait_for_screen!(&app, Screen::SearchComplete));

    if let Screen::SearchComplete(state) = &mut app.current_screen {
        for (file_path, num_expected_matches) in &expected_matches {
            let num_actual_matches = state
                .search_state
                .results
                .iter()
                .filter(|result| {
                    let result_path = result.path.to_str().unwrap();
                    let file_path = file_path.to_str().unwrap();
                    result_path == temp_dir.path().join(file_path).to_string_lossy()
                })
                .count();
            let num_expected_matches = *num_expected_matches;
            assert_eq!(
                num_actual_matches, num_expected_matches,
                "{file_path:?}: expected {num_expected_matches}, found {num_actual_matches}",
            );
        }

        assert_eq!(state.search_state.results.len(), total_num_expected_matches);
    } else {
        panic!(
            "Expected SearchComplete results, found {:?}",
            app.current_screen
        );
    }

    app.trigger_replacement();

    process_bp_events(&mut app).await;
    assert!(wait_for_screen!(&app, Screen::Results));

    if let Screen::Results(search_state) = &app.current_screen {
        assert_eq!(search_state.num_successes, total_num_expected_matches);
        assert_eq!(search_state.num_ignored, 0);
        assert_eq!(search_state.errors.len(), 0);
    } else {
        panic!(
            "Expected screen to be Screen::Results, instead found {:?}",
            app.current_screen
        );
    }
}

#[tokio::test]
#[serial]
async fn test_search_replace_defaults() {
    let temp_dir = create_test_files!(
        "file1.txt" => {
            "This is a test file",
            "It contains some test content",
            "For TESTING purposes",
            "Test TEST tEsT tesT test",
            "TestbTESTctEsTdtesTetest",
            " test ",
        },
        "file2.txt" => {
            "Another test file",
            "With different content",
            "Also for testing",
        },
        "file3.txt" => {
            "something",
            "123 bar[a-b]+.*bar)(baz 456",
            "test-TEST-tESt",
            "something",
        }
    );

    let search_fields = SearchFields::with_values(SearchFieldValues {
        search: FieldValue::new("t[esES]+t", false),
        replace: FieldValue::new("123,", false),
        ..SearchFieldValues::default()
    });
    search_and_replace_test(
        &temp_dir,
        search_fields,
        false,
        vec![
            (Path::new("file1.txt"), 5),
            (Path::new("file2.txt"), 2),
            (Path::new("file3.txt"), 1),
        ],
    )
    .await;

    assert_test_files!(
        &temp_dir,
        "file1.txt" => {
            "This is a 123, file",
            "It contains some 123, content",
            "For TESTING purposes",
            "Test TEST tEsT tesT 123,",
            "TestbTESTctEsTdtesTe123,",
            " 123, ",
        },
        "file2.txt" => {
            "Another 123, file",
            "With different content",
            "Also for 123,ing",
        },
        "file3.txt" => {
            "something",
            "123 bar[a-b]+.*bar)(baz 456",
            "123,-TEST-123,",
            "something",
        }
    );
}

test_with_both_regex_modes!(
    test_search_replace_fixed_string,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "file2.txt" => {
                "Another test file",
                "With different content",
                "Also for testing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new(".*", false),
            replace: FieldValue::new("example", false),
            fixed_strings: FieldValue::new(true, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            &temp_dir,
            search_fields,
            false,
            vec![
                (Path::new("file1.txt"), 0),
                (Path::new("file2.txt"), 0),
                (Path::new("file3.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "file2.txt" => {
                "Another test file",
                "With different content",
                "Also for testing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+examplebar)(baz 456",
                "something",
            }
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_search_replace_match_case,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For TESTING purposes",
                "Test TEST tEsT tesT test",
                "TestbTESTctEsTdtesTetest",
                " test ",
            },
            "file2.txt" => {
                "Another test file",
                "With different content",
                "Also for testing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "test-TEST-tESt",
                "something",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new("test", false),
            replace: FieldValue::new("REPLACEMENT", false),
            fixed_strings: FieldValue::new(true, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            &temp_dir,
            search_fields,
            false,
            vec![
                (Path::new("file1.txt"), 5),
                (Path::new("file2.txt"), 2),
                (Path::new("file3.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => {
                "This is a REPLACEMENT file",
                "It contains some REPLACEMENT content",
                "For TESTING purposes",
                "Test TEST tEsT tesT REPLACEMENT",
                "TestbTESTctEsTdtesTeREPLACEMENT",
                " REPLACEMENT ",
            },
            "file2.txt" => {
                "Another REPLACEMENT file",
                "With different content",
                "Also for REPLACEMENTing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "REPLACEMENT-TEST-tESt",
                "something",
            }
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_search_replace_dont_match_case,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For TESTING purposes",
                "Test TEST tEsT tesT test",
                "TestbTESTctEsTdtesTetest",
                " test ",
            },
            "file2.txt" => {
                "Another test file",
                "With different content",
                "Also for testing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "test-TEST-tESt",
                "something",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new("test", false),
            replace: FieldValue::new("REPLACEMENT", false),
            fixed_strings: FieldValue::new(true, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(false, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            &temp_dir,
            search_fields,
            false,
            vec![
                (Path::new("file1.txt"), 6),
                (Path::new("file2.txt"), 2),
                (Path::new("file3.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => {
                "This is a REPLACEMENT file",
                "It contains some REPLACEMENT content",
                "For REPLACEMENTING purposes",
                "REPLACEMENT REPLACEMENT REPLACEMENT REPLACEMENT REPLACEMENT",
                "REPLACEMENTbREPLACEMENTcREPLACEMENTdREPLACEMENTeREPLACEMENT",
                " REPLACEMENT ",
            },
            "file2.txt" => {
                "Another REPLACEMENT file",
                "With different content",
                "Also for REPLACEMENTing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "REPLACEMENT-REPLACEMENT-REPLACEMENT",
                "something",
            }
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_regex,
    |advanced_regex: bool| async move {
        let temp_dir = &create_test_files!(
            "file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "file2.txt" => {
                "Another test file",
                "With different content",
                "Also for testing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new(r"\b\w+ing\b", false),
            replace: FieldValue::new("VERB", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            temp_dir,
            search_fields,
            false,
            vec![
                (Path::new("file1.txt"), 1),
                (Path::new("file2.txt"), 1),
                (Path::new("file3.txt"), 2),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For VERB purposes",
            },
            "file2.txt" => {
                "Another test file",
                "With different content",
                "Also for VERB",
            },
            "file3.txt" => {
                "VERB",
                "123 bar[a-b]+.*bar)(baz 456",
                "VERB",
            }
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_no_matches,
    |advanced_regex: bool| async move {
        let temp_dir = &create_test_files!(
            "file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "file2.txt" => {
                "Another test file",
                "With different content",
                "Also for testing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new("nonexistent-string", false),
            replace: FieldValue::new("replacement", false),
            fixed_strings: FieldValue::new(true, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            temp_dir,
            search_fields,
            false,
            vec![
                (Path::new("file1.txt"), 0),
                (Path::new("file2.txt"), 0),
                (Path::new("file3.txt"), 0),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "file2.txt" => {
                "Another test file",
                "With different content",
                "Also for testing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            }
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_invalid_regex,
    |advanced_regex: bool| async move {
        let temp_dir = &create_test_files!(
            "file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "file2.txt" => {
                "Another test file",
                "With different content",
                "Also for testing",
            },
            "file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new("[invalid regex", false),
            replace: FieldValue::new("replacement", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);
        let mut app = setup_app(temp_dir, search_fields, false);

        let res = app.perform_search_if_valid();
        assert!(res != EventHandlingResult::Exit);
        assert!(matches!(app.current_screen, Screen::SearchFields));
        process_bp_events(&mut app).await;
        assert!(!wait_for_screen!(&app, Screen::SearchComplete)); // We shouldn't get to the SearchComplete page, so assert that we never get there
        assert!(matches!(app.current_screen, Screen::SearchFields));
        Ok(())
    }
);

#[tokio::test]
#[serial]
async fn test_advanced_regex_negative_lookahead() {
    let temp_dir = &create_test_files!(
        "file1.txt" => {
            "This is a test file",
            "It contains some test content",
            "For testing purposes",
        },
        "file2.txt" => {
            "Another test file",
            "With different content",
            "Also for testing",
        },
        "file3.txt" => {
            "something",
            "123 bar[a-b]+.*bar)(baz 456",
            "something",
        }
    );

    let search_fields = SearchFields::with_values(SearchFieldValues {
        search: FieldValue::new("(test)(?!ing)", false),
        replace: FieldValue::new("BAR", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("", false),
        exclude_files: FieldValue::new("", false),
    })
    .with_advanced_regex(true);
    search_and_replace_test(
        temp_dir,
        search_fields,
        false,
        vec![
            (Path::new("file1.txt"), 2),
            (Path::new("file2.txt"), 1),
            (Path::new("file3.txt"), 0),
        ],
    )
    .await;

    assert_test_files!(
        temp_dir,
        "file1.txt" => {
            "This is a BAR file",
            "It contains some BAR content",
            "For testing purposes",
        },
        "file2.txt" => {
            "Another BAR file",
            "With different content",
            "Also for testing",
        },
        "file3.txt" => {
            "something",
            "123 bar[a-b]+.*bar)(baz 456",
            "something",
        }
    );
}

test_with_both_regex_modes!(
    test_update_search_results_include_dir,
    |advanced_regex: bool| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir2/file2.txt" => {
                "Another test file",
                "With different content",
                "Also for testing",
            },
            "dir2/file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something testing",
            },
            "dir3/file4.txt" => {
                "some testing text from dir3/file4.txt, blah",
            },
            "dir3/subdir1/file5.txt" => {
                "some testing text from dir3/subdir1/file5.txt, blah",
            },
            "dir4/subdir2/file6.txt" => {
                "some testing text from dir4/subdir2/file6.txt, blah",
            },
            "dir4/subdir3/file7.txt" => {
                "some testing text from dir4/subdir3/file7.txt, blah",
            },
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new("testing", false),
            replace: FieldValue::new("f", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("dir2/*, dir3/**, */subdir3/*", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            temp_dir,
            search_fields,
            false,
            vec![
                (&Path::new("dir1").join("file1.txt"), 0),
                (&Path::new("dir2").join("file2.txt"), 1),
                (&Path::new("dir2").join("file3.txt"), 1),
                (&Path::new("dir3").join("file4.txt"), 1),
                (&Path::new("dir3").join("subdir1").join("file5.txt"), 1),
                (&Path::new("dir4").join("subdir2").join("file6.txt"), 0),
                (&Path::new("dir4").join("subdir3").join("file7.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir2/file2.txt" => {
                "Another test file",
                "With different content",
                "Also for f",
            },
            "dir2/file3.txt" => {
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something f",
            },
            "dir3/file4.txt" => {
                "some f text from dir3/file4.txt, blah",
            },
            "dir3/subdir1/file5.txt" => {
                "some f text from dir3/subdir1/file5.txt, blah",
            },
            "dir4/subdir2/file6.txt" => {
                "some testing text from dir4/subdir2/file6.txt, blah",
            },
            "dir4/subdir3/file7.txt" => {
                "some f text from dir4/subdir3/file7.txt, blah",
            },
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_exclude_dir,
    |advanced_regex: bool| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir1/file1.rs" => {
                "func testing() {",
                r#"  "testing""#,
                "}",
            },
            "dir2/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir2/file2.rs" => {
                "func main2() {",
                r#"  "testing""#,
                "}",
            },
            "dir2/file3.rs" => {
                "func main3() {",
                r#"  "testing""#,
                "}",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new("testing", false),
            replace: FieldValue::new("REPL", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("dir1", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            temp_dir,
            search_fields,
            false,
            vec![
                (&Path::new("dir1").join("file1.txt"), 0),
                (&Path::new("dir1").join("file1.rs"), 0),
                (&Path::new("dir2").join("file1.txt"), 1),
                (&Path::new("dir2").join("file2.rs"), 1),
                (&Path::new("dir2").join("file3.rs"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir1/file1.rs" => {
                "func testing() {",
                r#"  "testing""#,
                "}",
            },
            "dir2/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For REPL purposes",
            },
            "dir2/file2.rs" => {
                "func main2() {",
                r#"  "REPL""#,
                "}",
            },
            "dir2/file3.rs" => {
                "func main3() {",
                r#"  "REPL""#,
                "}",
            }
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_multiple_includes_and_excludes,
    |advanced_regex: bool| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir1/file1.rs" => {
                "func testing1() {",
                r#"  "testing1""#,
                "}",
            },
            "dir1/file2.rs" => {
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            },
            "dir2/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir2/file2.rs" => {
                "func main2() {",
                r#"  "testing""#,
                "}",
            },
            "dir2/file3.rs" => {
                "func main3() {",
                r#"  "testing""#,
                "}",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new("testing", false),
            replace: FieldValue::new("REPL", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("dir1/*, *.rs", false),
            exclude_files: FieldValue::new("**/file2.rs", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            temp_dir,
            search_fields,
            false,
            vec![
                (&Path::new("dir1").join("file1.txt"), 1),
                (&Path::new("dir1").join("file1.rs"), 2),
                (&Path::new("dir1").join("file2.rs"), 0),
                (&Path::new("dir2").join("file1.txt"), 0),
                (&Path::new("dir2").join("file2.rs"), 0),
                (&Path::new("dir2").join("file3.rs"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For REPL purposes",
            },
            "dir1/file1.rs" => {
                "func REPL1() {",
                r#"  "REPL1""#,
                "}",
            },
            "dir1/file2.rs" => {
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            },
            "dir2/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir2/file2.rs" => {
                "func main2() {",
                r#"  "testing""#,
                "}",
            },
            "dir2/file3.rs" => {
                "func main3() {",
                r#"  "REPL""#,
                "}",
            }
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_update_search_results_multiple_includes_and_excludes_additional_spacing,
    |advanced_regex: bool| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir1/file1.rs" => {
                "func testing1() {",
                r#"  "testing1""#,
                "}",
            },
            "dir1/file2.rs" => {
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            },
            "dir1/subdir1/subdir2/file3.rs" => {
                "func testing3() {",
                r#"  "testing3""#,
                "}",
            },
            "dir2/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir2/file2.rs" => {
                "func main2() {",
                r#"  "testing""#,
                "}",
            },
            "dir2/file3.rs" => {
                "func main3() {",
                r#"  "testing""#,
                "}",
            },
            "dir2/file4.py" => {
                "def main():",
                "  return 'testing'",
            },
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new("testing", false),
            replace: FieldValue::new("REPL", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new(" dir1/*,*.rs   ,  *.py", false),
            exclude_files: FieldValue::new("  **/file2.rs ", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            temp_dir,
            search_fields,
            false,
            vec![
                (&Path::new("dir1").join("file1.txt"), 1),
                (&Path::new("dir1").join("file1.rs"), 2),
                (&Path::new("dir1").join("file2.rs"), 0),
                (
                    &Path::new("dir1")
                        .join("subdir1")
                        .join("subdir2")
                        .join("file3.rs"),
                    2,
                ),
                (&Path::new("dir2").join("file1.txt"), 0),
                (&Path::new("dir2").join("file2.rs"), 0),
                (&Path::new("dir2").join("file3.rs"), 1),
                (&Path::new("dir2").join("file4.py"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For REPL purposes",
            },
            "dir1/file1.rs" => {
                "func REPL1() {",
                r#"  "REPL1""#,
                "}",
            },
            "dir1/file2.rs" => {
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            },
            "dir1/subdir1/subdir2/file3.rs" => {
                "func REPL3() {",
                r#"  "REPL3""#,
                "}",
            },
            "dir2/file1.txt" => {
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            },
            "dir2/file2.rs" => {
                "func main2() {",
                r#"  "testing""#,
                "}",
            },
            "dir2/file3.rs" => {
                "func main3() {",
                r#"  "REPL""#,
                "}",
            },
            "dir2/file4.py" => {
                "def main():",
                "  return 'REPL'",
            },
        );
        Ok(())
    }
);

test_with_both_regex_modes!(test_ignores_gif_file, |advanced_regex: bool| async move {
    let temp_dir = &create_test_files!(
        "dir1/file1.txt" => {
            "This is a text file",
        },
        "dir2/file2.gif" => {
            "This is a gif file",
        },
        "file3.txt" => {
            "This is a text file",
        }
    );

    let search_fields = SearchFields::with_values(SearchFieldValues {
        search: FieldValue::new("is", false),
        replace: FieldValue::new("", false),
        fixed_strings: FieldValue::new(false, false),
        match_whole_word: FieldValue::new(false, false),
        match_case: FieldValue::new(true, false),
        include_files: FieldValue::new("", false),
        exclude_files: FieldValue::new("", false),
    })
    .with_advanced_regex(advanced_regex);
    search_and_replace_test(
        temp_dir,
        search_fields,
        false,
        vec![
            (&Path::new("dir1").join("file1.txt"), 1),
            (&Path::new("dir2").join("file2.gif"), 0),
            (Path::new("file3.txt"), 1),
        ],
    )
    .await;

    assert_test_files!(
        temp_dir,
        "dir1/file1.txt" => {
            "Th  a text file",
        },
        "dir2/file2.gif" => {
            "This is a gif file",
        },
        "file3.txt" => {
            "Th  a text file",
        }
    );
    Ok(())
});

test_with_both_regex_modes!(
    test_ignores_hidden_files_by_default,
    |advanced_regex: bool| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => {
                "This is a text file",
            },
            ".dir2/file2.rs" => {
                "This is a file in a hidden directory",
            },
            ".file3.txt" => {
                "This is a hidden text file",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new(r"\bis\b", false),
            replace: FieldValue::new("REPLACED", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            temp_dir,
            search_fields,
            false,
            vec![
                (&Path::new("dir1").join("file1.txt"), 1),
                (&Path::new(".dir2").join("file2.rs"), 0),
                (Path::new(".file3.txt"), 0),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => {
                "This REPLACED a text file",
            },
            ".dir2/file2.rs" => {
                "This is a file in a hidden directory",
            },
            ".file3.txt" => {
                "This is a hidden text file",
            }
        );
        Ok(())
    }
);

test_with_both_regex_modes!(
    test_includes_hidden_files_with_flag,
    |advanced_regex: bool| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => {
                "This is a text file",
            },
            ".dir2/file2.rs" => {
                "This is a file in a hidden directory",
            },
            ".file3.txt" => {
                "This is a hidden text file",
            }
        );

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new(r"\bis\b", false),
            replace: FieldValue::new("REPLACED", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);
        search_and_replace_test(
            temp_dir,
            search_fields,
            true,
            vec![
                (&Path::new("dir1").join("file1.txt"), 1),
                (&Path::new(".dir2").join("file2.rs"), 1),
                (Path::new(".file3.txt"), 1),
            ],
        )
        .await;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => {
                "This REPLACED a text file",
            },
            ".dir2/file2.rs" => {
                "This REPLACED a file in a hidden directory",
            },
            ".file3.txt" => {
                "This REPLACED a hidden text file",
            }
        );
        Ok(())
    }
);

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn read_file<P>(p: P) -> String
where
    P: AsRef<Path>,
{
    fs::read_to_string(p).unwrap().replace("\r\n", "\n")
}

test_with_both_regex_modes!(
    test_binary_file_filtering,
    |advanced_regex: bool| async move {
        let temp_dir = TempDir::new().unwrap();
        let fixtures_dir = "tests/fixtures/binary_test";
        copy_dir_all(format!("{fixtures_dir}/initial"), temp_dir.path())?;

        let search_fields = SearchFields::with_values(SearchFieldValues {
            search: FieldValue::new("sample", false),
            replace: FieldValue::new("REPLACED", false),
            fixed_strings: FieldValue::new(false, false),
            match_whole_word: FieldValue::new(false, false),
            match_case: FieldValue::new(true, false),
            include_files: FieldValue::new("", false),
            exclude_files: FieldValue::new("", false),
        })
        .with_advanced_regex(advanced_regex);

        search_and_replace_test(
            &temp_dir,
            search_fields,
            false,
            vec![
                (&Path::new("textfiles").join("code.rs"), 1),
                (&Path::new("textfiles").join("config.json"), 1),
                (&Path::new("textfiles").join("document.txt"), 2),
                (&Path::new("textfiles").join("noextension"), 1),
                (&Path::new("binaries").join("image.wrongext"), 0),
                (&Path::new("binaries").join("document.pdf"), 0),
                (&Path::new("binaries").join("document_pdf_wrong_ext.rs"), 0),
                (&Path::new("binaries").join("archive.zip"), 0),
                (&Path::new("binaries").join("rust_binary"), 0),
            ],
        )
        .await;

        let text_files = vec![
            "textfiles/code.rs",
            "textfiles/config.json",
            "textfiles/document.txt",
            "textfiles/noextension",
        ];

        let binary_files = vec![
            "binaries/image.wrongext",
            "binaries/document.pdf",
            "binaries/document_pdf_wrong_ext.rs",
            "binaries/archive.zip",
            "binaries/rust_binary",
        ];

        for file in &text_files {
            let actual = read_file(temp_dir.path().join(file));
            let expected = read_file(format!("{fixtures_dir}/updated/{file}"));

            assert_eq!(
                actual, expected,
                "Text file {file} was not correctly updated",
            );
        }

        for file in &binary_files {
            let actual = fs::read(temp_dir.path().join(file))?;
            let original = fs::read(format!("{fixtures_dir}/initial/{file}"))?;

            assert_eq!(
                actual, original,
                "Binary file {file} was unexpectedly modified",
            );
        }

        Ok(())
    }
);

#[tokio::test]
async fn test_keymaps_search_fields() {
    let (app, _app_event_receiver) =
        App::new_with_receiver(None, false, false, &SearchFieldValues::default());

    assert!(matches!(app.current_screen, Screen::SearchFields));

    assert_debug_snapshot!("search_fields_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_fields_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_search_complete() {
    let (mut app, _app_event_receiver) =
        App::new_with_receiver(None, false, false, &SearchFieldValues::default());

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.current_screen = Screen::SearchComplete(SearchCompleteState::new(
        SearchState::new(receiver),
        std::time::Instant::now(),
    ));

    assert_debug_snapshot!("search_complete_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_complete_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_search_progressing() {
    let (mut app, _app_event_receiver) =
        App::new_with_receiver(None, false, false, &SearchFieldValues::default());

    let (_sender, receiver) = mpsc::unbounded_channel();
    app.current_screen =
        Screen::SearchProgressing(SearchInProgressState::new(tokio::spawn(async {}), receiver));

    assert_debug_snapshot!("search_progressing_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("search_progressing_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_performing_replacement() {
    let (mut app, _app_event_receiver) =
        App::new_with_receiver(None, false, false, &SearchFieldValues::default());

    let (sender, receiver) = mpsc::unbounded_channel();
    app.current_screen =
        Screen::PerformingReplacement(PerformingReplacementState::new(None, sender, receiver));

    assert_debug_snapshot!(
        "performing_replacement_compact_keymaps",
        app.keymaps_compact()
    );
    assert_debug_snapshot!("performing_replacement_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_results() {
    let (mut app, _app_event_receiver) =
        App::new_with_receiver(None, false, false, &SearchFieldValues::default());

    let replace_state_with_errors = ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![SearchResult {
            path: PathBuf::from("error.txt"),
            line_number: 1,
            line: "test line".to_string(),
            replacement: "replacement".to_string(),
            included: true,
            replace_result: Some(ReplaceResult::Error("Test error".to_string())),
        }],
        replacement_errors_pos: 0,
    };
    app.current_screen = Screen::Results(replace_state_with_errors);

    assert_debug_snapshot!("results_with_errors_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("results_with_errors_all_keymaps", app.keymaps_all());

    let replace_state_without_errors = ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![],
        replacement_errors_pos: 0,
    };
    app.current_screen = Screen::Results(replace_state_without_errors);

    assert_debug_snapshot!(
        "results_without_errors_compact_keymaps",
        app.keymaps_compact()
    );
    assert_debug_snapshot!("results_without_errors_all_keymaps", app.keymaps_all());
}

#[tokio::test]
async fn test_keymaps_popup() {
    let (mut app, _app_event_receiver) =
        App::new_with_receiver(None, false, false, &SearchFieldValues::default());
    app.add_error(AppError {
        name: "Test".to_string(),
        long: "Test error".to_string(),
    });

    assert_debug_snapshot!("popup_compact_keymaps", app.keymaps_compact());
    assert_debug_snapshot!("popup_all_keymaps", app.keymaps_all());
}
