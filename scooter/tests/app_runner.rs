use anyhow::bail;
use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use futures::Stream;
use insta::assert_snapshot;
use rand::Rng;
use ratatui::backend::TestBackend;
use regex::Regex;
use serial_test::serial;
use std::{env, io, path::Path, pin::Pin, task::Poll};
use tempfile::TempDir;
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time::{sleep, Duration, Instant},
};

use scooter::app_runner::{AppConfig, AppRunner};
use scooter_core::{
    app::AppRunConfig,
    fields::{FieldValue, SearchFieldValues},
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
            "{}Successful replacements \\(lines\\):.*\n.*{num_success} (.|\n)*Ignored \\(lines\\):.*\n.*{num_ignored} (.|\n)*Errors:.*\n.*{num_errors} (.|\n)*{}",
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

fn assert_snapshot_with_filters(name: &str, snapshot: impl AsRef<str>) {
    insta::with_settings!({filters => vec![
        (r"\[Time taken: [^\]]+\]", "[Time taken: TIME]"),
    ]}, {
        assert_snapshot!(name, snapshot.as_ref());
    });
}

async fn wait_for_match(
    snapshot_rx: &mut UnboundedReceiver<String>,
    pattern: Pattern,
    timeout_ms: u64,
) -> anyhow::Result<String> {
    wait_for_match_impl(snapshot_rx, pattern, true, timeout_ms).await
}

async fn wait_for_match_impl(
    snapshot_rx: &mut UnboundedReceiver<String>,
    pattern: Pattern,
    should_match: bool,
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
                    Some(s) if should_match == pattern.is_match(&s) => return Ok(s),
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
    build_test_runner_with_width(directory, advanced_regex, 30)
}

fn build_test_runner_with_width(
    directory: Option<&Path>,
    advanced_regex: bool,
    width: u16,
) -> anyhow::Result<TestRunner> {
    let config = AppConfig {
        directory: directory.map_or(env::current_dir().unwrap(), Path::to_path_buf),
        app_run_config: AppRunConfig {
            advanced_regex,
            ..AppRunConfig::default()
        },
        ..AppConfig::default()
    };
    build_test_runner_with_config_and_width(config, width)
}

fn build_test_runner_with_config(config: AppConfig<'_>) -> anyhow::Result<TestRunner> {
    build_test_runner_with_config_and_width(config, 24)
}

fn build_test_runner_with_config_and_width(
    config: AppConfig<'_>,
    width: u16,
) -> anyhow::Result<TestRunner> {
    let backend = TestBackend::new(width * 10 / 3, width);

    let (event_sender, event_stream) = TestEventStream::new();
    let (snapshot_tx, snapshot_rx) = mpsc::unbounded_channel();

    let mut runner = AppRunner::new_test_with_snapshot(config, backend, event_stream, snapshot_tx)?;
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
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )))?;

    let timeout_res = tokio::time::timeout(Duration::from_secs(1), async {
        run_handle.await.unwrap();
    })
    .await;

    assert!(
        timeout_res.is_ok(),
        "Couldn't shut down in a reasonable time"
    );
    Ok(())
}

fn send_key_with_modifiers(
    key: KeyCode,
    modifiers: KeyModifiers,
    event_sender: &UnboundedSender<CrosstermEvent>,
) {
    event_sender
        .send(CrosstermEvent::Key(KeyEvent::new(key, modifiers)))
        .unwrap_or_else(|e| panic!("failed to send key {key:?}, modifiers {modifiers:?}\n{e}"));
}

fn send_key(key: KeyCode, event_sender: &UnboundedSender<CrosstermEvent>) {
    send_key_with_modifiers(key, KeyModifiers::empty(), event_sender);
}

fn send_chars(word: &str, event_sender: &UnboundedSender<CrosstermEvent>) {
    word.chars()
        .for_each(|key| send_key(KeyCode::Char(key), event_sender));
}

