use anyhow::bail;
use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use futures::Stream;
use insta::assert_snapshot;
use log::LevelFilter;
use ratatui::backend::TestBackend;
use regex::Regex;
use std::{io, path::Path, pin::Pin, task::Poll};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time::{sleep, Duration, Instant},
};

use scooter::{
    app_runner::{AppConfig, AppRunner},
    fields::{FieldValue, SearchFieldValues},
    test_with_both_regex_modes, test_with_both_regex_modes_and_fixed_strings,
};

mod utils;

struct TestEventStream(UnboundedReceiver<CrosstermEvent>);

impl TestEventStream {
    fn new() -> (UnboundedSender<CrosstermEvent>, Self) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (sender, Self(receiver))
    }
}

impl Stream for TestEventStream {
    type Item = Result<CrosstermEvent, io::Error>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.get_mut()
            .0
            .poll_recv(cx)
            .map(|opt| opt.map(Result::Ok))
    }
}

pub enum Pattern {
    String(String),
    Regex(Regex),
}

impl Pattern {
    fn string(s: &str) -> Self {
        Self::String(s.to_owned())
    }

    fn regex_must_compile(pattern: &str) -> Self {
        Pattern::Regex(Regex::new(pattern).unwrap())
    }

    fn final_screen(
        success: bool,
        num_success: usize,
        num_ignored: usize,
        num_errors: usize,
    ) -> Pattern {
        let s = format!(
            "{}Successful replacements:.*\n.*{num_success} (.|\n)*Ignored:.*\n.*{num_ignored} (.|\n)*Errors:.*\n.*{num_errors} (.|\n)*{}",
            if success { "Success!(.|\n)*" } else { "" },
            if success { "" } else { "Errors:" },
        );
        Pattern::regex_must_compile(&s)
    }

    fn is_match(&self, text: &str) -> bool {
        match self {
            Pattern::String(s) => text.contains(s),
            Pattern::Regex(r) => r.is_match(text),
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Pattern::String(s) => s,
            Pattern::Regex(r) => r.as_str(),
        }
    }
}

async fn wait_for_text(
    snapshot_rx: &mut UnboundedReceiver<String>,
    pattern: Pattern,
    timeout_ms: u64,
) -> anyhow::Result<String> {
    let timeout = Duration::from_millis(timeout_ms);
    let start = Instant::now();
    let mut last_snapshot = None;

    let err_with_snapshot =
        |error_msg: &str, last_snapshot: Option<String>| -> anyhow::Result<String> {
            let formatted_snapshot = match last_snapshot {
                Some(snapshot) => &format!("Current buffer snapshot:\n{snapshot}"),
                None => "No buffer snapshots received",
            };

            bail!(
                "{error_msg}: {patt}\n{formatted_snapshot}",
                patt = pattern.as_str().escape_debug(),
            )
        };

    while start.elapsed() <= timeout {
        tokio::select! {
            snapshot = snapshot_rx.recv() => {
                match snapshot {
                    Some(s) if pattern.is_match(&s) => return Ok(s),
                    Some(s) => { last_snapshot = Some(s); },
                    None => return err_with_snapshot("Channel closed while waiting for pattern", last_snapshot),
                }
            }
            () = sleep(timeout - start.elapsed()) => {
                break;
            }
        }
    }

    err_with_snapshot("Timeout waiting for pattern", last_snapshot)
}

async fn get_snapshot_after_wait(
    snapshot_rx: &mut UnboundedReceiver<String>,
    timeout_ms: u64,
) -> anyhow::Result<String> {
    let timeout = Duration::from_millis(timeout_ms);
    let start = Instant::now();
    let mut last_snapshot = None;

    while start.elapsed() <= timeout {
        tokio::select! {
            snapshot = snapshot_rx.recv() => {
                match snapshot {
                    Some(s) => { last_snapshot = Some(s); },
                    None => break, // Channel closed, return latest snapshot
                }
            }
            () = sleep(timeout - start.elapsed()) => {
                // Wait for more snapshots
            }
        }
    }

    match last_snapshot {
        Some(s) => Ok(s),
        None => bail!("No snapshots received within wait period"),
    }
}

type TestRunner = (
    JoinHandle<()>,
    UnboundedSender<CrosstermEvent>,
    UnboundedReceiver<String>,
);

