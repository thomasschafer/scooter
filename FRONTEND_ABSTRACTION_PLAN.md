# Frontend Abstraction Plan

## Status

**Ready for Phase 0 implementation.** All design decisions made, detailed implementation steps documented.

## Goal

Make `scooter-core` a truly frontend-agnostic library with a minimal public API that allows easy implementation of new frontends (Helix plugin, Neovim plugin, web UI, etc.).

## Current State

### What's Working Well
- Core business logic (search, replace, diff) is independent of UI frameworks
- `App` orchestrates all operations cleanly
- Keyboard abstraction exists to share config across implementations

### Current Problems

1. **`App` exposes internal state** - Fields like `current_screen`, `search_fields`, `searcher` are public, forcing frontends to understand internal structure
2. **Event passthrough required** - Frontends receive `Event::App` and `Event::PerformReplacement` and must call methods back on `App`
3. **No view abstraction** - Frontends pattern match on `Screen` enum and access nested state directly
4. **Rendering utilities in core** - `read_lines_range_highlighted()` uses syntect in core
5. **Single event pattern** - No support for callback-based frontends (Helix, Neovim)

## Architecture Vision

**scooter-core should expose:**
- Key event handling (shared config across frontends)
- View state objects (read-only snapshots for rendering)
- Two event consumption patterns (event loop and callback modes)
- Pure business logic functions

**Frontends should:**
- Translate input to `KeyEvent` and call `app.handle_key_event()`
- Get view state via `app.view()` and render it
- Handle async events via `next_event()` or `poll_event()`
- Own syntax highlighting and styling

---

## Implementation Plan

### Phase 0: Event System Cleanup

**Goal:** Remove event passthrough and support both event-loop and callback-based frontends.

#### Current Problem

Frontends receive events and must call methods back on `App`:

```rust
// In scooter/src/app_runner.rs
Some(event) = self.event_receiver.recv() => {
    match event {
        Event::App(app_event) => self.app.handle_app_event(&app_event),  // Passthrough
        Event::PerformReplacement => self.app.perform_replacement(),      // Passthrough
        Event::LaunchEditor((file, line)) => { /* handle */ }
        Event::ExitAndReplace(state) => { /* exit */ }
    }
}
```

Core sends `Event::App` and `Event::PerformReplacement` to itself via a channel, making frontends be middlemen.

#### Solution

**Split events:** Internal events (handled by core) vs Frontend events (handled by frontends).

```rust
// Internal - not exposed to frontends
enum InternalEvent {
    PerformSearch,
    PerformReplacement,
}

// Frontend - exposed
pub enum FrontendEvent {
    LaunchEditor((PathBuf, usize)),      // Frontend-specific
    ExitAndReplace(ExitAndReplaceState), // Exit for stdin mode
    Rerender,                             // Returned after internal/bg events
}
```

**Key simplification:** The old `AppEvent::Rerender` becomes `FrontendEvent::Rerender`. Core automatically returns `Rerender` after handling any internal or background event.

**Provide two APIs:**

```rust
impl App {
    // Async - for event loop frontends (Ratatui, standalone apps)
    pub async fn next_event(&mut self) -> Option<FrontendEvent>;

    // Non-blocking - for callback frontends (Helix, Neovim plugins)
    pub fn poll_event(&mut self) -> Option<FrontendEvent>;
}
```

#### Detailed Implementation Steps

**Step 1: Create new event types** in `scooter-core/src/app.rs`:

```rust
// Keep ExitAndReplaceState
pub struct ExitAndReplaceState {
    pub stdin: Arc<String>,
    pub search_config: ParsedSearchConfig,
    pub replace_results: Vec<SearchResultWithReplacement>,
}

// NEW: Internal events (not pub) - core handles these
enum InternalEvent {
    PerformSearch,
    PerformReplacement,
}

// NEW: Frontend events (pub) - frontends handle these
pub enum FrontendEvent {
    LaunchEditor((PathBuf, usize)),
    ExitAndReplace(ExitAndReplaceState),
    Rerender,
}

// OLD: Keep temporarily for migration, then remove
pub enum AppEvent {
    Rerender,    // Becomes FrontendEvent::Rerender
    PerformSearch,  // Becomes InternalEvent::PerformSearch
}

pub enum Event {
    LaunchEditor((PathBuf, usize)),
    App(AppEvent),
    PerformReplacement,
    ExitAndReplace(ExitAndReplaceState),
}
```

