# Frontend Abstraction Plan

## Status

**Phase 0 in progress.** Event system design finalized with unified event channel approach.

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

**Split events:** Internal events (handled by core) vs Frontend events (handled by frontends), unified into a single channel.

```rust
// Top-level event enum - wraps both types in single channel
pub enum Event {
    Internal(InternalEvent),
    Frontend(FrontendEvent),
}

// Internal - handled by core, not directly exposed
enum InternalEvent {
    PerformSearch,
    PerformReplacement,
}

// Frontend - handled by frontends
pub enum FrontendEvent {
    LaunchEditor((PathBuf, usize)),      // Frontend-specific
    ExitAndReplace(ExitAndReplaceState), // Exit for stdin mode
    Rerender,                             // Returned after internal/bg events
}
```

**Key simplification:** The old `AppEvent::Rerender` becomes `FrontendEvent::Rerender`. Core automatically returns `Rerender` after handling any internal or background event.

**Unified channel approach:** Instead of separate channels for internal and frontend events, use a single `Event` channel. Background tasks (like debounce timers) send `Event::Internal(...)`, replacement threads send `Event::Frontend(FrontendEvent::Rerender)`, etc. Core's event handling methods automatically process internal events and return frontend events.

**Event handling in frontends:**

Frontends receive a single `UnboundedReceiver<Event>` and match on the variants. The frontend event loop handles `Event::Internal` by calling core methods and handles `Event::Frontend` directly.

#### Detailed Implementation Steps

**Step 1: Create new event types** in `scooter-core/src/app.rs`:

```rust
// Keep ExitAndReplaceState
pub struct ExitAndReplaceState {
    pub stdin: Arc<String>,
    pub search_config: ParsedSearchConfig,
    pub replace_results: Vec<SearchResultWithReplacement>,
}

// NEW: Top-level unified event enum
pub enum Event {
    Internal(InternalEvent),
    Frontend(FrontendEvent),
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

// OLD: Remove these after migration
pub enum AppEvent {
    Rerender,           // Becomes FrontendEvent::Rerender
    PerformSearch,      // Becomes InternalEvent::PerformSearch
}

pub enum OldEvent {
    LaunchEditor((PathBuf, usize)),
    App(AppEvent),
    PerformReplacement,
    ExitAndReplace(ExitAndReplaceState),
}
```

**Step 2: Update App struct** to use unified event channel:

```rust
pub struct App {
    // ... existing fields ...

    // Single event sender for both internal and frontend events
    event_sender: UnboundedSender<Event>,
}
```

**Step 3: Update all event sends** throughout `app.rs` and `replace.rs`:

```rust
// Internal events - wrapped in Event::Internal:
// OLD: self.event_sender.send(Event::PerformReplacement)
// NEW: self.event_sender.send(Event::Internal(InternalEvent::PerformReplacement))

// OLD: event_sender.send(Event::App(AppEvent::PerformSearch))
// NEW: event_sender.send(Event::Internal(InternalEvent::PerformSearch))

// Frontend events - wrapped in Event::Frontend:
// OLD: event_sender.send(Event::App(AppEvent::Rerender))
// NEW: event_sender.send(Event::Frontend(FrontendEvent::Rerender))

// OLD: self.event_sender.send(Event::LaunchEditor((file, line)))
// NEW: self.event_sender.send(Event::Frontend(FrontendEvent::LaunchEditor((file, line))))

// OLD: self.event_sender.send(Event::ExitAndReplace(state))
// NEW: self.event_sender.send(Event::Frontend(FrontendEvent::ExitAndReplace(state)))
```

**Locations to update:**
- `app.rs:762` - `Event::PerformReplacement` → `Event::Internal(InternalEvent::PerformReplacement)`
- `app.rs:1052` - `Event::App(AppEvent::PerformSearch)` → `Event::Internal(InternalEvent::PerformSearch)` (in debounce timer)
- `app.rs:1100` - `Event::App(AppEvent::Rerender)` → `Event::Frontend(FrontendEvent::Rerender)`
- `app.rs:1396` - `Event::App(AppEvent::Rerender)` → `Event::Frontend(FrontendEvent::Rerender)`
- `replace.rs:212` - `Event::App(AppEvent::Rerender)` → `Event::Frontend(FrontendEvent::Rerender)`
- `replace.rs:217` - `Event::App(AppEvent::Rerender)` → `Event::Frontend(FrontendEvent::Rerender)`
- Search for `Event::LaunchEditor` and `Event::ExitAndReplace` - update to `Event::Frontend(FrontendEvent::...)`