fn build_test_runner(directory: Option<&Path>, advanced_regex: bool) -> anyhow::Result<TestRunner> {
    let config = AppConfig {
        directory: directory.map(|d| d.to_str().unwrap().to_owned()),
        include_hidden: false,
        advanced_regex,
        search_field_values: SearchFieldValues::default(),
        log_level: LevelFilter::Warn,
        immediate_search: false,
    };
    build_test_runner_with_config(config)
}

fn build_test_runner_with_config(config: AppConfig<'_>) -> anyhow::Result<TestRunner> {
    let backend = TestBackend::new(80, 24);

    let (event_sender, event_stream) = TestEventStream::new();
    let (snapshot_tx, snapshot_rx) = mpsc::unbounded_channel();

    let mut runner =
        AppRunner::new(config, backend, event_stream)?.with_snapshot_channel(snapshot_tx);
    runner.init()?;

    let run_handle = tokio::spawn(async move {
        runner.run_event_loop().await.unwrap();
    });

    Ok((run_handle, event_sender, snapshot_rx))
}

async fn shutdown(
    event_sender: UnboundedSender<CrosstermEvent>,
    run_handle: JoinHandle<()>,
) -> anyhow::Result<()> {
    event_sender.send(CrosstermEvent::Key(KeyEvent::new(
        KeyCode::Esc,
        KeyModifiers::empty(),
    )))?;

    run_handle.await?;

    Ok(())
}

fn send_key_with_modifiers(
    key: KeyCode,
    modifiers: KeyModifiers,
    event_sender: &UnboundedSender<CrosstermEvent>,
) {
    event_sender
        .send(CrosstermEvent::Key(KeyEvent::new(key, modifiers)))
        .unwrap();
}

fn send_key(key: KeyCode, event_sender: &UnboundedSender<CrosstermEvent>) {
    send_key_with_modifiers(key, KeyModifiers::empty(), event_sender);
}

fn send_chars(word: &str, event_sender: &UnboundedSender<CrosstermEvent>) {
    word.chars()
        .for_each(|key| send_key(KeyCode::Char(key), event_sender));
}

#[tokio::test]
async fn test_search_current_dir() -> anyhow::Result<()> {
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(None, false)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    shutdown(event_sender, run_handle).await
}