#[tokio::test]
#[serial]
async fn test_search_current_dir() -> anyhow::Result<()> {
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(None, false)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("search", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_error_when_search_text_is_empty() -> anyhow::Result<()> {
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(None, false)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_key(KeyCode::Enter, &event_sender);

    let snapshot = wait_for_match(
        &mut snapshot_rx,
        Pattern::string("Search field must not be empty"),
        1000,
    )
    .await?;
    assert_snapshot!("search_text_empty_error", snapshot);

    shutdown(event_sender, run_handle).await
}

test_with_both_regex_modes!(
    test_search_and_replace_simple_dir,
    |advanced_regex| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => text!(
                "This is some test content before 123",
                "  with some spaces at the start",
                "and special ? characters 1! @@ # and number 890",
                "        some    tabs  and - more % special   **** characters ())",
            ),
            "file2.py" => text!(
                "from datetime import datetime as dt, timedelta as td",
                "def mix_types(x=100, y=\"test\"): return f\"{x}_{y}\" if isinstance(x, int) else None",
                "class TestClass:",
                "    super_long_name_really_before_long_name_very_long_name = 123",
                "    return super_long_name_really_before_long_name_very_long_name",
                "test_dict = {\"key1\": [1,2,3], 123: \"num key\", (\"a\",\"b\"): True, \"before\": 1, \"test-key\": None}",
            ),
        );

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner(Some(temp_dir.path()), advanced_regex)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("before", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("after", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Nothing should have changed yet
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => text!(
                "This is some test content before 123",
                "  with some spaces at the start",
                "and special ? characters 1! @@ # and number 890",
                "        some    tabs  and - more % special   **** characters ())",
            ),
            "file2.py" => text!(
                "from datetime import datetime as dt, timedelta as td",
                "def mix_types(x=100, y=\"test\"): return f\"{x}_{y}\" if isinstance(x, int) else None",
                "class TestClass:",
                "    super_long_name_really_before_long_name_very_long_name = 123",
                "    return super_long_name_really_before_long_name_very_long_name",
                "test_dict = {\"key1\": [1,2,3], 123: \"num key\", (\"a\",\"b\"): True, \"before\": 1, \"test-key\": None}",
            ),
        );

        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        // Verify that "before" has been replaced with "after"
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => text!(
                "This is some test content after 123",
                "  with some spaces at the start",
                "and special ? characters 1! @@ # and number 890",
                "        some    tabs  and - more % special   **** characters ())",
            ),
            "file2.py" => text!(
                "from datetime import datetime as dt, timedelta as td",
                "def mix_types(x=100, y=\"test\"): return f\"{x}_{y}\" if isinstance(x, int) else None",
                "class TestClass:",
                "    super_long_name_really_after_long_name_very_long_name = 123",
                "    return super_long_name_really_after_long_name_very_long_name",
                "test_dict = {\"key1\": [1,2,3], 123: \"num key\", (\"a\",\"b\"): True, \"after\": 1, \"test-key\": None}",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_search_and_replace_no_matches,
    |advanced_regex| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => text!(
                "This is some test content 123",
            ),
        );

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner(Some(temp_dir.path()), advanced_regex)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("before", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("after", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Nothing should have changed yet
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => text!(
                "This is some test content 123",
            ),
        );

        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        // Verify that nothing has changed
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => text!(
                "This is some test content 123",
            ),
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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("before", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("after", &event_sender);
        if fixed_strings {
            send_key(KeyCode::Tab, &event_sender);
            send_chars(" ", &event_sender); // Toggle on fixed strings
        }
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        assert_test_files!(&temp_dir);

        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(&temp_dir);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes_and_fixed_strings!(
    test_search_and_replace_whole_words,
    |advanced_regex, fixed_strings| async move {
        let temp_dir = &create_test_files!(
            "dir1/file1.txt" => text!(
                "this is something",
                "some text someone abcsome123",
                "some",
                "dashes-some-text",
                "slashes and commas/some,text",
                "moresometext",
                "text some",
            ),
            "file2.py" => text!(
                "print('Hello, some world!')",
            ),
        );

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner(Some(temp_dir.path()), advanced_regex)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Nothing should have changed yet
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => text!(
                "this is something",
                "some text someone abcsome123",
                "some",
                "dashes-some-text",
                "slashes and commas/some,text",
                "moresometext",
                "text some",
            ),
            "file2.py" => text!(
                "print('Hello, some world!')",
            ),
        );

        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        // Verify that "before" has been replaced with "after"
        assert_test_files!(
            &temp_dir,
            "dir1/file1.txt" => text!(
                "this is something",
                "REPLACE text someone abcsome123",
                "REPLACE",
                "dashes-REPLACE-text",
                "slashes and commas/REPLACE,text",
                "moresometext",
                "text REPLACE",
            ),
            "file2.py" => text!(
                "print('Hello, REPLACE world!')",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_search_and_replace_regex_capture_group,
    |advanced_regex| async move {
        let temp_dir = &create_test_files!(
            "phones.txt" => text!(
                "Phone: (020) 7123-4567",
                "Another: (0161) 4969-8523",
                "Different format: 020.7123.4567",
                "Also different: 020-7123-4567",
            ),
        );

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner(Some(temp_dir.path()), advanced_regex)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars(r"\((\d{3,4})\)\s(\d{4})-(\d{4})", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("+44 $2 $1-$3", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Nothing should have changed yet
        assert_test_files!(
            &temp_dir,
            "phones.txt" => text!(
                "Phone: (020) 7123-4567",
                "Another: (0161) 4969-8523",
                "Different format: 020.7123.4567",
                "Also different: 020-7123-4567",
            ),
        );

        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        // Verify only matching phone numbers are reformatted
        assert_test_files!(
            &temp_dir,
            "phones.txt" => text!(
                "Phone: +44 7123 020-4567",
                "Another: +44 4969 0161-8523",
                "Different format: 020.7123.4567",
                "Also different: 020-7123-4567",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

#[tokio::test]
#[serial]
async fn test_search_and_replace_advanced_regex_negative_lookahead() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => text!(
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
        ),
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    // Match 'let' declarations that aren't mutable
    // Use negative lookbehind for function parameters and negative lookahead for mut
    send_chars(r"(?<!mut\s)let\s(?!mut\s)(\w+)", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("let /* immutable */ $1", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    // Nothing should have changed yet
    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => text!(
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
        ),
    );

    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

    // Verify only non-mutable declarations are modified
    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => text!(
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
        ),
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_multi_select_mode() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => text!(
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
        ),
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => text!(
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
        ),
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_results_calculation_mixed() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => text!(
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
        ),
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::final_screen(true, 6, 2, 0), 1000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => text!(
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
        ),
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_results_calculation_all_success() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => text!(
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
        ),
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::final_screen(true, 8, 0, 0), 1000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => text!(
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
        ),
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_results_calculation_all_ignored() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => text!(
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
        ),
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Char('a'), &event_sender); // Toggle all off
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::final_screen(true, 0, 8, 0), 1000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => text!(
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
        ),
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_results_calculation_with_files_changed_errors() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => text!(
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
        ),
        "src/foo.rs" => text!(
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
            "",
        ),
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

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

    wait_for_match(
        &mut snapshot_rx,
        Pattern::final_screen(false, 3, 2, 3),
        1000,
    )
    .await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => text!(
            "fn process(mut data: Vec<u32>) {",
            "    changed mut count = 0;",
            "}",
        ),
        "src/foo.rs" => text!(
            "fn compute(input: Vec<u32>) -> u32 {",
            "    changed mut sum = 0;",
            "    changed multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
            "",
        ),
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_results_calculation_with_files_deleted_errors() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => text!(
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
        ),
        "src/foo.rs" => text!(
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
            "",
        ),
        "src/bar.rs" => text!(
            "fn something() {",
            "    let greeting = \"Hello, world!\";",
            "    println!(\"{greeting}\");",
            "}",
            "",
        ),
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner_with_width(Some(temp_dir.path()), true, 40)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    // Tab through to make sure the preview doesn't break
    for _ in 0..10 {
        send_key(KeyCode::Tab, &event_sender);
    }
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    delete_files!(&temp_dir.path(), "src/lib.rs", "src/foo.rs");

    send_key(KeyCode::Enter, &event_sender);

    let snapshot = wait_for_match(
        &mut snapshot_rx,
        Pattern::final_screen(false, 1, 0, 8),
        1000,
    )
    .await?;
    // Verify that errors are shown
    for path in [
        r"src(/|\\)foo.rs:3",
        r"src(/|\\)foo.rs:4",
        r"src(/|\\)lib.rs:2",
        r"src(/|\\)lib.rs:3",
        r"src(/|\\)lib.rs:4",
        r"src(/|\\)lib.rs:6",
    ] {
        let re = Regex::new(path).unwrap();
        assert!(
            re.is_match(&snapshot),
            "Expected snapshot to contain '{path}'\nFound:\n{snapshot}"
        );
    }

    assert_test_files!(
        &temp_dir,
        "src/bar.rs" => text!(
            "fn something() {",
            "    changed greeting = \"Hello, world!\";",
            "    println!(\"{greeting}\");",
            "}",
            "",
        ),
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_results_calculation_with_directory_deleted_errors() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => text!(
            "fn process(mut data: Vec<u32>) {",
            "    let mut count = 0;",
            "    let total = 0;",
            "    let values = Vec::new();",
            "    let mut items = data.clone();",
            "    let result = compute(data);",
            "}",
        ),
        "src/foo.rs" => text!(
            "fn compute(input: Vec<u32>) -> u32 {",
            "    let mut sum = 0;",
            "    let multiplier = 2;",
            "    let base = 10;",
            "    sum",
            "}",
            "",
        ),
        "src/bar.rs" => text!(
            "fn something() {",
            "    let greeting = \"Hello, world!\";",
            "    println!(\"{greeting}\");",
            "}",
            "",
        ),
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), true)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("let", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("changed", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    delete_files!(&temp_dir.path(), "src/");

    send_key(KeyCode::Char('j'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Char('G'), &event_sender);
    send_key(KeyCode::Char('k'), &event_sender);
    send_key(KeyCode::Char(' '), &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(
        &mut snapshot_rx,
        Pattern::final_screen(false, 0, 2, 7),
        1000,
    )
    .await?;

    assert_test_files!(&temp_dir);

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_help_screen_keymaps() -> anyhow::Result<()> {
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(None, false)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_key_with_modifiers(KeyCode::Char('h'), KeyModifiers::CONTROL, &event_sender);

    let snapshot = get_snapshot_after_wait(&mut snapshot_rx, 100).await?;
    assert_snapshot!("search_fields_help_screen_open", snapshot);

    send_key(KeyCode::Esc, &event_sender);

    let snapshot = get_snapshot_after_wait(&mut snapshot_rx, 100).await?;
    assert_snapshot!("search_fields_help_screen_closed", snapshot);

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_validation_errors() -> anyhow::Result<()> {
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(None, false)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    // Invalid regex in search
    send_key(KeyCode::Char('('), &event_sender);
    send_key(KeyCode::BackTab, &event_sender);
    // Invalid glob in files to exclude
    send_chars("{{", &event_sender);
    send_key(KeyCode::BackTab, &event_sender);
    // Invalid glob in files to include
    send_chars("*, {", &event_sender);

    let snapshot = get_snapshot_after_wait(&mut snapshot_rx, 100).await?;
    assert_snapshot!("search_fields_validation_errors_before_enter", snapshot);

    send_key(KeyCode::Enter, &event_sender);

    let snapshot = get_snapshot_after_wait(&mut snapshot_rx, 100).await?;
    assert_snapshot!("search_fields_validation_errors_shown", snapshot);

    send_key(KeyCode::Esc, &event_sender);

    let snapshot = get_snapshot_after_wait(&mut snapshot_rx, 100).await?;
    assert_snapshot!("search_fields_validation_errors_closed", snapshot);

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_prepopulated_fields() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "src/lib.rs" => text!(
            "fn process(mut data: Vec<u32>) {",
            "    let mut old_value = 0;",
            "    let result = compute(data);",
            "}",
        ),
        "src/foo.py" => text!(
            "def foo():",
            "    old_value = 0",
            "    result = compute(data)",
        ),
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
        match_whole_word: FieldValue {
            value: false,
            set_by_cli: true,
        },
        ..SearchFieldValues::default()
    };

    let config = AppConfig {
        directory: temp_dir.path().to_path_buf(),
        search_field_values,
        ..AppConfig::default()
    };
    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner_with_config_and_width(config, 48)?;

    // Search should happen automatically as the search field was prepopulated
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    // Pre-populated fields should be skipped when tabbing
    send_key(KeyCode::Tab, &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    // We should be at `include_files` field now
    send_chars("foo.py", &event_sender);

    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

    assert_test_files!(
        &temp_dir,
        "src/lib.rs" => text!(
            "fn process(mut data: Vec<u32>) {",
            "    let mut old_value = 0;",
            "    let result = compute(data);",
            "}",
        ),
        "src/foo.py" => text!(
            "def foo():",
            "    new_value = 0",
            "    result = compute(data)",
        ),
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
#[serial]
async fn test_replacement_progress_display() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "file1.txt" => text!(
            "This is a test file",
            "It contains some test content",
            "For testing purposes",
        ),
        "file2.txt" => text!(
            "Another test file here",
            "Also with test content",
            "test test test",
        ),
        "file3.txt" => text!(
            "Third file for testing",
            "More test data",
        ),
    );

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner(Some(temp_dir.path()), false)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("test", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("TEST", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(
        &mut snapshot_rx,
        Pattern::regex_must_compile(
            r"Performing replacement\.\.\.\s*\n\s*Completed: \d+/8 \(\d+\.\d{2}%\)\s*\n\s*Time: \d+\.\d{3}s",
        ),
        1000,
    )
    .await?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

    assert_test_files!(
        &temp_dir,
        "file1.txt" => text!(
            "This is a TEST file",
            "It contains some TEST content",
            "For TESTing purposes",
        ),
        "file2.txt" => text!(
            "Another TEST file here",
            "Also with TEST content",
            "TEST TEST TEST",
        ),
        "file3.txt" => text!(
            "Third file for TESTing",
            "More TEST data",
        ),
    );

    shutdown(event_sender, run_handle).await
}

test_with_both_regex_modes!(
    test_immediate_search_flag_skips_search_screen,
    |advanced_regex| async move {
        let temp_dir = &create_test_files!(
            "file1.txt" => text!(
                "This is some test content with SEARCH",
                "Another line with SEARCH here",
                "No match on this line",
            ),
            "file2.txt" => text!(
                "Start of file",
                "SEARCH appears here too",
                "End of file",
            ),
        );

        let search_field_values = SearchFieldValues {
            search: FieldValue::new("SEARCH", false),
            replace: FieldValue::new("REPLACED", false),
            ..SearchFieldValues::default()
        };
        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            search_field_values,
            app_run_config: AppRunConfig {
                advanced_regex,
                immediate_search: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 500).await?;
        assert!(Regex::new(r"file1\.txt").unwrap().is_match(&snapshot),);
        assert!(Regex::new(r"file2\.txt").unwrap().is_match(&snapshot),);

        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is some test content with REPLACED",
                "Another line with REPLACED here",
                "No match on this line",
            ),
            "file2.txt" => text!(
                "Start of file",
                "REPLACED appears here too",
                "End of file",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_immediate_replace_flag_skips_confirmation,
    |advanced_regex| async move {
        let temp_dir = &create_test_files!(
            "file1.txt" => text!(
                "Beautiful is better than ugly.",
                "Explicit is better than implicit.",
                "Simple is better than complex.",
                "Complex is better than complicated.",
            ),
            "file2.txt" => text!(
                "Flat is better than nested.",
                "Sparse is better than dense.",
                "Readability counts.",
                "Special cases aren't special enough to break the rules.",
                "Although practicality beats purity.",
                "Errors should never pass silently.",
            ),
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                immediate_replace: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("is", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACEMENT", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        // Replacement should happen without confirmation
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "Beautiful REPLACEMENT better than ugly.",
                "Explicit REPLACEMENT better than implicit.",
                "Simple REPLACEMENT better than complex.",
                "Complex REPLACEMENT better than complicated.",
            ),
            "file2.txt" => text!(
                "Flat REPLACEMENT better than nested.",
                "Sparse REPLACEMENT better than dense.",
                "Readability counts.",
                "Special cases aren't special enough to break the rules.",
                "Although practicality beats purity.",
                "Errors should never pass silently.",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_search_replace_defaults,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For TESTING purposes",
                "Test TEST tEsT tesT test",
                "TestbTESTctEsTdtesTetest",
                " test ",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "test-TEST-tESt",
                "something",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("t[esES]+t", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("123,", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a 123, file",
                "It contains some 123, content",
                "For TESTING purposes",
                "Test TEST tEsT tesT 123,",
                "TestbTESTctEsTdtesTe123,",
                " 123, ",
            ),
            "file2.txt" => text!(
                "Another 123, file",
                "With different content",
                "Also for 123,ing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "123,-TEST-123,",
                "something",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_search_replace_fixed_string,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars(".*", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("example", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Char(' '), &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+examplebar)(baz 456",
                "something",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_search_replace_match_case,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For TESTING purposes",
                "Test TEST tEsT tesT test",
                "TestbTESTctEsTdtesTetest",
                " test ",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "test-TEST-tESt",
                "something",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("test", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACEMENT", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        // Toggle on "fixed strings"
        send_key(KeyCode::Char(' '), &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a REPLACEMENT file",
                "It contains some REPLACEMENT content",
                "For TESTING purposes",
                "Test TEST tEsT tesT REPLACEMENT",
                "TestbTESTctEsTdtesTeREPLACEMENT",
                " REPLACEMENT ",
            ),
            "file2.txt" => text!(
                "Another REPLACEMENT file",
                "With different content",
                "Also for REPLACEMENTing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "REPLACEMENT-TEST-tESt",
                "something",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_search_replace_dont_match_case,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For TESTING purposes",
                "Test TEST tEsT tesT test",
                "TestbTESTctEsTdtesTetest",
                " test ",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "test-TEST-tESt",
                "something",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("test", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACEMENT", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        // Toggle on "fixed strings"
        send_key(KeyCode::Char(' '), &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        // Toggle off "match case"
        send_key(KeyCode::Char(' '), &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a REPLACEMENT file",
                "It contains some REPLACEMENT content",
                "For REPLACEMENTING purposes",
                "REPLACEMENT REPLACEMENT REPLACEMENT REPLACEMENT REPLACEMENT",
                "REPLACEMENTbREPLACEMENTcREPLACEMENTdREPLACEMENTeREPLACEMENT",
                " REPLACEMENT ",
            ),
            "file2.txt" => text!(
                "Another REPLACEMENT file",
                "With different content",
                "Also for REPLACEMENTing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "REPLACEMENT-REPLACEMENT-REPLACEMENT",
                "something",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_update_search_results_regex,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars(r"\b\w+ing\b", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("VERB", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For VERB purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for VERB",
            ),
            "file3.txt" => text!(
                "VERB",
                "123 bar[a-b]+.*bar)(baz 456",
                "VERB",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_update_search_results_no_matches,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("nonexistent-string", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("replacement", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        // Toggle on "fixed strings"
        send_key(KeyCode::Char(' '), &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_update_search_results_invalid_regex,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("[invalid regex", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("replacement", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        let snapshot = wait_for_match(&mut snapshot_rx, Pattern::string("Errors"), 500).await?;
        assert_snapshot!(
            format!(
                "error_with{suffix}_advanced_regex",
                suffix = if advanced_regex { "" } else { "out" }
            ),
            snapshot
        );

        // Nothing should have changed
        assert_test_files!(
            &temp_dir,
            "file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

#[tokio::test]
#[serial]
async fn test_advanced_regex_negative_lookahead() -> anyhow::Result<()> {
    let temp_dir = &create_test_files!(
        "file1.txt" => text!(
            "This is a test file",
            "It contains some test content",
            "For testing purposes",
        ),
        "file2.txt" => text!(
            "Another test file",
            "With different content",
            "Also for testing",
        ),
        "file3.txt" => text!(
            "something",
            "123 bar[a-b]+.*bar)(baz 456",
            "something",
        )
    );

    let config = AppConfig {
        directory: temp_dir.path().to_path_buf(),
        app_run_config: AppRunConfig {
            advanced_regex: true,
            ..AppRunConfig::default()
        },
        ..AppConfig::default()
    };
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("(test)(?!ing)", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("BAR", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Enter, &event_sender);
    wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

    assert_test_files!(
        temp_dir,
        "file1.txt" => text!(
            "This is a BAR file",
            "It contains some BAR content",
            "For testing purposes",
        ),
        "file2.txt" => text!(
            "Another BAR file",
            "With different content",
            "Also for testing",
        ),
        "file3.txt" => text!(
            "something",
            "123 bar[a-b]+.*bar)(baz 456",
            "something",
        )
    );

    shutdown(event_sender, run_handle).await
}

test_with_both_regex_modes!(
    test_update_search_results_include_dir,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for testing",
            ),
            "dir2/file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something testing",
            ),
            "dir3/file4.txt" => text!(
                "some testing text from dir3/file4.txt, blah",
            ),
            "dir3/subdir1/file5.txt" => text!(
                "some testing text from dir3/subdir1/file5.txt, blah",
            ),
            "dir4/subdir2/file6.txt" => text!(
                "some testing text from dir4/subdir2/file6.txt, blah",
            ),
            "dir4/subdir3/file7.txt" => text!(
                "some testing text from dir4/subdir3/file7.txt, blah",
            ),
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("testing", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("f", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("dir2/*, dir3/**, */subdir3/*", &event_sender); // "Files to include" field
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.txt" => text!(
                "Another test file",
                "With different content",
                "Also for f",
            ),
            "dir2/file3.txt" => text!(
                "something",
                "123 bar[a-b]+.*bar)(baz 456",
                "something f",
            ),
            "dir3/file4.txt" => text!(
                "some f text from dir3/file4.txt, blah",
            ),
            "dir3/subdir1/file5.txt" => text!(
                "some f text from dir3/subdir1/file5.txt, blah",
            ),
            "dir4/subdir2/file6.txt" => text!(
                "some testing text from dir4/subdir2/file6.txt, blah",
            ),
            "dir4/subdir3/file7.txt" => text!(
                "some f text from dir4/subdir3/file7.txt, blah",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_update_search_results_exclude_dir,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir1/file1.rs" => text!(
                "func testing() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "testing""#,
                "}",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("testing", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPL", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("dir1", &event_sender); // "Files to exclude" field
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir1/file1.rs" => text!(
                "func testing() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For REPL purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "REPL""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "REPL""#,
                "}",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_update_search_results_multiple_includes_and_excludes,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir1/file1.rs" => text!(
                "func testing1() {",
                r#"  "testing1""#,
                "}",
            ),
            "dir1/file2.rs" => text!(
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "testing""#,
                "}",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("testing", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPL", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("dir1/*, *.rs", &event_sender); // "Files to include" field
        send_key(KeyCode::Tab, &event_sender);
        send_chars("**/file2.rs", &event_sender); // "Files to exclude" field
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For REPL purposes",
            ),
            "dir1/file1.rs" => text!(
                "func REPL1() {",
                r#"  "REPL1""#,
                "}",
            ),
            "dir1/file2.rs" => text!(
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "REPL""#,
                "}",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_update_search_results_multiple_includes_and_excludes_additional_spacing,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir1/file1.rs" => text!(
                "func testing1() {",
                r#"  "testing1""#,
                "}",
            ),
            "dir1/file2.rs" => text!(
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            ),
            "dir1/subdir1/subdir2/file3.rs" => text!(
                "func testing3() {",
                r#"  "testing3""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file4.py" => text!(
                "def main():",
                "  return 'testing'",
            ),
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("testing", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPL", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("dir1/*, *.rs, *.py", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("**/file2.rs", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For REPL purposes",
            ),
            "dir1/file1.rs" => text!(
                "func REPL1() {",
                r#"  "REPL1""#,
                "}",
            ),
            "dir1/file2.rs" => text!(
                "func testing2() {",
                r#"  "testing2""#,
                "}",
            ),
            "dir1/subdir1/subdir2/file3.rs" => text!(
                "func REPL3() {",
                r#"  "REPL3""#,
                "}",
            ),
            "dir2/file1.txt" => text!(
                "This is a test file",
                "It contains some test content",
                "For testing purposes",
            ),
            "dir2/file2.rs" => text!(
                "func main2() {",
                r#"  "testing""#,
                "}",
            ),
            "dir2/file3.rs" => text!(
                "func main3() {",
                r#"  "REPL""#,
                "}",
            ),
            "dir2/file4.py" => text!(
                "def main():",
                "  return 'REPL'",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(test_ignores_gif_file, |advanced_regex: bool| async move {
    let temp_dir = &create_test_files!(
        "dir1/file1.txt" => text!(
            "This is a text file",
        ),
        "dir2/file2.gif" => text!(
            "This is a gif file",
        ),
        "file3.txt" => text!(
            "This is a text file",
        )
    );

    let config = AppConfig {
        directory: temp_dir.path().to_path_buf(),
        app_run_config: AppRunConfig {
            advanced_regex,
            ..AppRunConfig::default()
        },
        ..AppConfig::default()
    };
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

    send_chars("is", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

    send_key(KeyCode::Enter, &event_sender);
    wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

    assert_test_files!(
        temp_dir,
        "dir1/file1.txt" => text!(
            "Th  a text file",
        ),
        "dir2/file2.gif" => text!(
            "This is a gif file",
        ),
        "file3.txt" => text!(
            "Th  a text file",
        )
    );

    shutdown(event_sender, run_handle).await
});

test_with_both_regex_modes!(
    test_ignores_hidden_files_by_default,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a text file",
            ),
            ".dir2/file2.rs" => text!(
                "This is a file in a hidden directory",
            ),
            ".file3.txt" => text!(
                "This is a hidden text file",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars(r"\bis\b", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This REPLACED a text file",
            ),
            ".dir2/file2.rs" => text!(
                "This is a file in a hidden directory",
            ),
            ".file3.txt" => text!(
                "This is a hidden text file",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_includes_hidden_files_with_flag,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a text file",
            ),
            ".dir2/file2.rs" => text!(
                "This is a file in a hidden directory",
            ),
            ".file3.txt" => text!(
                "This is a hidden text file",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                include_hidden: true,
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars(r"\bis\b", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "dir1/file1.txt" => text!(
                "This REPLACED a text file",
            ),
            ".dir2/file2.rs" => text!(
                "This REPLACED a file in a hidden directory",
            ),
            ".file3.txt" => text!(
                "This REPLACED a hidden text file",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<()> {
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn read_file<P>(p: P) -> String
where
    P: AsRef<Path>,
{
    std::fs::read_to_string(p).unwrap().replace("\r\n", "\n")
}

test_with_both_regex_modes!(
    test_binary_file_filtering,
    |advanced_regex: bool| async move {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let fixtures_dir = "tests/fixtures/binary_test";
        copy_dir_all(format!("{fixtures_dir}/initial"), temp_dir.path())?;

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("sample", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

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
            let actual = std::fs::read(temp_dir.path().join(file))?;
            let original = std::fs::read(format!("{fixtures_dir}/initial/{file}"))?;
            assert_eq!(
                actual, original,
                "Binary file {file} was unexpectedly modified",
            );
        }

        shutdown(event_sender, run_handle).await
    }
);

fn count_occurrences(dir: &Path) -> anyhow::Result<(usize, usize)> {
    fn count_in_dir(
        dir: &Path,
        foo_count: &mut usize,
        bar_count: &mut usize,
    ) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                let content = std::fs::read_to_string(&path)?;
                *foo_count += content.matches("foo").count();
                *bar_count += content.matches("bar").count();
            } else if path.is_dir() {
                count_in_dir(&path, foo_count, bar_count)?;
            }
        }
        Ok(())
    }

    let mut foo_count = 0;
    let mut bar_count = 0;

    count_in_dir(dir, &mut foo_count, &mut bar_count)?;
    Ok((foo_count, bar_count))
}

async fn setup_test_data_with_foos_and_bars() -> anyhow::Result<(TempDir, usize, usize)> {
    const TARGET_FILE_SIZE: usize = 10 * 1024;
    const NUM_FILES: usize = 200;
    const FOO_PROBABILITY: u8 = 15;
    const BAR_PROBABILITY: u8 = 10;

    let temp_dir = TempDir::new().unwrap();

    let chars: Vec<char> =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 \n.,!?;:"
            .chars()
            .collect();

    let mut rng = rand::rng();
    let mut foo_count = 0;
    let mut bar_count = 0;

    for i in 0..NUM_FILES {
        let mut content = String::with_capacity(TARGET_FILE_SIZE);

        while content.len() < TARGET_FILE_SIZE {
            let choice = rng.random_range(0..100);

            if choice < FOO_PROBABILITY {
                content.push_str("foo");
                foo_count += 1;
            } else if choice < FOO_PROBABILITY + BAR_PROBABILITY {
                content.push_str("bar");
                bar_count += 1;
            } else {
                let chunk_size = rng.random_range(10..100);
                for _ in 0..chunk_size {
                    content.push(chars[rng.random_range(0..chars.len())]);
                }
            }
        }

        let file_path = temp_dir.path().join(format!("data_file_{i}.txt"));
        tokio::fs::write(&file_path, content).await?;
    }

    let (initial_foo_count, initial_bar_count) = count_occurrences(temp_dir.path())?;

    // The actual count should be at least what we tracked, but might be more due to
    // random generation creating "foo" or "bar" sequences, hence the >=
    assert!(
        initial_foo_count >= foo_count,
        "Test setup failed: found fewer 'foo' ({initial_foo_count}) than we intentionally created ({foo_count})",
    );
    assert!(
        initial_bar_count >= bar_count,
        "Test setup failed: found fewer 'bar' ({initial_bar_count}) than we intentionally created ({bar_count})",
    );
    Ok((temp_dir, initial_foo_count, initial_bar_count))
}

test_with_both_regex_modes!(
    test_preview_updates_work_correctly,
    |advanced_regex: bool| async move {
        let (temp_dir, initial_foo_count, initial_bar_count) =
            setup_test_data_with_foos_and_bars().await?;
        assert!(
            initial_foo_count > 0,
            "Test setup failed: should have at least some 'foo' occurrences"
        );
        assert!(
            initial_bar_count > 0,
            "Test setup failed: should have at least some 'bar' occurrences"
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(config, 48)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars("qux", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("bux", &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::BackTab, &event_sender);
        for _ in 0..3 {
            send_key(KeyCode::Backspace, &event_sender);
        }
        send_chars("foo", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        for _ in 0..3 {
            send_key(KeyCode::Backspace, &event_sender);
        }
        send_chars("baz", &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        // Don't wait for "Search complete", start searching again immediately

        send_key(KeyCode::Backspace, &event_sender);
        send_chars("r", &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        let (mid_foo_count, mid_bar_count) = count_occurrences(temp_dir.path())?;
        assert_eq!(
            mid_foo_count, initial_foo_count,
            "Mid-test foo count mismatch - nothing should have changed yet"
        );
        assert_eq!(
            mid_bar_count, initial_bar_count,
            "Mid-test bar count mismatch - nothing should have changed yet"
        );

        send_key(KeyCode::Enter, &event_sender); // Jump to search results
        send_key(KeyCode::Enter, &event_sender); // Try to begin replacement

        let timeout = Duration::from_millis(3000);
        let result = tokio::time::timeout(timeout, async {
            // State enum to prevent pressing key twice
            #[derive(PartialEq, Eq)]
            enum WaitingFor {
                PopupToClose,
                PopupToOpenOrReplacementToStart,
            }
            let mut current_state = WaitingFor::PopupToOpenOrReplacementToStart;
            loop {
                match snapshot_rx.recv().await {
                    Some(snapshot) => {
                        if snapshot.contains("Performing replacement...") {
                            return;
                        } else if snapshot.contains("Updating replacement preview") {
                            if current_state == WaitingFor::PopupToOpenOrReplacementToStart {
                                send_key(KeyCode::Esc, &event_sender); // Close popup
                            }
                            current_state = WaitingFor::PopupToClose;
                        } else {
                            if current_state == WaitingFor::PopupToClose {
                                send_key(KeyCode::Enter, &event_sender); // Try to begin replacement
                            }
                            current_state = WaitingFor::PopupToOpenOrReplacementToStart;
                        }
                    }
                    None => panic!("Snapshot channel closed"),
                }
            }
        })
        .await;
        assert!(result.is_ok(), "Timed out before preview was updated");

        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        let (final_foo_count, final_bar_count) = count_occurrences(temp_dir.path())?;
        eprintln!("[debug] initial_foo_count = {initial_foo_count} + initial_bar_count = {initial_bar_count}, final_foo_count={final_foo_count}, final_bar_count={final_bar_count}");
        assert_eq!(
            final_foo_count, 0,
            "Final foo count should be 0 after replacement"
        );
        assert_eq!(
            final_bar_count,
            initial_foo_count + initial_bar_count,
            "Final bar count should equal initial foo + bar counts"
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_text_wrapping_in_preview,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "This is a file with",
                "some very long lines of text. Lots of text which won't fit on one line so it will initially be truncated, but users can toggle text wrapping so that it all shows up on the screen.",
                "Some more lines here which aren't",
                "quite as long.",
                "Some",
                "more lines further",
                "on",
                "blah",
                "1",
                "2",
                "3",
                "4",
                "5",
                "6",
                "7",
                "8",
                "9 users 10",
                ".",
            ),
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                include_hidden: true,
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 10).await?;

        send_chars(r"users", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("text_wrapping_disabled__result_1", snapshot);

        send_key_with_modifiers(KeyCode::Char('l'), KeyModifiers::CONTROL, &event_sender);
        let snapshot = wait_for_match(
            &mut snapshot_rx,
            Pattern::string("it all shows up on the screen."),
            1000,
        )
        .await?;
        assert_snapshot_with_filters("text_wrapping_enabled__result_1", snapshot);
        send_key(KeyCode::Down, &event_sender);
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("9 users 10"), 1000).await?;
        assert_snapshot_with_filters("text_wrapping_enabled__result_2", snapshot);

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "This is a file with",
                "some very long lines of text. Lots of text which won't fit on one line so it will initially be truncated, but REPLACED can toggle text wrapping so that it all shows up on the screen.",
                "Some more lines here which aren't",
                "quite as long.",
                "Some",
                "more lines further",
                "on",
                "blah",
                "1",
                "2",
                "3",
                "4",
                "5",
                "6",
                "7",
                "8",
                "9 REPLACED 10",
                ".",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);
