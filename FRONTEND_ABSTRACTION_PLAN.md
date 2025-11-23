# Frontend abstraction plan

## Status

Ready to begin Phase 1 - view state abstraction.

## Goal

Make `scooter-core` a truly frontend-agnostic library with a minimal public API that allows easy implementation of new frontends (Helix plugin, Neovim plugin, web UI, etc.).

## Current state

### What's working well
- Core business logic (search, replace, diff) is independent of UI frameworks
- `App` orchestrates all operations cleanly
- Keyboard abstraction exists to share config across implementations

### Current problems

1. `App` exposes internal state - fields like `current_screen`, `search_fields`, `searcher` are public, forcing frontends to understand internal structure
2. Event passthrough required - frontends receive `Event::App` and `Event::PerformReplacement` and must call methods back on `App`
3. No view abstraction - frontends pattern match on `Screen` enum and access nested state directly
4. UI mutates core state - frontend updates `view_offset` and `num_displayed` based on viewport size

## Architecture vision

scooter-core should expose:
- Key event handling (shared config across frontends)
- View state objects (read-only snapshots for rendering)
- Event consumption via async `event_recv()`
- Pure business logic functions

Frontends should:
- Translate input to `KeyEvent` and call `app.handle_key_event()`
- Get view state via `app.view()` and render it
- Handle async events via `event_recv()`
- Use syntax highlighting from core (syntect, for now)

---

## Implementation plan

### Phase 1: View state abstraction

Goal: Hide internal state and expose immutable view snapshots.

#### Changes

1. Create view state structs in `scooter-core/src/view.rs`:

```rust
pub struct AppView<'a> {
    pub input_source: &'a InputSource,
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
    pub results: &'a [SearchResultWithReplacement],
    pub primary_selected_idx: usize,
    pub selected_indices: SelectedIndices<'a>,  // Helper for is_selected()
    pub view_offset: usize,  // TODO: Consider computed visible_window() approach
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
    pub started_at: Instant,
}

pub struct ResultsView<'a> {
    pub num_successes: usize,
    pub num_ignored: usize,
    pub errors: &'a [SearchResultWithReplacement],
    pub scroll_offset: usize,
}
```

2. Add view accessor:

```rust
impl App {
    pub fn view(&self) -> AppView<'_>;
}
```

3. Make internal types private/`pub(crate)`:

All types should be private by default. Explicitly make these currently-public types private:

```rust
pub struct App {
    config: Config,                       // Private - access via view.config
    current_screen: Screen,               // Private - access via view.view
    search_fields: SearchFields,          // Private - access via view
    searcher: Option<Searcher>,           // Private
    input_source: InputSource,            // Private - access via view.input_source
    event_sender: UnboundedSender<Event>, // Keep public (for syntax highlighting)
    event_receiver: UnboundedReceiver<Event>, // Private
    // ... all other fields private
}

// Hide internal types from frontends
pub(crate) enum Screen { ... }
pub(crate) struct SearchState { ... }
pub(crate) struct SearchFields { ... }
pub(crate) enum Selected { ... }
// ... etc

// Only expose what's needed in view
pub struct TextField { ... }
pub struct CheckboxField { ... }
pub enum FocussedSection { ... }
```

4. Remove `num_displayed` from core state:

`SearchState::num_displayed` is purely a UI concern and shouldn't be in core. Remove it and pass viewport dimensions as needed when rendering.

5. Update `scooter/src/ui/view.rs`:

Replace `app.current_screen` and `app.search_fields` with `app.view()`.

Breaking changes: Major

---

### Phase 2: Documentation

Goal: Make it trivial to implement a new frontend.

#### Deliverables

1. `scooter-core/FRONTEND_GUIDE.md`:
   - Architecture overview
   - Step-by-step implementation guide
   - Event loop vs callback mode patterns
   - View rendering examples
2. `examples/minimal_frontend.rs`:
   - Bare-bones frontend (no ratatui)
   - Shows complete event loop
   - Demonstrates view rendering
3. API documentation:
   - Document all public items in scooter-core
   - Add examples to key types
   - Clarify frontend vs core responsibilities
4. `scooter-core/README.md`:
   - Explain crate purpose
   - Link to frontend guide
   - List example frontends

Breaking changes: None

---

## Optional future phases

These are deferred until needed (e.g., when implementing editor plugins):

### Extract rendering utilities

Goal: Move syntax highlighting to frontends.

Status: Keeping syntax highlighting in `scooter-core` for now. Revisit when implementing editor plugins.

Changes would include:
- Move `read_lines_range_highlighted()` to frontend
- Make `syntect` optional in scooter-core
- Move highlighting cache to frontend

Breaking changes: Minor

---

### Field state cleanup

Goal: Ensure text field logic is clean.

#### Review areas

1. `TextField` - verify keyboard handling is minimal
2. `CheckboxField` - verify no rendering logic
3. `SearchFields` - verify no UI-specific code

Breaking changes: Minor or none

---

### Config documentation

Goal: Document which config fields are core vs frontend-specific.

#### Current config structure

```rust
pub struct Config {
    pub editor_open: EditorOpenConfig,  // Core ✅
    pub search: SearchConfig,           // Core ✅
    pub keys: KeysConfig,               // Core ✅
    pub preview: PreviewConfig,         // Default TUI settings
    pub style: StyleConfig,             // Default TUI settings
}
```

#### Decision

Keep all config in core for now. Document which fields are core vs TUI-specific:
- `preview` (syntax theme, wrap text) - useful to share, but frontends can ignore
- `style` (true_color) - potentially useful for non-TUI frontends too
- Config is currently mutable at runtime (e.g., toggling `wrap_text`)

Future consideration: Potential fields to extract when implementing editor plugins:
- Syntax highlighting theme (if highlighting moves to frontends)
- Text wrapping default (editor plugins may have their own config)
- True color setting (may vary by frontend)

Breaking changes: None (documentation only)

---

## Frontend implementation patterns

### Event loop mode (Ratatui TUI)

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
        EventHandlingResult::Rerender => {
            let view = app.view();
            render(&view)?;
        }
        EventHandlingResult::Exit(r) => return Ok(r),
        EventHandlingResult::None => {}
    }
}
```

Note: Editor plugin patterns will be determined when implementing Helix/Neovim support.

---

## Success criteria

Frontend responsibilities:
- Translate input to `KeyEvent` → call `app.handle_key_event()`
- Receive events via `app.event_recv()`
- Call `app.handle_internal_event()` for `Event::Internal`
- Handle `Event::Frontend` variants directly
- Call `app.view()` to get immutable render state
- Render using their UI framework

What frontends never touch:
- Event receivers (owned by App)
- Internal state (`Screen`, `SearchState`, `SearchFields`, etc. - all private/`pub(crate)`)
- Search/replace orchestration
- Background processing channels

Shared across all frontends:
- Key bindings and search config
- Search/replace logic and diff generation
- Syntax highlighting (via core, for now)

Frontend-specific:
- Rendering, layout, viewport management, editor integration

---

## Decisions made

1. Config ownership: All config stays in core. `PreviewConfig` and `StyleConfig` are marked as "default TUI settings" that frontends can ignore. Config remains mutable at runtime.
2. Syntax highlighting: Stays in core for now using syntect. Revisit when implementing editor plugins (Phase 2 deferred).
3. Non-blocking events: No `poll_event()` for now. Defer until implementing Helix/Neovim plugins and determine actual needs.
4. Viewport concerns: `num_displayed` removed from core. `view_offset` stays for now but marked for potential refactoring to computed approach.
5. Event sender: Remains public for now (needed for syntax highlighting cache spawning background tasks).
