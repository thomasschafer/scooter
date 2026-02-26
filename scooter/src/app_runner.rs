use anyhow::Context;
use crossterm::{
    event::{self, Event as CrosstermEvent},
    style::Stylize as _,
};
use futures::{Stream, StreamExt};
use log::{LevelFilter, error};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend, TestBackend},
    crossterm::event::KeyEventKind,
};
use scooter_core::{
    app::{
        App, AppRunConfig, Event, EventHandlingResult, ExitAndReplaceState, ExitState, InputSource,
    },
    config::{self, Config},
    errors::AppError,
    fields::SearchFieldValues,
    keyboard::KeyEvent,
    replace::ReplaceState,
};
use scooter_core::{replace::ReplaceResult, search::SearchResultWithReplacement};
use std::{
    collections::HashMap,
    env,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    sync::Arc,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{logging::DEFAULT_LOG_LEVEL, tui::Tui};

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppConfig<'a> {
    pub directory: PathBuf,
    pub log_level: LevelFilter,
    pub search_field_values: SearchFieldValues<'a>,
    pub app_run_config: AppRunConfig,
    pub stdin_content: Option<String>,
    pub editor_command_override: Option<String>,
}

impl Default for AppConfig<'_> {
    fn default() -> Self {
        Self {
            directory: env::current_dir().unwrap(),
            log_level: LevelFilter::from_str(DEFAULT_LOG_LEVEL).unwrap(),
            search_field_values: SearchFieldValues::default(),
            app_run_config: AppRunConfig::default(),
            stdin_content: None,
            editor_command_override: None,
        }
    }
}

pub trait EventStream:
    Stream<Item = Result<CrosstermEvent, std::io::Error>> + Send + Unpin
{
}
impl<T: Stream<Item = Result<CrosstermEvent, std::io::Error>> + Send + Unpin> EventStream for T {}

pub type CrosstermEventStream = event::EventStream;

pub struct AppRunner<B: Backend, E: EventStream, S: SnapshotProvider<B>> {
    app: App,
    tui: Tui<B>,
    event_stream: E,
    snapshot_provider: S,
}

pub trait SnapshotProvider<B: Backend> {
    fn send_snapshot(&self, tui: &Tui<B>);
}

pub struct NoOpSnapshotProvider;

impl<B: Backend> SnapshotProvider<B> for NoOpSnapshotProvider {
    #[inline]
    fn send_snapshot(&self, _tui: &Tui<B>) {
        // No-op - optimized away in release builds
    }
}

pub struct TestSnapshotProvider {
    sender: UnboundedSender<String>,
}

// Used in integration tests
#[allow(dead_code)]
impl TestSnapshotProvider {
    pub fn new(sender: UnboundedSender<String>) -> Self {
        Self { sender }
    }
}

impl SnapshotProvider<TestBackend> for TestSnapshotProvider {
    fn send_snapshot(&self, tui: &Tui<TestBackend>) {
        let buffer = tui.terminal.backend().buffer();
        let contents = buffer
            .content
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                if i % buffer.area.width as usize == 0 && i > 0 {
                    "\n" // TODO: should this be `cell.symbol() + "\n"`?
                } else {
                    cell.symbol()
                }
            })
            .collect::<Vec<_>>()
            .join("");
        let _ = self.sender.send(contents);
    }
}

impl AppRunner<CrosstermBackend<io::Stdout>, CrosstermEventStream, NoOpSnapshotProvider> {
    pub fn new_runner(app_config: AppConfig<'_>) -> anyhow::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let event_stream = CrosstermEventStream::new();
        let snapshot_provider = NoOpSnapshotProvider;
        let mut user_config = config::load_config().context("Failed to read config file")?;

        // Apply CLI override for editor command if provided
        if let Some(ref editor_command) = app_config.editor_command_override {
            user_config.editor_open.command = Some(editor_command.clone());
        }

        Self::new(
            app_config,
            user_config,
            backend,
            event_stream,
            snapshot_provider,
        )
    }
}

