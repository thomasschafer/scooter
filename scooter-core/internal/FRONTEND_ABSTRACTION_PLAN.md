# Frontend abstraction plan

## Status

Ready to begin Phase 1 — view state abstraction.

## Goal

Make `scooter-core` a frontend-agnostic library with a clean public API. Since `scooter-core` is only consumed internally (the TUI and `scooter.hx`), breaking changes are fine — just update both consumers as part of the work. Frontends (the ratatui TUI, the `scooter.hx` Helix plugin, future editor plugins) should only need to implement their UI layer — no reimplementing of search/replace state machines, selection logic, result management, or diff generation.

### The problem today

`scooter.hx` (the Helix plugin, in a separate repo) contains its own `ScooterHx` struct that fully reimplements the search/replace lifecycle — states like `NotStarted`, `SearchInProgress`, `SearchComplete`, `PerformingReplacement`, `ReplacementComplete` — because scooter-core's `App` was too opaque to use directly. All of that logic already exists in `scooter-core`; it just isn't accessible to frontends.

After this work, `scooter.hx` should be able to drop its reimplemented state machine entirely and wrap the `scooter-core` API in a thin FFI layer. No Helix- or Steel-specific code belongs in this repo.

---

## Current state

### What's working well
- Core business logic (search, replace, diff) is correct and well-tested
- `App` orchestrates all operations cleanly
- Keyboard abstraction (`KeyEvent`, `KeyCode`, `KeyModifiers`) exists and is reusable

### Current problems

1. **`App` leaks internal state** — `current_screen`, `search_fields`, `searcher`, `popup`, `ui_state` etc. are all `pub`, forcing frontends to understand internals
2. **No view abstraction** — frontends pattern match on `Screen`, `SearchState`, `Selected` directly
3. **Operations only reachable via key events** — there's no way to e.g. call `start_search(params)` directly; everything goes through `handle_key_event()`, which is TUI-centric and useless to a plugin that manages its own UI state
4. **UI state in core** — `num_displayed`, `last_render`, `view_offset` live in core but are frontend concerns
5. **Async event loop coupling** — frontends must drive `event_recv()` + `handle_internal_event()` in a tokio select loop; doesn't fit frontends that don't own the event loop

---

## Architecture vision

### scooter-core exposes:

**Lifecycle operations** (directly callable, not gated behind key events):
- `app.start_search(params)` — begin a search with the given configuration
- `app.cancel_search()` — cancel any in-progress search
- `app.start_replace()` — begin replacement on current results
- `app.cancel_replace()` — cancel any in-progress replacement
- `app.reset()` — return to initial state

**Selection:**
- `app.toggle_inclusion(idx)` — toggle a single result in/out
- `app.toggle_all()` — toggle all results

**View:**
- `app.view() -> AppView<'_>` — immutable snapshot of all state needed to render
- `app.poll_background_events() -> bool` — drain pending internal events; returns `true` if state changed

**Key event handling (TUI convenience):**
- `app.handle_key_event(KeyEvent) -> EventHandlingResult` — retained for the TUI, which drives everything via key events

### Frontends are responsible for:
- Their own input handling (translating native events to scooter operations or `KeyEvent`)
- Rendering using their own UI framework, based on `app.view()`
- Driving background event processing: either polling `event_recv()` in an async select loop (TUI), or calling `poll_background_events()` on each frame (plugin frontends)
- Handling `EventHandlingResult::Exit`, `LaunchEditor`, `ExitAndReplace` in a frontend-appropriate way

### What frontends never touch:
- `Screen`, `SearchState`, `SearchFields`, `Selected`, `Searcher`, `UIState` — all `pub(crate)`
- Search/replace orchestration, background task management, diff generation, file I/O

---

## Phase 1: View state abstraction

**Goal:** Hide internal state behind immutable view snapshots, and expose direct operation methods.

### 1.1 — `App` constructor

```rust
impl App {
    pub fn new(config: Config, directory: PathBuf) -> Self;
}
```

`Config` is already public. `directory` is the root path to search under. All other startup behaviour (headless mode, immediate search, etc.) moves out of `App` and into `AppRunConfig`, which is passed separately to the TUI runner rather than stored in `App`.

### 1.2 — Expose direct operation methods on `App`

Make these callable without going through `handle_key_event()`. `handle_key_event` is retained for the TUI but should internally delegate to these same methods — so both paths share logic and stay consistent.