test_with_both_regex_modes!(
    test_search_and_replace_simple_dir,
    |advanced_regex| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => {
                "This is some test content before 123",
                "  with some spaces at the start",
                "and special ? characters 1! @@ # and number 890",
                "        some    tabs  and - more % special   **** characters ())",
            },
            "file2.py" => {
                "from datetime import datetime as dt, timedelta as td",
                "def mix_types(x=100, y=\"test\"): return f\"{x}_{y}\" if isinstance(x, int) else None",
                "class TestClass:",
                "    super_long_name_really_before_long_name_very_long_name = 123",
                "    return super_long_name_really_before_long_name_very_long_name",
                "test_dict = {\"key1\": [1,2,3], 123: \"num key\", (\"a\",\"b\"): True, \"before\": 1, \"test-key\": None}",
            },
        );

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner(Some(temp_dir.path()), advanced_regex)?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("before", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("after", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Nothing should have changed yet
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => {
                "This is some test content before 123",
                "  with some spaces at the start",
                "and special ? characters 1! @@ # and number 890",
                "        some    tabs  and - more % special   **** characters ())",
            },
            "file2.py" => {
                "from datetime import datetime as dt, timedelta as td",
                "def mix_types(x=100, y=\"test\"): return f\"{x}_{y}\" if isinstance(x, int) else None",
                "class TestClass:",
                "    super_long_name_really_before_long_name_very_long_name = 123",
                "    return super_long_name_really_before_long_name_very_long_name",
                "test_dict = {\"key1\": [1,2,3], 123: \"num key\", (\"a\",\"b\"): True, \"before\": 1, \"test-key\": None}",
            },
        );

        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 1000).await?;

        // Verify that "before" has been replaced with "after"
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => {
                "This is some test content after 123",
                "  with some spaces at the start",
                "and special ? characters 1! @@ # and number 890",
                "        some    tabs  and - more % special   **** characters ())",
            },
            "file2.py" => {
                "from datetime import datetime as dt, timedelta as td",
                "def mix_types(x=100, y=\"test\"): return f\"{x}_{y}\" if isinstance(x, int) else None",
                "class TestClass:",
                "    super_long_name_really_after_long_name_very_long_name = 123",
                "    return super_long_name_really_after_long_name_very_long_name",
                "test_dict = {\"key1\": [1,2,3], 123: \"num key\", (\"a\",\"b\"): True, \"after\": 1, \"test-key\": None}",
            },
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_search_and_replace_no_matches,
    |advanced_regex| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => {
                "This is some test content 123",
            },
        );

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner(Some(temp_dir.path()), advanced_regex)?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("before", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("after", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Nothing should have changed yet
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => {
                "This is some test content 123",
            },
        );

        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 1000).await?;

        // Verify that nothing has changed
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => {
                "This is some test content 123",
            },
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_search_and_replace_empty_dir,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = &create_test_files!();

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner(Some(temp_dir.path()), advanced_regex)?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("before", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("after", &event_sender);
        if fixed_strings {
            send_key(KeyCode::Tab, &event_sender);
            send_chars(" ", &event_sender); // Toggle on fixed strings
        }
        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        assert_test_files!(&temp_dir);

        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 1000).await?;

        assert_test_files!(&temp_dir);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_search_and_replace_whole_words,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => {
                "this is something",
                "some text someone abcsome123",
                "some",
                "dashes-some-text",
                "slashes and commas/some,text",
                "moresometext",
                "text some",
            },
            "file2.py" => {
                "print('Hello, some world!')",
            },
        );

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner(Some(temp_dir.path()), advanced_regex)?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("some", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACE", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        if fixed_strings {
            send_chars(" ", &event_sender); // Toggle on fixed strings
        }
        send_key(KeyCode::Tab, &event_sender);
        send_chars(" ", &event_sender); // Toggle on whole word matching
        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Nothing should have changed yet
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => {
                "this is something",
                "some text someone abcsome123",
                "some",
                "dashes-some-text",
                "slashes and commas/some,text",
                "moresometext",
                "text some",
            },
            "file2.py" => {
                "print('Hello, some world!')",
            },
        );

        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 1000).await?;

        // Verify that "before" has been replaced with "after"
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => {
                "this is something",
                "REPLACE text someone abcsome123",
                "REPLACE",
                "dashes-REPLACE-text",
                "slashes and commas/REPLACE,text",
                "moresometext",
                "text REPLACE",
            },
            "file2.py" => {
                "print('Hello, REPLACE world!')",
            },
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_search_and_replace_regex_capture_group,
    |advanced_regex| async move {
        let temp_dir = &create_test_files!(
            "phones.txt" => {
                "Phone: (020) 7123-4567",
                "Another: (0161) 4969-8523",
                "Different format: 020.7123.4567",
                "Also different: 020-7123-4567",
            },
        );

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner(Some(temp_dir.path()), advanced_regex)?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars(r"\((\d{3,4})\)\s(\d{4})-(\d{4})", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("+44 $2 $1-$3", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
        wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Nothing should have changed yet
        assert_test_files!(
            &temp_dir,
            "phones.txt" => {
                "Phone: (020) 7123-4567",
                "Another: (0161) 4969-8523",
                "Different format: 020.7123.4567",
                "Also different: 020-7123-4567",
            },
        );

        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 1000).await?;

        // Verify only matching phone numbers are reformatted
        assert_test_files!(
            &temp_dir,
            "phones.txt" => {
                "Phone: +44 7123 020-4567",
                "Another: +44 4969 0161-8523",
                "Different format: 020.7123.4567",
                "Also different: 020-7123-4567",
            },
        );

        shutdown(event_sender, run_handle).await
    }
);