impl<E: EventStream> AppRunner<TestBackend, E, TestSnapshotProvider> {
    // Used in integration tests
    #[allow(dead_code)]
    pub fn new_snapshot_test(
        app_config: AppConfig<'_>,
        backend: TestBackend,
        event_stream: E,
        snapshot_sender: UnboundedSender<String>,
    ) -> anyhow::Result<Self> {
        // Tests should use default config, not load from user's config directory
        let test_config = Config::default();
        Self::new_snapshot_test_override_config(
            app_config,
            backend,
            event_stream,
            snapshot_sender,
            test_config,
        )
    }

    // Used in integration tests
    #[allow(dead_code)]
    pub fn new_snapshot_test_override_config(
        app_config: AppConfig<'_>,
        backend: TestBackend,
        event_stream: E,
        snapshot_sender: UnboundedSender<String>,
        config: Config,
    ) -> anyhow::Result<Self> {
        let snapshot_provider = TestSnapshotProvider::new(snapshot_sender);
        Self::new(app_config, config, backend, event_stream, snapshot_provider)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuoteContext {
    None,
    Single,
    Double,
}

fn escape_for_single_quotes(path: &str) -> String {
    if cfg!(windows) {
        path.to_string()
    } else {
        path.replace('\'', "'\\''")
    }
}

fn escape_for_double_quotes(path: &str) -> String {
    if cfg!(windows) {
        path.to_string()
    } else {
        let mut escaped = String::with_capacity(path.len());
        for ch in path.chars() {
            match ch {
                '\\' | '"' | '$' | '`' => {
                    escaped.push('\\');
                    escaped.push(ch);
                }
                _ => escaped.push(ch),
            }
        }
        escaped
    }
}

fn quote_path_unquoted(path: &str) -> String {
    if cfg!(windows) {
        format!("\"{path}\"")
    } else {
        format!("'{}'", escape_for_single_quotes(path))
    }
}

fn build_editor_command(editor_command: &str, file_path: &Path, line: usize) -> String {
    let file_str = file_path.to_string_lossy();
    let line_str = line.to_string();
    let mut output = String::with_capacity(editor_command.len() + file_str.len());
    let mut context = QuoteContext::None;
    let mut escape_next = false;
    let mut idx = 0;

    while idx < editor_command.len() {
        let rest = &editor_command[idx..];
        if rest.starts_with("%file") {
            let replacement = match context {
                QuoteContext::None => quote_path_unquoted(file_str.as_ref()),
                QuoteContext::Single => escape_for_single_quotes(file_str.as_ref()),
                QuoteContext::Double => escape_for_double_quotes(file_str.as_ref()),
            };
            output.push_str(&replacement);
            idx += "%file".len();
            continue;
        }
        if rest.starts_with("%line") {
            output.push_str(&line_str);
            idx += "%line".len();
            continue;
        }

        let ch = rest.chars().next().expect("non-empty slice");
        output.push(ch);

        if escape_next {
            escape_next = false;
        } else {
            match context {
                QuoteContext::None => match ch {
                    '\'' => context = QuoteContext::Single,
                    '"' => context = QuoteContext::Double,
                    '\\' if !cfg!(windows) => escape_next = true,
                    _ => {}
                },
                QuoteContext::Single => {
                    if ch == '\'' {
                        context = QuoteContext::None;
                    }
                }
                QuoteContext::Double => match ch {
                    '"' => context = QuoteContext::None,
                    '\\' if !cfg!(windows) => escape_next = true,
                    _ => {}
                },
            }
        }

        idx += ch.len_utf8();
    }

    output
}

impl<B: Backend + 'static, E: EventStream, S: SnapshotProvider<B>> AppRunner<B, E, S>
where
    B::Error: Send + Sync,
{
    pub fn new(
        app_config: AppConfig<'_>,
        config: Config,
        backend: B,
        event_stream: E,
        snapshot_provider: S,
    ) -> anyhow::Result<Self> {
        let input_source = if let Some(stdin_content) = app_config.stdin_content {
            InputSource::Stdin(Arc::new(stdin_content))
        } else {
            InputSource::Directory(app_config.directory.clone())
        };

        let app = App::new(
            input_source,
            &app_config.search_field_values,
            app_config.app_run_config,
            config,
        )?;

        let terminal = Terminal::new(backend)?;
        let tui = Tui::new(terminal);

        Ok(Self {
            app,
            tui,
            event_stream,
            snapshot_provider,
        })
    }

    pub fn init(&mut self) -> anyhow::Result<()> {
        self.tui.init()?;
        self.draw()?;

        Ok(())
    }

    pub fn draw(&mut self) -> anyhow::Result<()> {
        self.tui.draw(&mut self.app)?;
        self.snapshot_provider.send_snapshot(&self.tui);
        Ok(())
    }

    pub async fn run_event_loop(&mut self) -> anyhow::Result<Option<ExitState>> {
        loop {
            let event_handling_result = tokio::select! {
                Some(Ok(event)) = self.event_stream.next() => {
                    match event {
                        CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                            let mut key_event: KeyEvent = key.into();
                            key_event.canonicalize();
                            self.app.handle_key_event(key_event)
                        },
                        CrosstermEvent::Resize(_, _) => EventHandlingResult::Rerender,
                        _ => EventHandlingResult::None,
                    }
                }
                event = self.app.event_recv() => {
                    match event {
                        Event::LaunchEditor((file_path, line)) => {
                            let mut res = EventHandlingResult::Rerender;
                            self.tui.show_cursor()?;
                            match self.open_editor(file_path, line) {
                                Ok(()) => {
                                    if self.app.config.editor_open.exit {
                                        res = EventHandlingResult::Exit(None);
                                    }
                                }
                                Err(e) => {
                                    self.app.add_error(
                                        AppError{
                                            name: "Failed to launch editor".to_string(),
                                            long: e.to_string(),
                                        },
                                    );
                                    error!("Failed to open editor: {e}");
                                }
                            }
                            self.tui.init()?;
                            res
                        }
                        Event::ExitAndReplace(state) => {
                            return Ok(Some(ExitState::StdinState(state)));
                        }
                        Event::Rerender => {
                            EventHandlingResult::Rerender
                        }
                        Event::Internal(internal_event) => {
                            self.app.handle_internal_event(internal_event)
                        }
                    }
                }
                else => {
                    return Ok(None);
                }
            };

            match event_handling_result {
                EventHandlingResult::Rerender => self.draw()?,
                EventHandlingResult::Exit(results) => return Ok(results.map(|t| *t)),
                EventHandlingResult::None => {}
            }
        }
    }

    pub fn cleanup(&mut self) -> anyhow::Result<()> {
        self.app.cancel_in_progress_tasks();
        self.tui.exit()
    }

    fn open_editor(&self, file_path: PathBuf, line: usize) -> anyhow::Result<()> {
        match &self.app.config.editor_open.command {
            Some(command) => {
                Self::open_editor_from_command(command, &file_path, line)?;
            }
            None => {
                Self::open_default_editor(file_path, line)?;
            }
        }
        Ok(())
    }

    fn open_editor_from_command(
        editor_command: &str,
        file_path: &Path,
        line: usize,
    ) -> anyhow::Result<()> {
        let editor_command = build_editor_command(editor_command, file_path, line);

        let output = {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                Command::new("cmd")
                    .arg("/C")
                    .raw_arg(&editor_command)
                    .output()?
            }
            #[cfg(not(windows))]
            {
                Command::new("sh").arg("-c").arg(&editor_command).output()?
            }
        };

        if output.status.success() {
            Ok(())
        } else {
            let status_code = output
                .status
                .code()
                .map_or("<not found>".to_owned(), |r| r.to_string());
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(anyhow::anyhow!(
                "Failed to execute command\nStatus: {status_code}\nOutput: {stderr}",
            ))
        }
    }