**Step 4: Update `App::new_with_receiver()`**:

```rust
pub fn new_with_receiver(
    input_source: InputSource,
    search_field_values: &SearchFieldValues<'a>,
    app_run_config: &AppRunConfig,
    config: Config,
) -> anyhow::Result<(Self, UnboundedReceiver<Event>)> {
    let (event_sender, event_receiver) = mpsc::unbounded_channel();

    let app = Self::new(
        input_source,
        search_field_values,
        event_sender,
        app_run_config,
        config,
    )?;

    Ok((app, event_receiver))
}
```

**Step 5: Add internal event handler to App**:

Since frontends will match on `Event` variants, we need a method to handle internal events:

```rust
impl App {
    // Public method for frontends to call when they receive Event::Internal
    pub fn handle_internal_event(&mut self, event: InternalEvent) -> EventHandlingResult {
        match event {
            InternalEvent::PerformSearch => {
                self.perform_search_if_valid();
                EventHandlingResult::Rerender
            }
            InternalEvent::PerformReplacement => {
                self.perform_replacement();
                EventHandlingResult::Rerender
            }
        }
    }
}
```

**Note:** This follows the same pattern as `handle_key_event()` and `handle_background_processing_event()` - returns `EventHandlingResult` so frontends can handle all events uniformly.

**Step 6: Update `scooter/src/app_runner.rs`**:

The key is to maintain the existing clean pattern where each `tokio::select!` branch produces an `EventHandlingResult`, then a single match at the end handles all cases.

```rust
// In run_event_loop (lines 216-279):
loop {
    let event_handling_result = tokio::select! {
        Some(Ok(event)) = self.event_stream.next() => {
            // Unchanged - handles crossterm events
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
        Some(event) = self.event_receiver.recv() => {
            match event {
                Event::Internal(internal_event) => {
                    self.app.handle_internal_event(internal_event)
                }
                Event::Frontend(FrontendEvent::LaunchEditor((file_path, line))) => {
                    // Keep existing LaunchEditor implementation (lines 232-253)
                    // Just change Event::LaunchEditor to Event::Frontend(FrontendEvent::LaunchEditor)
                    let mut res = EventHandlingResult::Rerender;
                    self.tui.show_cursor()?;
                    match self.open_editor(file_path, line) {
                        // ... existing code ...
                    }
                    self.tui.init()?;
                    res
                }
                Event::Frontend(FrontendEvent::ExitAndReplace(state)) => {
                    // Keep existing ExitAndReplace implementation (line 254-256)
                    return Ok(Some(ExitState::StdinState(state)));
                }
                Event::Frontend(FrontendEvent::Rerender) => {
                    // Keep existing Rerender implementation (line 257-259)
                    EventHandlingResult::Rerender
                }
            }
        }
        Some(event) = self.app.background_processing_recv() => {
            // Keep existing background processing implementation (line 265-267)
            self.app.handle_background_processing_event(event)
        }
        else => {
            // Keep existing else implementation (line 268-270)
            return Ok(None);
        }
    };

    // Keep existing EventHandlingResult match (lines 273-277)
    match event_handling_result {
        EventHandlingResult::Rerender => self.draw()?,
        EventHandlingResult::Exit(results) => return Ok(results.map(|t| *t)),
        EventHandlingResult::None => {}
    }
}
```

**Changes:**
- Add new match arm for `Event::Internal(internal_event)` → calls `app.handle_internal_event()`
- Change `Event::App(app_event)` → removed (replaced by Internal)
- Change `Event::LaunchEditor` → `Event::Frontend(FrontendEvent::LaunchEditor)` (keep existing impl)
- Change `Event::ExitAndReplace` → `Event::Frontend(FrontendEvent::ExitAndReplace)` (keep existing impl)
- Change `Event::Rerender` → `Event::Frontend(FrontendEvent::Rerender)` (keep existing impl)
- **Pattern preserved**: each branch produces `EventHandlingResult`, single match at end (minimal changes to existing structure)

**Step 7: Remove old enums** after all migrations complete:
- Remove old `Event` enum variants (App, LaunchEditor, ExitAndReplace, Rerender)
- Remove `pub enum AppEvent`
- Remove or rename `handle_event()` method (was `handle_app_event()`)

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
use scooter_core::{App, Event, InternalEvent, FrontendEvent, KeyEvent, EventHandlingResult};