**Step 2: Update App struct** to have separate channels:

```rust
pub struct App {
    // ... existing fields ...

    // OLD: event_sender: UnboundedSender<Event>,
    // NEW:
    internal_sender: UnboundedSender<InternalEvent>,
    frontend_sender: UnboundedSender<FrontendEvent>,
}
```

**Step 3: Update all event sends** throughout `app.rs` and `replace.rs`:

```rust
// Internal events:
// OLD: self.event_sender.send(Event::PerformReplacement)
// NEW: self.internal_sender.send(InternalEvent::PerformReplacement)

// OLD: event_sender.send(Event::App(AppEvent::PerformSearch))
// NEW: internal_sender.send(InternalEvent::PerformSearch)

// Frontend events:
// OLD: event_sender.send(Event::App(AppEvent::Rerender))
// NEW: frontend_sender.send(FrontendEvent::Rerender)

// OLD: self.event_sender.send(Event::LaunchEditor((file, line)))
// NEW: self.frontend_sender.send(FrontendEvent::LaunchEditor((file, line)))

// OLD: self.event_sender.send(Event::ExitAndReplace(state))
// NEW: self.frontend_sender.send(FrontendEvent::ExitAndReplace(state))
```

**Locations to update:**
- `app.rs:762` - `Event::PerformReplacement` → `InternalEvent::PerformReplacement`
- `app.rs:1064` - `Event::App(AppEvent::PerformSearch)` → `InternalEvent::PerformSearch`
- `app.rs:1100` - `Event::App(AppEvent::Rerender)` → `FrontendEvent::Rerender`
- `app.rs:1396` - `Event::App(AppEvent::Rerender)` → `FrontendEvent::Rerender`
- `replace.rs:212` - `Event::App(AppEvent::Rerender)` → `FrontendEvent::Rerender`
- `replace.rs:217` - `Event::App(AppEvent::Rerender)` → `FrontendEvent::Rerender`
- Search for `Event::LaunchEditor` and `Event::ExitAndReplace` - update to `FrontendEvent::`

**Step 4: Update `App::new_with_receiver()`**:

```rust
pub fn new_with_receiver(
    input_source: InputSource,
    search_field_values: &SearchFieldValues<'a>,
    app_run_config: &AppRunConfig,
    config: Config,
) -> anyhow::Result<(Self, UnboundedReceiver<FrontendEvent>)> {
    let (internal_sender, internal_receiver) = mpsc::unbounded_channel();
    let (frontend_sender, frontend_receiver) = mpsc::unbounded_channel();

    let app = Self::new(
        input_source,
        search_field_values,
        internal_sender,
        frontend_sender,
        app_run_config,
        config,
    )?;

    Ok((app, frontend_receiver))
}
```

**Step 5: Implement `next_event()` and `poll_event()`**:

```rust
impl App {
    pub async fn next_event(&mut self,
        internal_rx: &mut UnboundedReceiver<InternalEvent>,
        frontend_rx: &mut UnboundedReceiver<FrontendEvent>
    ) -> Option<FrontendEvent> {
        loop {
            tokio::select! {
                Some(internal) = internal_rx.recv() => {
                    // Handle internal events automatically
                    match internal {
                        InternalEvent::PerformSearch => {
                            // Trigger search
                            self.perform_search_if_valid();
                        }
                        InternalEvent::PerformReplacement => {
                            self.perform_replacement();
                        }
                    }
                    // Return Rerender after handling internal event
                    return Some(FrontendEvent::Rerender);
                }
                Some(bg_event) = self.background_processing_recv() => {
                    self.handle_background_processing_event(bg_event);
                    // Return Rerender after background event
                    return Some(FrontendEvent::Rerender);
                }
                Some(frontend) = frontend_rx.recv() => {
                    // Return frontend events as-is
                    return Some(frontend);
                }
                else => return None,
            }
        }
    }

    pub fn poll_event(&mut self,
        internal_rx: &mut UnboundedReceiver<InternalEvent>,
        frontend_rx: &mut UnboundedReceiver<FrontendEvent>
    ) -> Option<FrontendEvent> {
        // Try internal events first (non-blocking)
        let mut handled_internal = false;
        while let Ok(internal) = internal_rx.try_recv() {
            match internal {
                InternalEvent::PerformSearch => {
                    self.perform_search_if_valid();
                }
                InternalEvent::PerformReplacement => {
                    self.perform_replacement();
                }
            }
            handled_internal = true;
        }

        if handled_internal {
            return Some(FrontendEvent::Rerender);
        }

        // Try background events (need non-blocking variant)
        // TODO: Implement try_recv for background processing

        // Try frontend events
        frontend_rx.try_recv().ok()
    }
}
```