    fn open_default_editor(file_path: PathBuf, line: usize) -> anyhow::Result<()> {
        let editor = match env::var("EDITOR") {
            Ok(val) if !val.trim().is_empty() => val,
            _ => match env::var("VISUAL") {
                Ok(val) if !val.trim().is_empty() => val,
                _ => {
                    if cfg!(windows) {
                        "notepad".to_string()
                    } else {
                        "vi".to_string()
                    }
                }
            },
        };

        let parts: Vec<&str> = editor.split_whitespace().collect();
        let Some(program) = parts.first() else {
            return Err(anyhow::anyhow!("Found empty editor command"));
        };
        let mut cmd = Command::new(program);
        if parts.len() > 1 {
            cmd.args(&parts[1..]);
        }

        let editor_name = Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(program)
            .to_lowercase();
        match editor_name.as_str() {
            e if ["vi", "vim", "nvim", "kak", "nano"].contains(&e) => {
                cmd.arg(format!("+{line}")).arg(file_path);
            }
            e if ["hx", "helix", "subl", "sublime_text", "zed"].contains(&e) => {
                cmd.arg(format!("{}:{}", file_path.to_string_lossy(), line));
            }
            e if ["code", "code-insiders", "codium", "vscodium"].contains(&e) => {
                cmd.arg("-g")
                    .arg(format!("{}:{}", file_path.to_string_lossy(), line));
            }
            e if ["emacs", "emacsclient"].contains(&e) => {
                cmd.arg(format!("+{line}:0")).arg(file_path);
            }
            "notepad++" => {
                cmd.arg(file_path).arg(format!("-n{line}"));
            }
            _ => {
                cmd.arg(file_path);
            }
        }

        cmd.status()?;
        Ok(())
    }
}

