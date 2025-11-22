# Frontend Abstraction Plan

## Status

**Phase 0 in progress** - Event system partially refactored. See Phase 0 section for remaining work.

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
- Handle async events via `event_recv()` or `poll_event()`
- Own syntax highlighting and styling

---

## Implementation Plan

### Phase 0: Event System Cleanup

**Goal:** Remove event passthrough and support both event-loop and callback-based frontends.

#### Current State

✅ Core owns event channels, frontends call `app.event_recv()`
✅ Internal events unified under `Event::Internal(InternalEvent)`
✅ Frontend calls `app.handle_internal_event()` - no passthrough

#### Remaining Work

🔲 **Wrap frontend events** - Create `FrontendEvent` enum and wrap:
  - `Event::LaunchEditor` → `Event::Frontend(FrontendEvent::LaunchEditor(...))`
  - `Event::ExitAndReplace` → `Event::Frontend(FrontendEvent::ExitAndReplace(...))`
  - `Event::Rerender` → `Event::Frontend(FrontendEvent::Rerender)`

🔲 **Add non-blocking API** - Implement `app.poll_event()` for callback-based frontends (Helix/Neovim)
  - Should multiplex main + background channels like `event_recv()` does

#### Target Event Structure

```rust
pub enum Event {
    Internal(InternalEvent),        // Core handles via app.handle_internal_event()
    Frontend(FrontendEvent),        // Frontend handles directly
}

pub enum InternalEvent {
    App(AppEvent),
    Background(BackgroundProcessingEvent),
}

pub enum FrontendEvent {
    LaunchEditor((PathBuf, usize)),
    ExitAndReplace(ExitAndReplaceState),
    Rerender,
}
```

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

## Frontend Implementation Patterns

### Event Loop Mode (Ratatui, standalone TUI)

```rust
loop {
    let result = tokio::select! {
        Some(Ok(event)) = input_stream.next() => {
            app.handle_key_event(event.into())
        }
        Some(event) = app.event_recv() => {
            match event {
                Event::Internal(e) => app.handle_internal_event(e),
                Event::Frontend(FrontendEvent::LaunchEditor((f, l))) => { /* ... */ }
                Event::Frontend(FrontendEvent::ExitAndReplace(s)) => return Ok(Some(s)),
                Event::Frontend(FrontendEvent::Rerender) => EventHandlingResult::Rerender,
            }
        }
    };

    match result {
        EventHandlingResult::Rerender => render(app)?,
        EventHandlingResult::Exit(r) => return Ok(r),
        EventHandlingResult::None => {}
    }
}
```

### Callback Mode (Helix, Neovim plugins)

```rust
impl HelixPlugin {
    pub fn on_key(&mut self, key: HelixKey) {
        match self.app.handle_key_event(key.into()) {
            EventHandlingResult::Rerender => self.render(),
            // ...
        }
    }

    pub fn on_tick(&mut self) {
        while let Some(event) = self.app.poll_event() {  // Non-blocking
            match event {
                Event::Internal(e) => self.app.handle_internal_event(e),
                Event::Frontend(FrontendEvent::Rerender) => self.render(),
                // ...
            }
        }
    }
}
```

**Key difference:** `event_recv()` is async/blocking, `poll_event()` is non-blocking.

---

## Success Criteria

**Frontend responsibilities:**
- Translate input to `KeyEvent` → call `app.handle_key_event()`
- Receive events via `app.event_recv()` or `app.poll_event()`
- Call `app.handle_internal_event()` for `Event::Internal`
- Handle `Event::Frontend` variants directly
- Call `app.view()` to get render state
- Render using their UI framework

**What frontends never touch:**
- Event receivers (owned by App)
- Internal state (`current_screen`, `search_fields`, etc.)
- Search/replace orchestration
- Background processing channels

**Shared across all frontends:**
- Key bindings and search config
- Search/replace logic and diff generation

**Frontend-specific:**
- Rendering, layout, syntax highlighting, editor integration

---

## Open Questions

1. **Config ownership:** Should `PreviewConfig` (themes) and `StyleConfig` stay in core?
   - **Lean toward:** Keep in core for consistency, mark as optional for frontends

2. **LaunchEditor event:** Should this be optional/configurable?
   - **Lean toward:** Yes, editor plugins can disable it

3. **Syntect dependency:** Keep in core or move entirely to frontends?
   - **Depends on:** Config decision above