**Note:** `AppEvent::Rerender` becomes `FrontendEvent::Rerender`, and we return it automatically after handling any internal or background event.

**Step 6: Update `scooter/src/app_runner.rs`**:

```rust
pub struct AppRunner<B: Backend, E: EventStream, S: SnapshotProvider<B>> {
    app: App,
    frontend_receiver: UnboundedReceiver<FrontendEvent>,  // Changed type
    internal_receiver: UnboundedReceiver<InternalEvent>,  // NEW
    tui: Tui<B>,
    event_stream: E,
    snapshot_provider: S,
}

// In run_event_loop:
tokio::select! {
    Some(Ok(event)) = self.event_stream.next() => {
        // Handle crossterm events...
    }
    Some(event) = self.app.next_event(&mut self.internal_receiver, &mut self.frontend_receiver) => {
        match event {
            FrontendEvent::LaunchEditor((file, line)) => { /* ... */ }
            FrontendEvent::ExitAndReplace(state) => { /* ... */ }
            FrontendEvent::Rerender => self.draw()?,
        }
    }
}
```

**Step 7: Remove old enums** after all migrations complete:
- Remove `pub enum Event`
- Remove `pub enum AppEvent`
- Remove `handle_app_event()` (or make it private/internal)

**Challenges to address:**

1. **Receiver ownership:** `next_event()` needs mutable access to receivers. Options:
   - Store receivers in `App` (makes API cleaner)
   - Pass receivers as parameters (more flexible)
   - Use interior mutability (RefCell/Mutex)

2. **Background processing channel:** Currently tied to screen state. Need to make it accessible to `next_event()`.

3. **API ergonomics:** Should `App::new_with_receiver()` return multiple receivers?

**Breaking Changes:** Major

---

### Phase 1: View State Abstraction

**Goal:** Hide internal state and expose immutable view snapshots.

#### Changes

**1. Create view state structs** in `scooter-core/src/view.rs`:

```rust
pub struct AppView<'a> {
    pub view: ViewKind<'a>,
    pub popup: Option<PopupView<'a>>,
    pub config: &'a Config,
}

pub enum PopupView<'a> {
    Error(&'a [AppError]),
    Help(&'a [(String, String)]),  // keymaps
    Text { title: &'a str, body: &'a str },
}

pub enum ViewKind<'a> {
    SearchFields(SearchFieldsView<'a>),
    PerformingReplacement(PerformingReplacementView<'a>),
    Results(ResultsView<'a>),
}

pub struct SearchFieldsView<'a> {
    // Fields
    pub search: &'a TextField,
    pub replace: &'a TextField,
    pub fixed_strings: &'a CheckboxField,
    pub whole_word: &'a CheckboxField,
    pub match_case: &'a CheckboxField,
    pub include_files: &'a TextField,
    pub exclude_files: &'a TextField,
    pub highlighted_idx: usize,
    pub focussed_section: FocussedSection,

    // Optional search results (appears in same screen)
    pub search_results: Option<SearchResultsView<'a>>,
}

pub struct SearchResultsView<'a> {
    pub input_source: &'a InputSource,
    pub results: &'a [SearchResultWithReplacement],
    pub primary_selected_idx: usize,
    pub selected_indices: SelectedIndices<'a>,  // Helper for is_selected()
    pub view_offset: usize,
    pub search_started: Instant,
    pub search_completed: Option<Instant>,
    pub replacements_in_progress: Option<(usize, usize)>,
}

// Helper to check if index is selected without exposing internal Selected enum
pub struct SelectedIndices<'a> {
    selection: &'a Selected,
}

impl SelectedIndices<'_> {
    pub fn is_selected(&self, idx: usize) -> bool {
        // Delegates to SearchState::is_selected() logic
    }

    pub fn is_primary(&self, idx: usize) -> bool {
        // Delegates to SearchState::is_primary_selected() logic
    }
}

pub struct PerformingReplacementView<'a> {
    pub num_completed: usize,
    pub total: usize,
    pub elapsed: Duration,
}

pub struct ResultsView<'a> {
    pub num_successes: usize,
    pub num_ignored: usize,
    pub errors: &'a [SearchResultWithReplacement],
    pub scroll_offset: usize,
}
```

