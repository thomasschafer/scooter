use anyhow::bail;
use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use futures::Stream;
use log::LevelFilter;
use ratatui::backend::TestBackend;
use scooter::app_runner::{AppConfig, AppRunner};
use std::{io, pin::Pin, task::Poll};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
    time::{sleep, Duration, Instant},
};

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
) -> anyhow::Result<()> {
    let timeout = Duration::from_millis(timeout_ms);

    let start = Instant::now();
    let mut last_snapshot = None;
    loop {
        if let Ok(snapshot) = snapshot_rx.try_recv() {
            if snapshot.contains(text) {
                return Ok(());
            }
            last_snapshot = Some(snapshot);
        };

        if start.elapsed() > timeout {
            let formatted_snapshot = match last_snapshot {
                Some(snapshot) => &format!("Current buffer snapshot:\n{snapshot}"),
                None => "No buffer snapshots recieved",
            };
            bail!("Timeout waiting for text: {text}\n{formatted_snapshot}");
        }

        sleep(Duration::from_millis(5)).await;
    }
}

type TestRunner = (
    AppRunner<TestBackend, TestEventStream>,
    UnboundedSender<CrosstermEvent>,
    UnboundedReceiver<String>,
);

fn build_test_runner() -> anyhow::Result<TestRunner> {
    let backend = TestBackend::new(80, 24);
    let config = AppConfig {
        directory: None,
        hidden: false,
        advanced_regex: false,
        log_level: LevelFilter::Warn,
    };

    let (event_sender, event_stream) = TestEventStream::new();
    let (snapshot_tx, snapshot_rx) = mpsc::unbounded_channel();
    let runner = AppRunner::new(config, backend, event_stream)?.with_snapshot_channel(snapshot_tx);

    Ok((runner, event_sender, snapshot_rx))
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

#[tokio::test]
async fn test_basic_search() -> anyhow::Result<()> {
    let (mut runner, event_sender, mut snapshot_rx) = build_test_runner()?;
    runner.init()?;

    let run_handle = tokio::spawn(async move {
        runner.run_event_loop().await.unwrap();
    });

    wait_for_text(&mut snapshot_rx, "Search text", 10).await?;

    event_sender.send(CrosstermEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    )))?;

    wait_for_text(&mut snapshot_rx, "Still searching", 100).await?;

    wait_for_text(&mut snapshot_rx, "Search complete", 1000).await?;

    shutdown(event_sender, run_handle).await
}