#[tokio::test]
async fn test_search_and_replace_advanced_regex_negative_lookahead() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
        },
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    // Match 'let' declarations that aren't mutable
    // Use negative lookbehind for function parameters and negative lookahead for mut
    send_chars(r"(?<!mut\s)let\s(?!mut\s)(\w+)", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("let /* immutable */ $1", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    // Nothing should have changed yet
    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
        },
    );

    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 1000).await?;

    // Verify only non-mutable declarations are modified
    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let /* immutable */ total = 0;",
            "    let /* immutable */ values = Vec::new();",
            "    let mut items = data.clone();",
            "    let /* immutable */ result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let /* immutable */ multiplier = 2;",
            "    let /* immutable */ base = 10;",
            "    sum",
            "}",
        },
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_multi_select_mode() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
        },
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    // Highlight 3rd to 6th search result with multi-select, and 8th with single selection
    send_key(KeyCode::Char('a'), &event_sender); // Toggle all off
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char('v'), &event_sender); // Enable multi-select
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender); // Toggle multiple selected
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Esc, &event_sender); // Exit multi-select
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender); // Toggle single selected
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 1000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    changed values = Vec::new();",
            "    changed mut items = data.clone();",
            "    changed result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    changed mut sum = 0;",
            "    let multiplier = 2;",
            "    changed base = 10;",
            "    sum",
            "}",
        },
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_results_calculation_mixed() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
        },
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::final_screen(true, 6, 2, 0), 1000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    changed mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    changed mut items = data.clone();",
            "    changed result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    changed mut sum = 0;",
            "    changed multiplier = 2;",
            "    changed base = 10;",
            "    sum",
            "}",
        },
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_results_calculation_all_success() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
        },
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::final_screen(true, 8, 0, 0), 1000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    changed mut count = 0;",
            "    changed total = 0;",
            "    changed values = Vec::new();",
            "    changed mut items = data.clone();",
            "    changed result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    changed mut sum = 0;",
            "    changed multiplier = 2;",
            "    changed base = 10;",
            "    sum",
            "}",
        },
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_results_calculation_all_ignored() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
        },
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Char('a'), &event_sender); // Toggle all off
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::final_screen(true, 0, 8, 0), 1000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
            "",
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
        },
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_results_calculation_with_files_changed_errors() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
        },
        "src/foo.rs" => {
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
            "",
        },
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    overwrite_files!(
        &temp_dir.path(),
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "}",
        },
    );

    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Char('G'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(
        &mut snapshot_rx,
        Pattern::final_screen(false, 3, 2, 3),
        1000,
    )
    .await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    changed mut count = 0;",
            "}",
        },
        "src/foo.rs" => {
            "fn compute(input: Vec<u32>) -> u32 {",
            "    changed mut sum = 0;",
            "    changed multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
            "",
        },
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_results_calculation_with_files_deleted_errors() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
        },
        "src/foo.rs" => {
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
            "",
        },
        "src/bar.rs" => {
            "fn something() {",
            "    let greeting = \"Hello, world!\";",
            "    println!(\"{greeting}\");",
            "}",
            "",
        },
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    delete_files!(&temp_dir.path(), "src/lib.rs", "src/foo.rs");

    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Char('G'), &event_sender);
    send_key(KeyCode::Char('k'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(
        &mut snapshot_rx,
        Pattern::final_screen(false, 1, 2, 6),
        1000,
    )
    .await?;

    assert_test_files!(
        &temp_dir,
        "src/bar.rs" => {
            "fn something() {",
            "    changed greeting = \"Hello, world!\";",
            "    println!(\"{greeting}\");",
            "}",
            "",
        },
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_results_calculation_with_directory_deleted_errors() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
        },
        "src/foo.rs" => {
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
            "",
        },
        "src/bar.rs" => {
            "fn something() {",
            "    let greeting = \"Hello, world!\";",
            "    println!(\"{greeting}\");",
            "}",
            "",
        },
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    delete_files!(&temp_dir.path(), "src/");

    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Char('G'), &event_sender);
    send_key(KeyCode::Char('k'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(
        &mut snapshot_rx,
        Pattern::final_screen(false, 0, 2, 7),
        1000,
    )
    .await?;

    assert_test_files!(&temp_dir);

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_help_screen_keymaps() -> anyhow::Result<()> {
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(None, false)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_key_with_modifiers(KeyCode::Char('h'), KeyModifiers::CONTROL, &event_sender);

    let snapshot = get_snapshot_after_wait(&mut snapshot_rx, 100).await?;
    assert_snapshot!("search_fields_help_screen_open", snapshot);

    send_key(KeyCode::Esc, &event_sender);

    let snapshot = get_snapshot_after_wait(&mut snapshot_rx, 100).await?;
    assert_snapshot!("search_fields_help_screen_closed", snapshot);

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_prepopulated_fields() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut old_value = 0;",
            "    let result = compute(data);",
            "}",
        },
        "src/foo.py" => {
            "def foo():",
            "    old_value = 0",
            "    result = compute(data)",
        },
    );

    let search_field_values = SearchFieldValues {
        search: FieldValue {
            value: "old_value",
            set_by_cli: true,
        },
        replace: FieldValue {
            value: "new_value",
            set_by_cli: true,
        },
        fixed_strings: FieldValue {
            value: true,
            set_by_cli: false,
        },
        match_whole_word: FieldValue {
            value: false,
            set_by_cli: true,
        },
        match_case: FieldValue {
            value: false,
            set_by_cli: false,
        },
        include_files: FieldValue {
            value: "",
            set_by_cli: false,
        },
        exclude_files: FieldValue {
            value: "",
            set_by_cli: false,
        },
    };

    let config = AppConfig {
        directory: Some(temp_dir.path().to_string_lossy().into_owned()),
        include_hidden: false,
        advanced_regex: false,
        search_field_values,
        log_level: LevelFilter::Warn,
        immediate_search: false,
    };
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

    // Pre-populated fields should be skipped when tabbing
    send_key(KeyCode::Tab, &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    // We should be at `include_files` field now
    send_chars("foo.py", &event_sender);

    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 1000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => {
            "fn process(mut data: Vec<u32>) {",
            "    let mut old_value = 0;",
            "    let result = compute(data);",
            "}",
        },
        "src/foo.py" => {
            "def foo():",
            "    new_value = 0",
            "    result = compute(data)",
        },
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_replacement_progress_display() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "file1.txt" => {
            "This is a test file",
            "It contains some test content",
            "For testing purposes",
        },
        "file2.txt" => {
            "Another test file here",
            "Also with test content",
            "test test test",
        },
        "file3.txt" => {
            "Third file for testing",
            "More test data",
        },
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), false)?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("test", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("TEST", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 500).await?;
    wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(
        &mut snapshot_rx,
        Pattern::regex_must_compile(
            r"Performing replacement\.\.\.\s*\n\s*Completed: \d+/8 \(\d+\.\d{2}%\)\s*\n\s*Time: \d+\.\d{3}s",
        ),
        1000,
    )
    .await?;

    wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 500).await?;

    assert_test_files!(
        &temp_dir,
        "file1.txt" => {
            "This is a TEST file",
            "It contains some TEST content",
            "For TESTing purposes",
        },
        "file2.txt" => {
            "Another TEST file here",
            "Also with TEST content",
            "TEST TEST TEST",
        },
        "file3.txt" => {
            "Third file for TESTing",
            "More TEST data",
        },
    );

    shutdown(event_sender, run_handle).await
}