**2. Add view accessor:**

```rust
impl App {
    pub fn view(&self) -> AppView<'_>;
}
```

**3. Make fields private:**

```rust
pub struct App {
    config: Config,              // Access via view
    current_screen: Screen,      // Access via view
    search_fields: SearchFields, // Access via view
    // ... rest private
}
```

**4. Update `scooter/src/ui/view.rs`:**

Replace `app.current_screen` and `app.search_fields` with `app.view()`.

**Breaking Changes:** Major

---

### Phase 2: Extract Rendering Utilities

**Goal:** Move syntax highlighting to frontends.

#### Changes

**1. In `scooter-core/src/utils.rs`:**
- Keep `read_lines_range()` (pure file I/O)
- Remove `read_lines_range_highlighted()`

**2. In `scooter/src/syntax.rs` (new file):**
- Move `read_lines_range_highlighted()` here
- Use `scooter_core::utils::read_lines_range()` + syntect

**3. Make `syntect` optional in scooter-core:**
- Only keep if needed for theme loading in `Config`
- Otherwise move entirely to frontends

**4. Update `scooter/src/ui/view.rs`:**
- Use local `syntax::read_lines_range_highlighted()`

**Breaking Changes:** Minor (remove public function)

---

### Phase 3: Field State Cleanup

**Goal:** Ensure text field logic is clean (low priority).

#### Review Areas

1. `TextField` - verify keyboard handling is minimal
2. `CheckboxField` - verify no rendering logic
3. `SearchFields` - verify no UI-specific code

**Breaking Changes:** Minor or none

---

### Phase 4: Config Review

**Goal:** Clarify what config is core vs frontend-specific.

#### Current Config Structure

```rust
pub struct Config {
    pub editor_open: EditorOpenConfig,  // Core ✅
    pub search: SearchConfig,           // Core ✅
    pub keys: KeysConfig,               // Core ✅
    pub preview: PreviewConfig,         // Theme, syntax - frontend?
    pub style: StyleConfig,             // True color - frontend?
}
```

#### Options

**A. Keep all in core** - Document which fields frontends should use
**B. Split config** - Move UI config to frontends
**C. Make UI config optional** - Keep in core but mark as frontend-specific

**Decision needed**

**Breaking Changes:** Depends on approach

---

### Phase 5: Documentation

**Goal:** Make it trivial to implement a new frontend.

#### Deliverables

1. **`scooter-core/FRONTEND_GUIDE.md`:**
   - Architecture overview
   - Step-by-step implementation guide
   - Event loop vs callback mode patterns
   - View rendering examples

2. **`examples/minimal_frontend.rs`:**
   - Bare-bones frontend (no ratatui)
   - Shows complete event loop
   - Demonstrates view rendering

3. **API documentation:**
   - Document all public items in scooter-core
   - Add examples to key types
   - Clarify frontend vs core responsibilities

4. **`scooter-core/README.md`:**
   - Explain crate purpose
   - Link to frontend guide
   - List example frontends

**Breaking Changes:** None