```rust
impl App {
    /// Start a search with the given parameters.
    pub fn start_search(&mut self, params: SearchParams) -> Result<(), SearchError>;

    /// Cancel any in-progress search.
    pub fn cancel_search(&mut self);

    /// Start replacement on the current search results.
    pub fn start_replace(&mut self);

    /// Cancel any in-progress replacement.
    pub fn cancel_replace(&mut self);

    /// Toggle inclusion of a result by index.
    pub fn toggle_inclusion(&mut self, idx: usize);

    /// Toggle inclusion of all results.
    pub fn toggle_all(&mut self);

    /// Reset to initial state.
    pub fn reset(&mut self);
}

pub struct SearchParams {
    pub search: String,
    pub replace: String,
    pub fixed_strings: bool,
    pub match_whole_word: bool,
    pub match_case: bool,
    pub include_files: String,
    pub exclude_files: String,
    pub include_hidden: bool,
    pub multiline: bool,
}
```

### 1.3 — Create view types in `scooter-core/src/view.rs`

```rust
pub struct AppView<'a> {
    pub screen: ScreenView<'a>,
    pub popup: Option<PopupView<'a>>,
    pub config: &'a Config,
    pub input_source: &'a InputSource,
}

pub enum PopupView<'a> {
    Error(&'a [AppError]),
    Help(&'a [(String, String)]),   // (keybinding, description) pairs
    Text { title: &'a str, body: &'a str },
}

pub enum ScreenView<'a> {
    SearchFields(SearchFieldsView<'a>),
    PerformingReplacement(PerformingReplacementView<'a>),
    Results(ResultsView<'a>),
}

pub struct SearchFieldsView<'a> {
    pub search: &'a TextField,
    pub replace: &'a TextField,
    pub fixed_strings: &'a CheckboxField,
    pub whole_word: &'a CheckboxField,
    pub match_case: &'a CheckboxField,
    pub include_files: &'a TextField,
    pub exclude_files: &'a TextField,
    pub highlighted_idx: usize,
    pub focussed_section: FocussedSection,
    pub search_results: Option<SearchResultsView<'a>>,  // present once search triggered
}

pub struct SearchResultsView<'a> {
    pub results: &'a [SearchResultWithReplacement],
    pub primary_selected_idx: usize,
    pub selection: SelectionView<'a>,   // opaque; exposes is_selected(idx), is_primary(idx)
    pub view_offset: usize,
    pub search_started: Instant,
    pub search_completed: Option<Instant>,
}

pub struct SelectionView<'a> { /* opaque */ }
impl SelectionView<'_> {
    pub fn is_selected(&self, idx: usize) -> bool;
    pub fn is_primary(&self, idx: usize) -> bool;
}

pub struct PerformingReplacementView<'a> {
    pub num_completed: usize,
    pub total: usize,
    pub started_at: Instant,
}

pub struct ResultsView<'a> {
    pub num_successes: usize,
    pub num_ignored: usize,
    pub errors: &'a [SearchResultWithReplacement],
    pub scroll_offset: usize,
}
```

### 1.4 — Add view accessor and background event poll to `App`

```rust
impl App {
    /// Immutable snapshot of all state needed to render.
    pub fn view(&self) -> AppView<'_>;

    /// Drain all pending background events synchronously.
    /// Returns true if any state changed (caller should rerender).
    /// For frontends that don't own an async event loop.
    pub fn poll_background_events(&mut self) -> bool;
}
```

### 1.5 — Lock down internal types

Make `pub(crate)`:

```rust
pub(crate) enum Screen { ... }
pub(crate) struct SearchFieldsScreenState { ... }
pub(crate) struct SearchState { ... }
pub(crate) struct SearchFields { ... }
pub(crate) enum Selected { ... }
pub(crate) struct Searcher { ... }
pub(crate) struct UIState { ... }
pub(crate) enum Popup { ... }

// App fields — all private, accessed via view() or operation methods
pub struct App {
    config: Config,
    current_screen: Screen,
    search_fields: SearchFields,
    searcher: Option<Searcher>,
    input_source: InputSource,
    run_config: AppRunConfig,
    event_channels: EventChannels,
    ui_state: UIState,
    popup: Option<Popup>,
}

// These remain pub — needed by frontends directly
pub struct TextField { ... }
pub struct CheckboxField { ... }
pub enum FocussedSection { ... }
pub enum InputSource { ... }
pub enum EventHandlingResult { ... }
pub struct SearchResultWithReplacement { ... }  // see 1.5 for public API

// AppRunConfig moves out of App — TUI-specific startup options live here, not in core
pub(crate) struct AppRunConfig { ... }  // or moved entirely to scooter crate
```

