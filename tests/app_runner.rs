use anyhow::bail;
use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use futures::Stream;
use log::LevelFilter;
use ratatui::backend::TestBackend;
use scooter::app_runner::{AppConfig, AppRunner};
use std::{io, path::Path, pin::Pin, task::Poll};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time::{sleep, Duration, Instant},
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

async fn wait_for_text(
    snapshot_rx: &mut UnboundedReceiver<String>,
    text: &str,
    timeout_ms: u64,
) -> anyhow::Result<String> {
    let timeout = Duration::from_millis(timeout_ms);
    let start = Instant::now();
    let mut last_snapshot = None;

    while start.elapsed() <= timeout {
        tokio::select! {
            snapshot = snapshot_rx.recv() => {
                match snapshot {
                    Some(s) if s.contains(text) => return Ok(s),
                    Some(s) => { last_snapshot = Some(s); },
                    None => bail!("Channel closed while waiting for text: {text}"),
                }
            }
            _ = sleep(timeout - start.elapsed()) => {
                break;
            }
        }
    }

    let formatted_snapshot = match last_snapshot {
        Some(snapshot) => &format!("Current buffer snapshot:\n{snapshot}"),
        None => "No buffer snapshots recieved",
    };
    bail!("Timeout waiting for text: {text}\n{formatted_snapshot}")
}

type TestRunner = (
    JoinHandle<()>,
    UnboundedSender<CrosstermEvent>,
    UnboundedReceiver<String>,
);

fn build_test_runner(directory: Option<&Path>) -> anyhow::Result<TestRunner> {
    let backend = TestBackend::new(80, 24);
    let config = AppConfig {
        directory: directory.map(|d| d.to_str().unwrap().to_owned()),
        hidden: false,
        advanced_regex: false,
        log_level: LevelFilter::Warn,
    };

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

fn send_key(key: KeyCode, event_sender: &UnboundedSender<CrosstermEvent>) {
    event_sender
        .send(CrosstermEvent::Key(KeyEvent::new(
            key,
            KeyModifiers::empty(),
        )))
        .unwrap();
}

fn send_chars(word: &str, event_sender: &UnboundedSender<CrosstermEvent>) {
    word.chars()
        .for_each(|key| send_key(KeyCode::Char(key), event_sender));
}

#[tokio::test]
async fn test_search_current_dir() -> anyhow::Result<()> {
    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(None)?;

    wait_for_text(&mut snapshot_rx, "Search text", 10).await?;

    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, "Still searching", 500).await?;

    wait_for_text(&mut snapshot_rx, "Search complete", 1000).await?;

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_search_and_replace_simple_dir() -> anyhow::Result<()> {
    let temp_dir = &create_test_files! {
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
    };

    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(Some(temp_dir.path()))?;

    wait_for_text(&mut snapshot_rx, "Search text", 10).await?;

    send_chars("before", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("after", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, "Still searching", 500).await?;

    wait_for_text(&mut snapshot_rx, "Search complete", 1000).await?;

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

    wait_for_text(&mut snapshot_rx, "Success!", 1000).await?;

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

#[tokio::test]
async fn test_search_and_replace_no_matches() -> anyhow::Result<()> {
    let temp_dir = &create_test_files! {
        "dir1/file1.txt" => {
            "This is some test content 123",
        },
    };

    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(Some(temp_dir.path()))?;

    wait_for_text(&mut snapshot_rx, "Search text", 10).await?;

    send_chars("before", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("after", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, "Still searching", 500).await?;

    wait_for_text(&mut snapshot_rx, "Search complete", 1000).await?;

    // Nothing should have changed yet
    assert_test_files!(
        &temp_dir,
        "dir1/file1.txt" => {
            "This is some test content 123",
        },
    );

    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, "Success!", 1000).await?;

    // Verify that nothing has changed
    assert_test_files!(
        &temp_dir,
        "dir1/file1.txt" => {
            "This is some test content 123",
        },
    );

    shutdown(event_sender, run_handle).await
}

#[tokio::test]
async fn test_search_and_replace_empty_dir() -> anyhow::Result<()> {
    let temp_dir = &create_test_files! {};

    let (run_handle, event_sender, mut snapshot_rx) = build_test_runner(Some(temp_dir.path()))?;

    wait_for_text(&mut snapshot_rx, "Search text", 10).await?;

    send_chars("before", &event_sender);
    send_key(KeyCode::Tab, &event_sender);
    send_chars("after", &event_sender);
    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, "Still searching", 500).await?;

    wait_for_text(&mut snapshot_rx, "Search complete", 1000).await?;

    assert_test_files!(&temp_dir);

    send_key(KeyCode::Enter, &event_sender);

    wait_for_text(&mut snapshot_rx, "Success!", 1000).await?;

    assert_test_files!(&temp_dir);

    shutdown(event_sender, run_handle).await
}

// TODO: add tests for whole word matching, using fixed strings, standard + fancy regex