---

## Implementation Order

**Recommended:**
1. Phase 0 (Events) - Critical API fix
2. Phase 1 (View) - Core abstraction
3. Phase 2 (Rendering) - Can parallel with Phase 1
4. Phase 5 (Docs) - Make usable
5. Phase 3 (Fields) - Polish
6. Phase 4 (Config) - Evaluate and decide

**Minimal viable:**
1. Phase 0 + Phase 1 (Events + View)
2. Phase 5 (Basic docs)
3. Rest as needed

---

## Frontend Implementation Examples

### Pattern 1: Event Loop Mode (Ratatui, standalone TUI)

```rust
use scooter_core::{App, KeyEvent, FrontendEvent};

async fn run_event_loop(app: &mut App) {
    let mut input_stream = crossterm::event::EventStream::new();

    loop {
        tokio::select! {
            // User input
            Some(Ok(crossterm::event::Event::Key(key))) = input_stream.next() => {
                let key_event: KeyEvent = key.into();
                if app.handle_key_event(key_event).should_exit() {
                    break;
                }
                render(app);
            }

            // Async events from core
            Some(event) = app.next_event() => {
                match event {
                    FrontendEvent::LaunchEditor((file, line)) => {
                        suspend_tui()?;
                        open_editor(file, line)?;
                        resume_tui()?;
                    }
                    FrontendEvent::ExitAndReplace(state) => break,
                    FrontendEvent::Rerender => render(app),
                }
            }
        }
    }
}

fn render(app: &App) {
    let view = app.view();
    // Render based on view.view (ViewKind)
}
```

### Pattern 2: Callback Mode (Helix, Neovim plugins)

```rust
use scooter_core::{App, KeyEvent, FrontendEvent};

struct HelixScooterPlugin {
    app: App,
}

impl HelixScooterPlugin {
    // Called by Helix on key press
    pub fn on_key(&mut self, helix_key: HelixKey) {
        let key_event = translate_key(helix_key);
        if app.handle_key_event(key_event).should_exit() {
            self.close();
            return;
        }
        self.render();
    }

    // Called by Helix event loop every tick
    pub fn on_tick(&mut self) {
        let mut needs_render = false;

        while let Some(event) = self.app.poll_event() {
            match event {
                FrontendEvent::LaunchEditor((file, line)) => {
                    // Native Helix - just jump to location
                    helix_goto(file, line);
                }
                FrontendEvent::ExitAndReplace(state) => {
                    self.close();
                    return;
                }
                FrontendEvent::Rerender => {
                    needs_render = true;
                }
            }
        }

        if needs_render {
            self.render();
        }
    }

    fn render(&mut self) {
        let view = self.app.view();
        // Render using Helix UI system
    }
}
```

**Key differences:**
- Event loop: Uses `next_event()` (async) in `tokio::select!`
- Callback: Uses `poll_event()` (non-blocking) in host event loop
- Callback: Batches rendering for efficiency
- Both: Same view API, same key handling

---

## Success Criteria

A new frontend implementation should:

✅ **Only need to:**
- Translate input to `KeyEvent`
- Call `app.handle_key_event(key_event)`
- Call `app.view()` to get render state
- Call `app.next_event()` or `app.poll_event()` for async updates
- Render the view with their UI framework

✅ **Never need to:**
- Understand `App` internal state
- Manage screen transitions
- Handle search/replace orchestration
- Call passthrough methods like `handle_app_event()`

✅ **All frontends share:**
- Configuration (key bindings, search settings)
- Search/replace logic
- Field validation
- Diff generation

✅ **Frontends control:**
- Rendering and layout
- Syntax highlighting
- Color schemes
- Editor integration

---

## Open Questions

1. **Config ownership:** Should `PreviewConfig` (themes) and `StyleConfig` stay in core?
   - **Lean toward:** Keep in core for consistency, mark as optional for frontends

2. **LaunchEditor event:** Should this be optional/configurable?
   - **Lean toward:** Yes, editor plugins can disable it

3. **Syntect dependency:** Keep in core or move entirely to frontends?
   - **Depends on:** Config decision above
