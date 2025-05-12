use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use scooter::{
    test_with_both_regex_modes, App, EventHandlingResult, PerformingReplacementState, Popup,
    ReplaceResult, ReplaceState, Screen, SearchFieldValues, SearchFields, SearchInProgressState,
    SearchResult, SearchState,
};
use serial_test::serial;
use std::cmp::max;
use std::mem;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::mpsc;

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
    let (mut app, _app_event_receiver) = App::new_with_receiver(None, false, false);
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
    let (mut app, _app_event_receiver) = App::new_with_receiver(None, false, false);
    let (_sender, receiver) = mpsc::unbounded_channel();
    app.current_screen = Screen::SearchComplete(SearchState::new(receiver));
    app.search_fields = SearchFields::with_values(SearchFieldValues {
        search: "foo",
        replace: "bar",
        fixed_strings: true,
        whole_word: false,
        match_case: true,
        include_files: "pattern",
        exclude_files: "",
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
    let (mut app, _app_event_receiver) = App::new_with_receiver(None, false, false);
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
        search: "search invalid regex(",
        replace: "replacement",
        fixed_strings: false,
        whole_word: false,
        match_case: true,
        include_files: "",
        exclude_files: "",
    });
}

#[tokio::test]
async fn test_error_popup_invalid_include_files() {
    test_error_popup_invalid_input_impl(SearchFieldValues {
        search: "search",
        replace: "replacement",
        fixed_strings: false,
        whole_word: false,
        match_case: true,
        include_files: "foo{",
        exclude_files: "",
    });
}

#[tokio::test]
async fn test_error_popup_invalid_exclude_files() {
    test_error_popup_invalid_input_impl(SearchFieldValues {
        search: "search",
        replace: "replacement",
        fixed_strings: false,
        whole_word: false,
        match_case: true,
        include_files: "",
        exclude_files: "bar{",
    });
}

