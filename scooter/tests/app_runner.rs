use anyhow::bail;

use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use futures::Stream;
use insta::assert_snapshot;
use rand::RngExt;
use ratatui::backend::TestBackend;
use regex::Regex;
use scooter::app_runner::{AppConfig, AppRunner};
use serial_test::serial;
use std::{env, io, path::Path, pin::Pin, task::Poll};
use tempfile::TempDir;
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time::{Duration, Instant, sleep},
};

use scooter_core::{
    app::AppRunConfig,
    config::{Config, KeysConfig, KeysSearch, KeysSearchFocusFields, KeysSearchFocusResults},
    fields::{FieldValue, SearchFieldValues},
    keyboard::{
        KeyCode as CoreKeyCode, KeyEvent as CoreKeyEvent, KeyModifiers as CoreKeyModifiers,
    },
    keys,
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

const DEFAULT_TEST_WIDTH: u16 = 30;

fn build_test_runner(directory: Option<&Path>, advanced_regex: bool) -> anyhow::Result<TestRunner> {
    let app_config = AppConfig {
        directory: directory.map_or(env::current_dir().unwrap(), Path::to_path_buf),
        app_run_config: AppRunConfig {
            advanced_regex,
            ..AppRunConfig::default()
        },
        ..AppConfig::default()
    };
    build_test_runner_impl(app_config, Config::default(), DEFAULT_TEST_WIDTH)
}

fn build_test_runner_with_width(
    directory: Option<&Path>,
    advanced_regex: bool,
    width: u16,
) -> anyhow::Result<TestRunner> {
    let app_config = AppConfig {
        directory: directory.map_or(env::current_dir().unwrap(), Path::to_path_buf),
        app_run_config: AppRunConfig {
            advanced_regex,
            ..AppRunConfig::default()
        },
        ..AppConfig::default()
    };
    build_test_runner_impl(app_config, Config::default(), width)
}

fn build_test_runner_with_config(app_config: AppConfig<'_>) -> anyhow::Result<TestRunner> {
    build_test_runner_impl(app_config, Config::default(), DEFAULT_TEST_WIDTH)
}

fn build_test_runner_with_config_and_width(
    app_config: AppConfig<'_>,
    width: u16,
) -> anyhow::Result<TestRunner> {
    build_test_runner_impl(app_config, Config::default(), width)
}

fn build_test_runner_with_custom_config(
    app_config: AppConfig<'_>,
    user_config: Config,
) -> anyhow::Result<TestRunner> {
    build_test_runner_impl(app_config, user_config, DEFAULT_TEST_WIDTH)
}

fn build_test_runner_impl(
    app_config: AppConfig<'_>,
    user_config: Config,
    width: u16,
) -> anyhow::Result<TestRunner> {
    let backend = TestBackend::new(width * 10 / 3, width);

    let (event_sender, event_stream) = TestEventStream::new();
    let (snapshot_tx, snapshot_rx) = mpsc::unbounded_channel();

    let mut runner = AppRunner::new_snapshot_test_override_config(
        app_config,
        backend,
        event_stream,
        snapshot_tx,
        user_config,
    )?;
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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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
        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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
        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

test_with_both_regex_modes!(
    test_toggle_hidden_files_keybinding,
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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Search for "is" - should only find 1 result (hidden files excluded by default)
        send_chars(r"\bis\b", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Results: 1"), 100).await?;

        // Toggle hidden files on
        send_key_with_modifiers(KeyCode::Char('t'), KeyModifiers::CONTROL, &event_sender);

        // Wait for toast and re-search to complete - should now have 3 results
        wait_for_match(&mut snapshot_rx, Pattern::string("Hidden files: ON"), 100).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Results: 3"), 1000).await?;

        // Toggle hidden files off again
        send_key_with_modifiers(KeyCode::Char('t'), KeyModifiers::CONTROL, &event_sender);

        // Should go back to 1 result
        wait_for_match(&mut snapshot_rx, Pattern::string("Hidden files: OFF"), 100).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Results: 1"), 1000).await?;

        // Toggle hidden files back on for the replacement
        send_key_with_modifiers(KeyCode::Char('t'), KeyModifiers::CONTROL, &event_sender);

        // Should have 3 results again
        wait_for_match(&mut snapshot_rx, Pattern::string("Hidden files: ON"), 100).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Results: 3"), 1000).await?;

        // Perform the replacement
        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        // Verify all files were replaced (including hidden ones)
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

test_with_both_regex_modes!(
    test_toggle_multiline_keybinding,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "first line",
                "second line",
                "third line",
            ),
            "file2.txt" => text!(
                "other content",
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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Search for pattern that spans lines - multiline is off by default, so no results
        send_chars(r"first.*\nsecond", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(
            &mut snapshot_rx,
            Pattern::regex_must_compile("Results: 0.*Search complete"),
            1000,
        )
        .await?;

        // Toggle multiline on
        send_key_with_modifiers(KeyCode::Char('m'), KeyModifiers::ALT, &event_sender);

        // Wait for toast and re-search to complete - should now have 1 result
        wait_for_match(&mut snapshot_rx, Pattern::string("Multiline: ON"), 100).await?;
        wait_for_match(
            &mut snapshot_rx,
            Pattern::regex_must_compile("Results: 1.*Search complete"),
            1000,
        )
        .await?;

        // Toggle multiline off again
        send_key_with_modifiers(KeyCode::Char('m'), KeyModifiers::ALT, &event_sender);

        // Should go back to 0 results - verify toast and search completion
        wait_for_match(&mut snapshot_rx, Pattern::string("Multiline: OFF"), 1000).await?;
        wait_for_match(
            &mut snapshot_rx,
            Pattern::regex_must_compile("Results: 0.*Search complete"),
            1000,
        )
        .await?;

        // Toggle multiline back on for the replacement
        send_key_with_modifiers(KeyCode::Char('m'), KeyModifiers::ALT, &event_sender);

        // Should have 1 result again
        wait_for_match(&mut snapshot_rx, Pattern::string("Multiline: ON"), 100).await?;
        wait_for_match(
            &mut snapshot_rx,
            Pattern::regex_must_compile("Results: 1.*Search complete"),
            1000,
        )
        .await?;

        // Perform the replacement
        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        // Verify file was replaced
        // Pattern "first.*\nsecond" matches "first line\nsecond", leaving " line" after REPLACED
        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "REPLACED line",
                "third line",
            ),
            "file2.txt" => text!(
                "other content",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_toggle_escape_sequences_keybinding,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "line one",
                "line two",
            )
        );

        // Create config with a keybinding for toggle_interpret_escape_sequences
        let mut keys_config = KeysConfig::default();
        keys_config.search.toggle_interpret_escape_sequences = keys![CoreKeyEvent::new(
            CoreKeyCode::Char('e'),
            CoreKeyModifiers::ALT
        )];

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let user_config = Config {
            keys: keys_config,
            ..Config::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_custom_config(config, user_config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Search for "one" and replace with "1\n2" - escape sequences off by default
        send_chars("one", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"1\n2", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(
            &mut snapshot_rx,
            Pattern::regex_must_compile("Results: 1.*Search complete"),
            1000,
        )
        .await?;

        // Perform replacement with escape sequences OFF - should get literal \n
        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        // Verify file has literal \n (4 characters: 1, \, n, 2)
        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                r"line 1\n2",
                "line two",
            )
        );

        // Reset and try again with escape sequences ON
        send_key_with_modifiers(KeyCode::Char('r'), KeyModifiers::CONTROL, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Toggle escape sequences ON
        send_key_with_modifiers(KeyCode::Char('e'), KeyModifiers::ALT, &event_sender);
        wait_for_match(
            &mut snapshot_rx,
            Pattern::string("Escape sequences: ON"),
            100,
        )
        .await?;

        // Search and replace - now \n should become actual newline
        send_chars(r"1\\n2", &event_sender); // Search for the literal \n we just inserted
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"X\nY", &event_sender); // Replace with X<newline>Y
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(
            &mut snapshot_rx,
            Pattern::regex_must_compile("Results: 1.*Search complete"),
            1000,
        )
        .await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        // Verify file now has actual newline
        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "line X",
                "Y",
                "line two",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_escape_sequences_with_config_enabled,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "hello world blah",
                "some more text",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                interpret_escape_sequences: true, // Enabled from start
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace "world" with "there\nfriend"
        send_chars("world", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"there\nfriend", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(
            &mut snapshot_rx,
            Pattern::regex_must_compile("Results: 1.*Search complete"),
            1000,
        )
        .await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        // Verify \n was interpreted as newline
        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "hello there",
                "friend blah",
                "some more text",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

// Tests for escape sequence and multiline combinations - 4 variants
// These tests verify the preview rendering for replacements containing \n

test_with_both_regex_modes!(
    test_preview_escape_off_multiline_off,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "hello world",
                "another line",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: false,
                interpret_escape_sequences: false,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace "world" with "foo\nbar" (literal, not interpreted)
        send_chars("world", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"foo\nbar", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("preview_escape_off_multiline_off", snapshot);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_preview_escape_on_multiline_off,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "hello world",
                "another line",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: false,
                interpret_escape_sequences: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace "world" with "foo\nbar" (interpreted as newline)
        send_chars("world", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"foo\nbar", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("preview_escape_on_multiline_off", snapshot);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_preview_escape_off_multiline_on,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "hello world",
                "another line",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                interpret_escape_sequences: false,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace "world" with "foo\nbar" (literal, not interpreted)
        send_chars("world", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"foo\nbar", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("preview_escape_off_multiline_on", snapshot);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_preview_escape_on_multiline_on,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "hello world",
                "another line",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                interpret_escape_sequences: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace "world" with "foo\nbar" (interpreted as newline)
        send_chars("world", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"foo\nbar", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("preview_escape_on_multiline_on", snapshot);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_ignores_git_folders_by_default,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a text file",
            ),
            ".git/config" => text!(
                "This is a git config file",
            ),
            ".git/objects/pack/packfile" => text!(
                "This is a git object file",
            ),
            "submodule/.git/config" => text!(
                "This is a nested git config",
            ),
            // .git as a file (used in worktrees)
            "worktree/.git" => text!(
                "gitdir: /path/to/main/.git/worktrees/this",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                include_hidden: true, // Include hidden to ensure .git exclusion is separate
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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
            ".git/config" => text!(
                "This is a git config file",
            ),
            ".git/objects/pack/packfile" => text!(
                "This is a git object file",
            ),
            "submodule/.git/config" => text!(
                "This is a nested git config",
            ),
            "worktree/.git" => text!(
                "gitdir: /path/to/main/.git/worktrees/this",
            )
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_includes_git_folders_with_flag,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "dir1/file1.txt" => text!(
                "This is a text file",
            ),
            ".git/config" => text!(
                "This is a git config file",
            ),
            ".git/objects/pack/packfile" => text!(
                "This is a git object file",
            ),
            "submodule/.git/config" => text!(
                "This is a nested git config",
            ),
            // .git as a file (used in worktrees)
            "worktree/.git" => text!(
                "gitdir: /path/to/main/.git/worktrees/this",
            )
        );

        let config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                include_hidden: true,
                include_git_folders: true,
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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
            ".git/config" => text!(
                "This REPLACED a git config file",
            ),
            ".git/objects/pack/packfile" => text!(
                "This REPLACED a git object file",
            ),
            "submodule/.git/config" => text!(
                "This REPLACED a nested git config",
            ),
            "worktree/.git" => text!(
                "gitdir: /path/to/main/.git/worktrees/this",
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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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
                "some very long lines of text. This is really quite a long line. Lots of text which won't fit on one line so it will initially be truncated, but users can toggle text wrapping so that it all shows up on the screen.",
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
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) = build_test_runner_with_config(config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

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
                "some very long lines of text. This is really quite a long line. Lots of text which won't fit on one line so it will initially be truncated, but REPLACED can toggle text wrapping so that it all shows up on the screen.",
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

#[tokio::test]
#[serial]
async fn test_custom_help_menu_keybinding() -> anyhow::Result<()> {
    use scooter_core::config::{Config, KeysConfig, KeysGeneral};
    use scooter_core::keyboard::{
        KeyCode as CoreKeyCode, KeyEvent, KeyModifiers as CoreKeyModifiers,
    };
    use scooter_core::keys;

    // Change help menu from C-h to F1
    let mut config = Config::default();
    config.keys = KeysConfig {
        general: KeysGeneral {
            show_help_menu: keys![KeyEvent::new(CoreKeyCode::F(1), CoreKeyModifiers::NONE)],
            ..Default::default()
        },
        ..Default::default()
    };

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner_with_custom_config(AppConfig::default(), config)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

    // F1 should open help
    send_key(KeyCode::F(1), &event_sender);
    let snapshot = wait_for_match(&mut snapshot_rx, Pattern::string("Help"), 100).await?;
    assert_snapshot!(snapshot);

    shutdown(event_sender, run_handle).await
}

test_with_both_regex_modes!(
    test_custom_toggle_keybinding,
    |advanced_regex: bool| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "foo1",
                "foo2",
                "foo3",
                "foo4",
                "bar",
            ),
        );

        // Change toggle from space to 't'
        let mut config = Config::default();
        config.keys = KeysConfig {
            search: KeysSearch {
                fields: KeysSearchFocusFields {
                    focus_next_field: keys![CoreKeyEvent::new(
                        CoreKeyCode::Char('j'),
                        CoreKeyModifiers::CONTROL,
                    )],
                    ..Default::default()
                },
                results: KeysSearchFocusResults {
                    move_down: keys![CoreKeyEvent::new(
                        CoreKeyCode::Char('d'),
                        CoreKeyModifiers::NONE,
                    )],
                    toggle_selected_inclusion: keys![CoreKeyEvent::new(
                        CoreKeyCode::Char('t'),
                        CoreKeyModifiers::NONE,
                    )],
                    toggle_all_selected: keys![CoreKeyEvent::new(
                        CoreKeyCode::Char('x'),
                        CoreKeyModifiers::NONE,
                    )],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let app_config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_custom_config(app_config, config)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        send_chars("foo", &event_sender);
        send_key_with_modifiers(KeyCode::Char('j'), KeyModifiers::CONTROL, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Toggle all off
        send_key(KeyCode::Char('x'), &event_sender);

        // Down to third result
        send_key(KeyCode::Char('d'), &event_sender);
        send_key(KeyCode::Char('d'), &event_sender);

        // Toggle third result
        send_key(KeyCode::Char('t'), &event_sender);

        // Perform replacement - should not replace the deselected line
        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "foo1",
                "foo2",
                "REPLACED3",
                "foo4",
                "bar",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(test_multiline_search_preview, |advanced_regex| async move {
    let temp_dir = create_test_files!(
        "file1.txt" => text!(
            "line 1",
            "foo bar",
            "baz qux",
            "line 4",
            "foo bar",
            "baz qux",
            "line 7",
        ),
    );

    let app_config = AppConfig {
        directory: temp_dir.path().to_path_buf(),
        app_run_config: AppRunConfig {
            advanced_regex,
            multiline: true,
            ..AppRunConfig::default()
        },
        ..AppConfig::default()
    };

    let (run_handle, event_sender, mut snapshot_rx) =
        build_test_runner_with_config_and_width(app_config, 50)?;

    wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

    // Search for multiline pattern "foo bar\nbaz"
    send_chars(r"foo bar\nbaz", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("REPLACED", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
    let snapshot =
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
    assert_snapshot_with_filters("multiline_search_preview_result_1", snapshot);

    // Navigate down to second result
    send_key(KeyCode::Down, &event_sender);
    let snapshot = get_snapshot_after_wait(&mut snapshot_rx, 200).await?;
    assert_snapshot_with_filters("multiline_search_preview_result_2", snapshot);

    // Perform the replacement
    send_key(KeyCode::Enter, &event_sender);
    wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

    assert_test_files!(
        temp_dir,
        "file1.txt" => text!(
            "line 1",
            "REPLACED qux",
            "line 4",
            "REPLACED qux",
            "line 7",
        ),
    );

    shutdown(event_sender, run_handle).await
});

test_with_both_regex_modes!(
    test_multiline_search_single_line_match,
    |advanced_regex| async move {
        // Test multiline mode with matches that only span a single line
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "hello world",
                "foo bar baz",
                "test line",
            ),
        );

        let app_config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(app_config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Search for single-line pattern in multiline mode
        send_chars("foo", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("multiline_single_line_match_preview", snapshot);

        // Perform the replacement
        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "hello world",
                "REPLACED bar baz",
                "test line",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_multiline_search_three_line_match,
    |advanced_regex| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "line 1",
                "start match",
                "middle line",
                "end match",
                "line 5",
            ),
        );

        let app_config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(app_config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Search for pattern spanning 3 lines
        send_chars(r"start match\nmiddle line\nend match", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("SINGLE LINE", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("multiline_three_line_match_preview", snapshot);

        // Perform the replacement
        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "line 1",
                "SINGLE LINE",
                "line 5",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_multiline_match_at_file_start,
    |advanced_regex| async move {
        // Test multiline match starting at line 1 (no context before)
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "first line",
                "second line",
                "third line",
            ),
        );

        let app_config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(app_config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Search for pattern at file start
        send_chars(r"first line\nsecond", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "REPLACED line",
                "third line",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_multiline_match_at_file_end,
    |advanced_regex| async move {
        // Test multiline match ending at last line (no context after)
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "first line",
                "second line",
                "third line",
            ),
        );

        let app_config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(app_config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Search for pattern at file end
        send_chars(r"second line\nthird line", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "first line",
                "REPLACED",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_multiline_match_with_empty_line,
    |advanced_regex| async move {
        // Test multiline match containing an empty line
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "before",
                "start",
                "",
                "end",
                "after",
            ),
        );

        let app_config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(app_config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Search for pattern spanning empty line
        send_chars(r"start\n\nend", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "before",
                "REPLACED",
                "after",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_multiline_replacement_collapses_lines,
    |advanced_regex| async move {
        // Test that multiline match can be collapsed to shorter text
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "before",
                "line 1 here",
                "line 2 here",
                "after",
            ),
        );

        let app_config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(app_config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace multiline pattern with shorter text (keeps "here" suffix)
        send_chars(r"line 1 here\nline 2", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("MERGED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "before",
                "MERGED here",
                "after",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_multiline_replacement_to_single_line,
    |advanced_regex| async move {
        // Test that 3-line match can be replaced with single line
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "before",
                "start",
                "middle",
                "end",
                "after",
            ),
        );

        let app_config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(app_config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace 3 full lines with single word
        send_chars(r"start\nmiddle\nend", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("CONDENSED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;

        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "before",
                "CONDENSED",
                "after",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_stdin_multiline_search_and_replace,
    |advanced_regex| async move {
        // Test that multiline search works with stdin input
        let stdin_content = "foo bar\nbaz blah\nqux\n".to_string();

        let app_config = AppConfig {
            directory: std::env::temp_dir(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                print_results: true,
                ..AppRunConfig::default()
            },
            stdin_content: Some(stdin_content),
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(app_config, 80)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Search for multiline pattern that spans 2 lines
        send_chars(r"foo.*\n.*z", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;

        // Perform the replacement - for stdin mode, the app exits after replacement
        // without showing "Success!" screen, so we just wait for the app to complete
        send_key(KeyCode::Enter, &event_sender);
        wait_for_match(
            &mut snapshot_rx,
            Pattern::string("Performing replacement"),
            1000,
        )
        .await?;

        // Wait for the app to finish (it exits after stdin replacement)
        let timeout_res = tokio::time::timeout(Duration::from_secs(2), async {
            run_handle.await.unwrap();
        })
        .await;
        assert!(
            timeout_res.is_ok(),
            "App didn't complete in a reasonable time"
        );

        Ok(())
    }
);

// Stdin preview tests - verify preview rendering for stdin input with escape/multiline combinations

test_with_both_regex_modes!(
    test_stdin_preview_escape_off_multiline_off,
    |advanced_regex: bool| async move {
        let stdin_content = "hello world\nanother line\n".to_string();

        let config = AppConfig {
            directory: std::env::temp_dir(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: false,
                interpret_escape_sequences: false,
                ..AppRunConfig::default()
            },
            stdin_content: Some(stdin_content),
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace "world" with "foo\nbar" (literal, not interpreted)
        send_chars("world", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"foo\nbar", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("stdin_preview_escape_off_multiline_off", snapshot);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_stdin_preview_escape_on_multiline_off,
    |advanced_regex: bool| async move {
        let stdin_content = "hello world\nanother line\n".to_string();

        let config = AppConfig {
            directory: std::env::temp_dir(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: false,
                interpret_escape_sequences: true,
                ..AppRunConfig::default()
            },
            stdin_content: Some(stdin_content),
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace "world" with "foo\nbar" (interpreted as newline)
        send_chars("world", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"foo\nbar", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("stdin_preview_escape_on_multiline_off", snapshot);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_stdin_preview_escape_off_multiline_on,
    |advanced_regex: bool| async move {
        let stdin_content = "hello world\nanother line\n".to_string();

        let config = AppConfig {
            directory: std::env::temp_dir(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                interpret_escape_sequences: false,
                ..AppRunConfig::default()
            },
            stdin_content: Some(stdin_content),
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace "world" with "foo\nbar" (literal, not interpreted)
        send_chars("world", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"foo\nbar", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("stdin_preview_escape_off_multiline_on", snapshot);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_stdin_preview_escape_on_multiline_on,
    |advanced_regex: bool| async move {
        let stdin_content = "hello world\nanother line\n".to_string();

        let config = AppConfig {
            directory: std::env::temp_dir(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                interpret_escape_sequences: true,
                ..AppRunConfig::default()
            },
            stdin_content: Some(stdin_content),
            ..AppConfig::default()
        };
        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        // Replace "world" with "foo\nbar" (interpreted as newline)
        send_chars("world", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars(r"foo\nbar", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        let snapshot =
            wait_for_match(&mut snapshot_rx, Pattern::string("Search complete"), 1000).await?;
        assert_snapshot_with_filters("stdin_preview_escape_on_multiline_on", snapshot);

        shutdown(event_sender, run_handle).await
    }
);

test_with_both_regex_modes!(
    test_multiline_replacement_results_screen,
    |advanced_regex| async move {
        let temp_dir = create_test_files!(
            "file1.txt" => text!(
                "start one",
                "end one",
                "start two",
                "end two",
                "other content",
            ),
        );

        let app_config = AppConfig {
            directory: temp_dir.path().to_path_buf(),
            app_run_config: AppRunConfig {
                advanced_regex,
                multiline: true,
                ..AppRunConfig::default()
            },
            ..AppConfig::default()
        };

        let (run_handle, event_sender, mut snapshot_rx) =
            build_test_runner_with_config_and_width(app_config, 50)?;

        wait_for_match(&mut snapshot_rx, Pattern::string("Search text"), 100).await?;

        send_chars(r"start.*\nend", &event_sender);
        send_key(KeyCode::Tab, &event_sender);
        send_chars("REPLACED", &event_sender);
        send_key(KeyCode::Enter, &event_sender);

        wait_for_match(&mut snapshot_rx, Pattern::string("Still searching"), 1000).await?;
        wait_for_match(
            &mut snapshot_rx,
            Pattern::regex_must_compile("Results: 2.*Search complete"),
            1000,
        )
        .await?;

        // Perform replacement
        send_key(KeyCode::Enter, &event_sender);
        let snapshot = wait_for_match(&mut snapshot_rx, Pattern::string("Success!"), 2000).await?;
        assert_snapshot_with_filters("multiline_replacement_results_screen", snapshot);

        // Verify file was replaced correctly
        assert_test_files!(
            temp_dir,
            "file1.txt" => text!(
                "REPLACED one",
                "REPLACED two",
                "other content",
            ),
        );

        shutdown(event_sender, run_handle).await
    }
);