async fn run_event_loop(
    app: &mut App,
    event_receiver: &mut UnboundedReceiver<Event>
) -> Result<()> {
    let mut input_stream = crossterm::event::EventStream::new();

    loop {
        let event_handling_result = tokio::select! {
            // User input
            Some(Ok(crossterm_event)) = input_stream.next() => {
                let key_event: KeyEvent = crossterm_event.into();
                app.handle_key_event(key_event)
            }

            // Events from core (internal + frontend)
            Some(event) = event_receiver.recv() => {
                match event {
                    Event::Internal(internal_event) => {
                        app.handle_internal_event(internal_event)
                    }
                    Event::Frontend(FrontendEvent::LaunchEditor((file, line))) => {
                        // Handle editor launch - suspend TUI, open editor, resume
                        // (frontend-specific implementation)
                        EventHandlingResult::Rerender
                    }
                    Event::Frontend(FrontendEvent::ExitAndReplace(state)) => {
                        return Ok(Some(state));
                    }
                    Event::Frontend(FrontendEvent::Rerender) => {
                        EventHandlingResult::Rerender
                    }
                }
            }

            // Background processing events
            Some(bg_event) = app.background_processing_recv() => {
                app.handle_background_processing_event(bg_event)
            }
        };

        match event_handling_result {
            EventHandlingResult::Rerender => render(app)?,
            EventHandlingResult::Exit(results) => return Ok(results),
            EventHandlingResult::None => {}
        }
    }
}

fn render(app: &App) -> Result<()> {
    let view = app.view();
    // Render based on view.view (ViewKind)
    Ok(())
}
```

### Pattern 2: Callback Mode (Helix, Neovim plugins)

```rust
use scooter_core::{App, Event, InternalEvent, FrontendEvent, KeyEvent, EventHandlingResult};

struct HelixScooterPlugin {
    app: App,
    event_receiver: UnboundedReceiver<Event>,
}

impl HelixScooterPlugin {
    // Called by Helix on key press
    pub fn on_key(&mut self, helix_key: HelixKey) {
        let key_event = translate_key(helix_key);
        match self.app.handle_key_event(key_event) {
            EventHandlingResult::Exit(_) => self.close(),
            EventHandlingResult::Rerender => self.render(),
            EventHandlingResult::None => {}
        }
    }

    // Called by Helix event loop every tick
    pub fn on_tick(&mut self) {
        let mut needs_render = false;

        // Process all pending events (non-blocking)
        while let Ok(event) = self.event_receiver.try_recv() {
            let result = match event {
                Event::Internal(internal_event) => {
                    self.app.handle_internal_event(internal_event)
                }
                Event::Frontend(FrontendEvent::LaunchEditor((file, line))) => {
                    // Native Helix - just jump to location
                    helix_goto(file, line);
                    EventHandlingResult::None
                }
                Event::Frontend(FrontendEvent::ExitAndReplace(state)) => {
                    self.close();
                    return;
                }
                Event::Frontend(FrontendEvent::Rerender) => {
                    EventHandlingResult::Rerender
                }
            };

            if matches!(result, EventHandlingResult::Rerender) {
                needs_render = true;
            } else if matches!(result, EventHandlingResult::Exit(_)) {
                self.close();
                return;
            }
        }

        // Check background processing (non-blocking)
        while let Some(bg_event) = self.app.try_recv_background_processing() {
            let result = self.app.handle_background_processing_event(bg_event);
            if matches!(result, EventHandlingResult::Rerender) {
                needs_render = true;
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
- Event loop: Uses `.recv()` (async/blocking) in `tokio::select!`
- Callback: Uses `.try_recv()` (non-blocking) in host event loop
- Callback: Batches rendering for efficiency (single render per tick)
- Both: Same `EventHandlingResult` pattern, same view API, same key handling

---

## Success Criteria

A new frontend implementation should:

✅ **Only need to:**
- Translate input to `KeyEvent` and call `app.handle_key_event()`
- Receive events from `event_receiver` and match on `Event::Internal` vs `Event::Frontend`
- Call `app.handle_internal_event()` for internal events
- Handle frontend events (`LaunchEditor`, `ExitAndReplace`, `Rerender`)
- Match on `EventHandlingResult` (Rerender/Exit/None) for all event handling
- Call `app.view()` to get render state
- Render the view with their UI framework

✅ **Never need to:**
- Understand `App` internal state
- Manage screen transitions
- Handle search/replace orchestration
- Know difference between internal vs frontend events (just call appropriate handler)

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