fn test_help_popup_on_screen(initial_screen: Screen) {
    let (mut app, _app_event_receiver) = App::new_with_receiver(None, false, false);
    let screen_variant = std::mem::discriminant(&initial_screen);
    app.current_screen = initial_screen;

    assert!(app.popup().is_none());
    assert_eq!(mem::discriminant(&app.current_screen), screen_variant);

    let res_open = app.handle_key_event(&KeyEvent {
        code: KeyCode::Char('h'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert!(res_open == EventHandlingResult::Rerender);
    assert!(matches!(app.popup(), Some(Popup::Help)));
    assert_eq!(std::mem::discriminant(&app.current_screen), screen_variant);

    let res_close = app.handle_key_event(&KeyEvent {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
    assert!(res_close == EventHandlingResult::Rerender);
    assert!(app.popup().is_none());
    assert_eq!(std::mem::discriminant(&app.current_screen), screen_variant);
}

#[tokio::test]
async fn test_help_popup_on_search_fields() {
    test_help_popup_on_screen(Screen::SearchFields);
}

#[tokio::test]
async fn test_help_popup_on_search_in_progress() {
    let (_sender, receiver) = mpsc::unbounded_channel();
    let initial_screen =
        Screen::SearchProgressing(SearchInProgressState::new(tokio::spawn(async {}), receiver));
    test_help_popup_on_screen(initial_screen);
}

#[tokio::test]
async fn test_help_popup_on_search_complete() {
    let results = (0..100)
        .map(|i| SearchResult {
            path: PathBuf::from(format!("test{i}.txt")),
            line_number: 1,
            line: format!("test line {i}").to_string(),
            replacement: format!("replacement {i}").to_string(),
            included: true,
            replace_result: None,
        })
        .collect();
    let (_sender, receiver) = mpsc::unbounded_channel();
    let mut search_state = SearchState::new(receiver);
    search_state.results = results;

    test_help_popup_on_screen(Screen::SearchComplete(search_state));
}

#[tokio::test]
async fn test_help_popup_on_performing_replacement() {
    let (sender, receiver) = mpsc::unbounded_channel();
    let initial_screen =
        Screen::PerformingReplacement(PerformingReplacementState::new(None, sender, receiver));
    test_help_popup_on_screen(initial_screen);
}

#[tokio::test]
async fn test_help_popup_on_results() {
    let results_state = ReplaceState {
        num_successes: 5,
        num_ignored: 2,
        errors: vec![],
        replacement_errors_pos: 0,
    };
    test_help_popup_on_screen(Screen::Results(results_state));
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
    let (mut app, _app_event_receiver) =
        App::new_with_receiver(Some(temp_dir.path().to_path_buf()), include_hidden, false);
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
    let num_expected_matches = expected_matches
        .iter()
        .map(|(_, count)| count)
        .sum::<usize>();

    let mut app = setup_app(temp_dir, search_fields, include_hidden);
    let res = app.perform_search_if_valid();
    assert!(res != EventHandlingResult::Exit);

    process_bp_events(&mut app).await;
    assert!(wait_for_screen!(&app, Screen::SearchComplete));

    if let Screen::SearchComplete(search_state) = &mut app.current_screen {
        for (file_path, num_expected_matches) in &expected_matches {
            let num_actual_matches = search_state
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

        assert_eq!(search_state.results.len(), num_expected_matches);
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
        assert_eq!(search_state.num_successes, num_expected_matches);
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
        search: "t[esES]+t",
        replace: "123,",
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
            search: ".*",
            replace: "example",
            fixed_strings: true,
            whole_word: false,
            match_case: true,
            include_files: "",
            exclude_files: "",
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
            search: "test",
            replace: "REPLACEMENT",
            fixed_strings: true,
            whole_word: false,
            match_case: true,
            include_files: "",
            exclude_files: "",
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
            search: "test",
            replace: "REPLACEMENT",
            fixed_strings: true,
            whole_word: false,
            match_case: false,
            include_files: "",
            exclude_files: "",
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
            search: r"\b\w+ing\b",
            replace: "VERB",
            fixed_strings: false,
            whole_word: false,
            match_case: true,
            include_files: "",
            exclude_files: "",
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
            search: "nonexistent-string",
            replace: "replacement",
            fixed_strings: true,
            whole_word: false,
            match_case: true,
            include_files: "",
            exclude_files: "",
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
            search: "[invalid regex",
            replace: "replacement",
            fixed_strings: false,
            whole_word: false,
            match_case: true,
            include_files: "",
            exclude_files: "",
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
        search: "(test)(?!ing)",
        replace: "BAR",
        fixed_strings: false,
        whole_word: false,
        match_case: true,
        include_files: "",
        exclude_files: "",
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
            search: "testing",
            replace: "f",
            fixed_strings: false,
            whole_word: false,
            match_case: true,
            include_files: "dir2/*, dir3/**, */subdir3/*",
            exclude_files: "",
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
            search: "testing",
            replace: "REPL",
            fixed_strings: false,
            whole_word: false,
            match_case: true,
            include_files: "",
            exclude_files: "dir1",
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
            search: "testing",
            replace: "REPL",
            fixed_strings: false,
            whole_word: false,
            match_case: true,
            include_files: "dir1/*, *.rs",
            exclude_files: "**/file2.rs",
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
            search: "testing",
            replace: "REPL",
            fixed_strings: false,
            whole_word: false,
            match_case: true,
            include_files: " dir1/*,*.rs   ,  *.py",
            exclude_files: "  **/file2.rs ",
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
        search: "is",
        replace: "",
        fixed_strings: false,
        whole_word: false,
        match_case: true,
        include_files: "",
        exclude_files: "",
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
            search: r"\bis\b",
            replace: "REPLACED",
            fixed_strings: false,
            whole_word: false,
            match_case: true,
            include_files: "",
            exclude_files: "",
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
            search: r"\bis\b",
            replace: "REPLACED",
            fixed_strings: false,
            whole_word: false,
            match_case: true,
            include_files: "",
            exclude_files: "",
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

// TODO: tests for passing in directory via CLI arg
