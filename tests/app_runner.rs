use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use futures::Stream;
use ratatui::backend::TestBackend;
use scooter::app_runner::{AppConfig, AppRunner, EventStream};
use std::{pin::Pin, task::Poll};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

struct TestEventStream {
    receiver: UnboundedReceiver<CrosstermEvent>,
}

impl TestEventStream {
    fn new() -> (UnboundedSender<CrosstermEvent>, Self) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (sender, Self { receiver })
    }
}

impl Stream for TestEventStream {
    type Item = Result<CrosstermEvent, std::io::Error>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx).map(|opt| opt.map(Result::Ok))
    }
}

impl EventStream for TestEventStream {}

#[tokio::test]
async fn test_app_runner_todo() -> anyhow::Result<()> {
    let backend = TestBackend::new(80, 24);
    let config = AppConfig {
        directory: None,
        hidden: false,
        advanced_regex: false,
        log_level: log::LevelFilter::Info,
    };

    let (event_sender, event_stream) = TestEventStream::new();
    let (snapshot_tx, mut snapshot_rx) = mpsc::unbounded_channel();
    let mut runner =
        AppRunner::new(config, backend, event_stream)?.with_snapshot_channel(snapshot_tx);

    runner.init()?;

    let run_handle = tokio::spawn(async move { runner.run_event_loop().await });

    let contents = snapshot_rx.recv().await.unwrap();
    assert!(contents.contains("Search text"));

    event_sender.send(CrosstermEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::empty(),
    )))?;

    let contents = snapshot_rx.recv().await.unwrap();
    assert!(!contents.contains("Search text"));

    run_handle.await??;

    Ok(())
}