### 1.6 — `SearchResultWithReplacement` public API

This type is the primary data returned to frontends, so its fields need to be accessible. Expose at minimum:

```rust
impl SearchResultWithReplacement {
    pub fn display_path(&self) -> &str;
    pub fn absolute_path(&self) -> &Path;
    pub fn line_number(&self) -> usize;
    pub fn line_text(&self) -> &str;
    pub fn replacement_text(&self) -> &str;
    pub fn is_included(&self) -> bool;
    pub fn replace_result(&self) -> Option<&ReplaceResult>;  // error/success after replacement

    /// Build a diff preview for rendering (highlighted before/after lines).
    /// `context_lines` controls how many surrounding lines to include.
    pub fn build_preview(&self, context_lines: usize) -> Vec<PreviewLine>;
}
```

`build_preview` moves the diff logic that `scooter.hx` currently reimplicates into core as a first-class public method.

### 1.7 — Remove UI state from core

- Remove `num_displayed` from `SearchState` — purely a frontend concern, computed from viewport height
- Keep `view_offset` in core for now (affects keyboard nav logic), exposed via `SearchResultsView`

### 1.8 — Update TUI frontend

Replace all direct field accesses (`app.current_screen`, `app.search_fields`, `app.popup`, etc.) in `scooter/src/ui/` with `app.view()`.

---

## Phase 2: Documentation and examples

**Goal:** Make it trivial to implement a new frontend without reading scooter-core internals.

### 2.1 — `scooter-core/FRONTEND_GUIDE.md`
- Architecture overview
- What `App` manages vs what the frontend is responsible for
- Two integration patterns:
  - **Async loop** (TUI): `tokio::select!` over input + `app.event_recv()`
  - **Poll on frame** (plugin): call `app.poll_background_events()` before rendering
- View rendering reference — what each view type contains and when it appears
- Operation reference — `start_search`, `toggle_inclusion`, etc.

### 2.2 — `examples/minimal_frontend.rs`
- Bare-bones non-TUI frontend (no ratatui)
- Uses direct operation methods (`start_search`, `toggle_inclusion`, `start_replace`)
- Uses `poll_background_events()` rather than an async select loop
- Useful reference for future plugin authors

### 2.3 — API documentation
- All public items in scooter-core have doc comments
- Key types have examples
- Clear frontend/core responsibility boundary documented

---

## Deferred / future

### Rendering utilities

Syntax highlighting (`read_lines_range_highlighted()`, syntect) currently lives in scooter-core. This is fine for now since both the TUI and `scooter.hx` use it. Revisit if editor plugins want to use their own highlighting instead.

### Config

Which config is relevant per frontend type — documented here for reference, not action yet:

| Config field | TUI | Editor plugin |
|---|---|---|
| `editor_open` | ✅ | ❌ (the editor handles this) |
| `search` | ✅ | ✅ |
| `keys` | ✅ | ❌ (editor manages keybindings) |
| `preview.syntax_highlighting` | ✅ | ✅ or defer to editor |
| `preview.syntax_highlighting_theme` | ✅ | ❌ (use editor's theme) |
| `preview.wrap_text` | ✅ | ✅ |
| `style.true_color` | ✅ | ❌ (editor handles this) |

Frontends can safely ignore irrelevant config fields — they don't affect core logic.

### Field state cleanup

Review `TextField`, `CheckboxField`, `SearchFields` to confirm there's no rendering logic mixed into them. Expected to be clean already but worth verifying.

---

## Success criteria

After this work, a frontend needs only to:

1. Call `App::new()` to create an instance
2. Either:
   - Call `app.start_search(params)` / `app.toggle_inclusion(idx)` / etc. directly (plugin-style), or
   - Translate input to `KeyEvent` and call `app.handle_key_event()` (TUI-style)
3. Call `app.poll_background_events()` each frame, or drive `app.event_recv()` in an async loop
4. Call `app.view()` and render the result using their own UI framework
5. Handle `EventHandlingResult::Exit` and any frontend-specific events (`LaunchEditor`, `ExitAndReplace`)

It should never need to reimplement search/replace state machines, selection logic, diff generation, or file I/O.