pub fn format_replacement_results(
    num_successes: usize,
    num_ignored: Option<usize>,
    errors: Option<&[SearchResultWithReplacement]>,
) -> String {
    let errors_display = if let Some(errors) = errors {
        #[allow(clippy::format_collect)]
        errors
            .iter()
            .map(|error| {
                let (path, error) = error.display_error();
                format!("\n{path}:\n  {}", error.red())
            })
            .collect::<String>()
    } else {
        String::new()
    };

    let maybe_ignored_str = match num_ignored {
        Some(n) => format!("\nIgnored (lines): {n}"),
        None => "".into(),
    };
    let maybe_errors_str = match errors {
        Some(errors) => format!(
            "\nErrors: {num_errors}{errors_display}",
            num_errors = errors.len()
        ),
        None => "".into(),
    };

    format!(
        "Successful replacements (lines): {num_successes}{maybe_ignored_str}{maybe_errors_str}\n"
    )
}

pub async fn run_app_tui(app_config: AppConfig<'_>) -> anyhow::Result<Option<String>> {
    let mut runner = AppRunner::new_runner(app_config)?;
    runner.init()?;
    let mut exit_state = runner.run_event_loop().await?;
    runner.cleanup()?;

    let stats = match exit_state {
        Some(ExitState::Stats(stats)) => Some(stats),
        Some(ExitState::StdinState(ref mut state)) => {
            if runner.app.run_config.print_results {
                let res = write_results_to_stderr_with_stats(state)?;
                Some(res)
            } else {
                write_results_to_stderr(state)?;
                None
            }
        }
        None => {
            if runner.app.run_config.print_on_exit {
                match runner.app.input_source {
                    InputSource::Stdin(stdin) => write!(io::stderr(), "{stdin}")?,
                    InputSource::Directory(_) => unreachable!(),
                }
            }
            None
        }
    }
    .map(|stats| {
        format_replacement_results(
            stats.num_successes,
            Some(stats.num_ignored),
            Some(&stats.errors),
        )
    });
    Ok(stats)
}

fn write_results_to_stderr(state: &mut ExitAndReplaceState) -> anyhow::Result<()> {
    write_results_to_stderr_impl(state, false).map(|res| {
        assert!(res.is_none(), "Found Some stats, expected None");
    })
}