test_with_both_regex_modes!(
    test_immediate_search_flag_skips_search_screen,
    |advanced_regex| async move {
        let temp_dir = &create_test_files!(
            "file1.txt" => {
                "This is some test content with SEARCH",
                "Another line with SEARCH here",
                "No match on this line",
            },
            "file2.txt" => {
                "Start of file",
                "SEARCH appears here too",
                "End of file",
            },
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("SEARCH", false),
            replace: FieldValue::new("REPLACED", false),
            ..SearchFieldValues::default()
        };
        let config = AppConfig {
            directory: Some(temp_dir.path().to_str().unwrap().to_owned()),
            include_hidden: false,
            advanced_regex,
            search_field_values,
            log_level: LevelFilter::Warn,
            immediate_search: true,
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_text(&mut snapshot_rx, Pattern::string("Still searching"), 100).await?;
        let snapshot =
            wait_for_text(&mut snapshot_rx, Pattern::string("Search complete"), 500).await?;
        assert!(Regex::new(r"file1\.txt").unwrap().is_match(&snapshot),);
        assert!(Regex::new(r"file2\.txt").unwrap().is_match(&snapshot),);

        send_key(KeyCode::Enter, &event_sender);

        wait_for_text(&mut snapshot_rx, Pattern::string("Success!"), 500).await?;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => {
                "This is some test content with REPLACED",
                "Another line with REPLACED here",
                "No match on this line",
            },
            "file2.txt" => {
                "Start of file",
                "REPLACED appears here too",
                "End of file",
            },
        );

        shutdown(event_sender, run_handle).await
    }
);