fn write_results_to_stderr_with_stats(
    state: &mut ExitAndReplaceState,
) -> anyhow::Result<ReplaceState> {
    write_results_to_stderr_impl(state, true)
        .map(|res| res.expect("Found None stats, expected Some"))
}

fn write_results_to_stderr_impl(
    state: &mut ExitAndReplaceState,
    return_stats: bool,
) -> anyhow::Result<Option<ReplaceState>> {
    let mut num_successes = 0;
    let mut num_ignored = 0;

    let mut line_map = state
        .replace_results
        .iter_mut()
        .map(|res| (res.search_result.line_number, res))
        .collect::<HashMap<_, _>>();

    for (idx, line) in state.stdin.lines().enumerate() {
        let line_number = idx + 1; // Ensure line-number is 1-indexed
        let line_new = line_map
            .get_mut(&line_number)
            .and_then(|res| {
                assert_eq!(
                    line, res.search_result.line,
                    "line has changed since search"
                );
                if res.search_result.included {
                    res.replace_result = Some(ReplaceResult::Success);
                    num_successes += 1;
                    Some(res.replacement.as_str())
                } else {
                    num_ignored += 1;
                    None
                }
            })
            .unwrap_or(line);
        writeln!(io::stderr(), "{line_new}")?;
    }

    let res = if return_stats {
        Some(ReplaceState {
            num_successes,
            num_ignored,
            errors: Vec::new(),
            replacement_errors_pos: 0,
        })
    } else {
        None
    };
    Ok(res)
}

#[cfg(test)]
mod tests {
    use scooter_core::{line_reader::LineEnding, replace::ReplaceResult, search::SearchResult};

    use super::*;

    #[test]
    fn test_format_replacement_results_no_errors() {
        let result = format_replacement_results(5, Some(2), Some(&[]));
        assert_eq!(
            result,
            "Successful replacements (lines): 5\nIgnored (lines): 2\nErrors: 0\n"
        );
    }

    #[test]
    fn test_format_replacement_results_with_errors() {
        let error_result = SearchResultWithReplacement {
            search_result: SearchResult {
                path: Some(PathBuf::from("file.txt")),
                line_number: 10,
                line: "line".to_string(),
                line_ending: LineEnding::Lf,
                included: true,
            },
            replacement: "replacement".to_string(),
            replace_result: Some(ReplaceResult::Error("Test error".to_string())),
        };

        let result = format_replacement_results(3, Some(1), Some(&[error_result]));
        assert!(result.contains("Successful replacements (lines): 3\n"));
        assert!(result.contains("Ignored (lines): 1"));
        assert!(result.contains("Errors: 1"));
        assert!(result.contains("file.txt:10"));
        assert!(result.contains("Test error"));
    }

    #[test]
    fn test_format_replacement_results_no_ignored_count() {
        let result = format_replacement_results(7, None, Some(&[]));
        assert_eq!(result, "Successful replacements (lines): 7\nErrors: 0\n");
        assert!(!result.contains("Ignored (lines):"));
    }

    #[test]
    fn test_build_editor_command_unquoted_file() {
        let result = build_editor_command(
            "vim %file +%line",
            Path::new("/path/with spaces/file.txt"),
            42,
        );
        if cfg!(windows) {
            assert_eq!(result, "vim \"/path/with spaces/file.txt\" +42");
        } else {
            assert_eq!(result, "vim '/path/with spaces/file.txt' +42");
        }
    }

    #[test]
    fn test_build_editor_command_double_quoted_file() {
        let result = build_editor_command(
            "notepad++ \"%file\" -n%line",
            Path::new("/path/with spaces/file.txt"),
            10,
        );
        assert_eq!(result, "notepad++ \"/path/with spaces/file.txt\" -n10");
    }

    #[test]
    fn test_build_editor_command_single_quoted_file() {
        let result = build_editor_command(
            "vim '%file' +%line",
            Path::new("/path/with spaces/file.txt"),
            5,
        );
        assert_eq!(result, "vim '/path/with spaces/file.txt' +5");
    }

    #[test]
    fn test_build_editor_command_colon_suffix() {
        let result = build_editor_command(
            "subl %file:%line",
            Path::new("/path/with spaces/file.txt"),
            2,
        );
        if cfg!(windows) {
            assert_eq!(result, "subl \"/path/with spaces/file.txt\":2");
        } else {
            assert_eq!(result, "subl '/path/with spaces/file.txt':2");
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_helix_tmux_send_keys() {
        let command = r#"tmux send-keys -t "$TMUX_PANE" ":open \"%file:%line\"" Enter"#;
        let result = build_editor_command(command, Path::new("/path/with spaces/file.txt"), 7);
        assert_eq!(
            result,
            r#"tmux send-keys -t "$TMUX_PANE" ":open \"/path/with spaces/file.txt\":7" Enter"#
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_neovim_remote_send() {
        let command = r#"nvim --server $NVIM --remote-send '<cmd>lua EditLineFromScooter("%file", %line)<CR>'"#;
        let result = build_editor_command(command, Path::new("/path/with spaces/file.txt"), 9);
        assert_eq!(
            result,
            r#"nvim --server $NVIM --remote-send '<cmd>lua EditLineFromScooter("/path/with spaces/file.txt", 9)<CR>'"#
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_single_quote_in_path() {
        let result = build_editor_command(
            "vim %file +%line",
            Path::new("/path/it's a file/foo.txt"),
            1,
        );
        assert_eq!(result, "vim '/path/it'\\''s a file/foo.txt' +1");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_double_quotes_escape_dollar() {
        let result = build_editor_command("echo \"%file\"", Path::new("/path/$HOME/file.txt"), 1);
        assert_eq!(result, "echo \"/path/\\$HOME/file.txt\"");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_double_quotes_escape_backslash() {
        let result = build_editor_command(
            "vim \"%file\"",
            Path::new("/path/with\\backslash/file.txt"),
            1,
        );
        assert_eq!(result, "vim \"/path/with\\\\backslash/file.txt\"");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_double_quotes_escape_backtick() {
        let result = build_editor_command(
            "vim \"%file\"",
            Path::new("/path/with`backtick/file.txt"),
            1,
        );
        assert_eq!(result, "vim \"/path/with\\`backtick/file.txt\"");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_empty_path() {
        let result = build_editor_command("vim %file", Path::new(""), 1);
        assert_eq!(result, "vim ''");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_multiple_file_tokens() {
        let result = build_editor_command("diff %file %file", Path::new("/path/my file.txt"), 1);
        assert_eq!(result, "diff '/path/my file.txt' '/path/my file.txt'");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_no_tokens() {
        let result = build_editor_command("vim", Path::new("/some/file.txt"), 1);
        assert_eq!(result, "vim");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_unterminated_double_quote() {
        // Unterminated quote — the %file token should still be escaped for double-quote context
        let result = build_editor_command("vim \"%file", Path::new("/path/$HOME/file.txt"), 1);
        assert_eq!(result, "vim \"/path/\\$HOME/file.txt");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_backslash_before_file_token_resets_escape() {
        // A backslash before %file should not cause the character after the substitution
        // to be treated as escaped (skipping quote-context tracking).
        // Here the " after %file should open a double-quote context, causing $VAR to be
        // escaped. If escape_next leaks, the " is skipped and $VAR is left unescaped.
        let result =
            build_editor_command(r#"cmd \%file"%line $VAR""#, Path::new("/some/file.txt"), 1);
        assert_eq!(result, r#"cmd \'/some/file.txt'"1 \$VAR""#);
    }

    #[test]
    #[cfg(not(windows))]
    fn test_build_editor_command_backslash_before_line_token_resets_escape() {
        // Same as above but with %line: the backslash before %line should not leak
        // escape_next, so the " immediately after %line opens a double-quote context.
        let result = build_editor_command(
            r#"cmd \%line"%file""#,
            Path::new("/path/$HOME/file.txt"),
            42,
        );
        assert_eq!(result, r#"cmd \42"/path/\$HOME/file.txt""#);
    }
}
