# Frontend abstraction plan

## Status

Ready to begin Phase 0 — runtime and effect decoupling.

## Goal

Make `scooter-core` the single owner of Scooter's search/replace workflow, while exposing a clean, frontend-agnostic API that allows very thin frontends:

- the existing ratatui TUI
- `scooter.hx` / Helix integration
- future Neovim or other editor plugins

Frontends should render UI, translate user input into core actions, and handle a small set of typed side effects. They should not reimplement:

- search/replace state machines
- selection logic
- replacement preview generation
- search result management
- background task orchestration
- file I/O coordination

Breaking changes inside `scooter-core` are acceptable for this work. Update all internal consumers together.

## What the repo already gets right

- `scooter-core` already owns the real application state machine
- search, replace, diff, validation, and file processing logic are already centralized
- the `scooter` crate is already mostly a frontend: terminal setup, event ingestion, and ratatui rendering
- keyboard abstraction already exists and can remain as a TUI convenience layer

This means the refactor should not move "more logic into core" in a vague sense. Most of the logic is already there. The real work is to expose the right public API and remove the remaining TUI-shaped coupling.

## Problems to solve

### 1. Internal state is the API

Today frontends can only do useful work by understanding internal types such as `Screen`, `SearchState`, `SearchFields`, `Selected`, and `UIState`.

That is the main structural problem. It makes the TUI easy to write, but it prevents plugins from using `App` as a clean controller.

### 2. The public control surface is too TUI-shaped

The current app is primarily driven through `handle_key_event()`. That works for the TUI, but plugin frontends need to drive Scooter with explicit operations or frontend-neutral actions, not terminal keybindings.

### 3. Runtime coupling is still embedded in core

`App` currently owns debouncing, background scheduling, timers, and internal async event flow. A plugin frontend should not need to know or care whether the implementation uses Tokio.

This is the highest-priority coupling to remove. A nice view API alone is not enough if the frontend still needs to provide a Tokio-shaped environment.

### 4. TUI viewport state leaks into core

`view_offset` and `num_displayed` are currently tied to ratatui rendering behavior. Those are frontend viewport concerns, not domain state.

Core should own logical selection, not screen scroll position.

### 5. Side effects are not modeled clearly enough

Things like opening an editor, exiting with replacement results, showing a toast, or requesting rerender are currently expressed through ad hoc event channels and `EventHandlingResult`.

Plugin frontends need a typed side-effect surface they can consume directly.

## Architecture target

`scooter-core` should expose three stable surfaces:

### 1. A controller API

`App` remains the stateful controller for a Scooter session.

It should expose:

- construction from explicit session init data
- frontend-neutral actions or direct operation methods
- background progress polling / effect draining
- immutable view snapshots

### 2. A typed effect API

Core should emit typed effects for frontend-specific behavior.

Examples:

- open a file at a line
- return stdin replacement payload
- return replacement stats
- show a toast or message
- request rerender

Frontends consume effects and decide what to do with them.

### 3. A stable view API

Frontends should render from immutable DTO-style view structs.

The public view API should expose:

- screen/state summary
- field values and validation state
- results and selection state
- replacement progress
- final results state
- popup/toast/message state

The public view API should not expose internal widget models or TUI-specific state.

## Non-goals

- no Helix-specific or Steel-specific code in this repo
- no ratatui types in `scooter-core`
- no plugin-specific rendering helpers in the core public API
- no requirement that plugin frontends adopt terminal-style keymaps or viewport behavior

## Phase 0: Runtime and effect decoupling

**Goal:** remove the remaining Tokio/event-loop coupling from the public integration story before designing the final frontend API.

This phase comes first because a plugin-friendly view API is not enough if the frontend still has to participate in the current internal async orchestration model.

### 0.1 - Introduce a frontend-facing effect API

Create a typed effect surface for side effects that frontends must handle explicitly.

```rust
pub enum AppEffect {
    RequestRender,
    OpenLocation {
        path: PathBuf,
        line: usize,
    },
    ExitWithStats(ReplaceState),
    ExitAndReplaceStdin(ExitAndReplaceState),
    ShowToast {
        message: String,
    },
}
```

The exact names can change, but the shape should be explicit and typed.

Important rules:

- effects are frontend-facing
- effects are drained from `App`
- effects replace the need for frontends to understand internal event plumbing

### 0.2 - Replace boolean polling with effect draining

Do not use a frontend API like `poll_background_events() -> bool`.

That loses too much information and forces frontends to separately reconstruct what happened.

Instead expose something like:

```rust
impl App {
    pub fn pump(&mut self) -> Vec<AppEffect>;
}
```

or:

```rust
impl App {
    pub fn drain_effects(&mut self) -> Vec<AppEffect>;
    pub fn poll(&mut self);
}
```

The exact split is flexible, but the outcome is not:

- frontends can advance core state without owning an async `select!` loop
- frontends can read all pending effects directly

### 0.3 - Introduce an internal scheduling abstraction

Core currently uses Tokio timers and tasks directly. That should no longer be the frontend contract.

Introduce an internal scheduler/executor abstraction, for example:

```rust
pub trait AppRuntime: Send + Sync + 'static {
    fn spawn(&self, job: AppJob);
    fn schedule(&self, delay: Duration, job: AppJob);
}
```

or an equivalent internal abstraction.

The important part is the design boundary:

- `App` can still use async/background work internally
- frontends must not need to provide or understand Tokio-specific mechanics

For the first implementation it is acceptable for `scooter-core` to ship a default Tokio-backed runtime implementation internally, as long as the public API no longer exposes Tokio/event-loop assumptions.

### 0.4 - Keep `handle_key_event()` as a compatibility layer

The TUI should continue to use key handling for convenience.

But `handle_key_event()` becomes a thin adapter:

- translate key input into frontend-neutral actions
- dispatch through the same controller API plugins use

That keeps behavior unified and prevents divergence between TUI and plugin flows.

## Phase 1: Stable controller API

**Goal:** make `App` directly usable by non-TUI frontends without exposing internal state structs.

### 1.1 - Replace the narrow constructor proposal with explicit session init data

Do not reduce construction to `Config + directory`.

That would lose existing capabilities:

- stdin input mode
- initial/prepopulated field values
- immediate search / immediate replace behavior
- other session-scoped options

Instead introduce a single initialization struct:

```rust
pub struct AppInit<'a> {
    pub config: Config,
    pub input_source: InputSource,
    pub initial_fields: SearchFieldValues<'a>,
    pub session: SessionOptions,
}

pub struct SessionOptions {
    pub include_hidden: bool,
    pub include_git_folders: bool,
    pub advanced_regex: bool,
    pub multiline: bool,
    pub interpret_escape_sequences: bool,
    pub immediate_search: bool,
    pub immediate_replace: bool,
    pub print_results: bool,
    pub print_on_exit: bool,
}

impl App {
    pub fn new(init: AppInit<'_>) -> anyhow::Result<Self>;
}
```

Names can change, but construction should remain explicit and complete.

### 1.2 - Expose frontend-neutral actions

Do not stop at a tiny set of direct methods like `start_search()` and `toggle_all()`.

That would still force plugin frontends to reimplement selection and navigation behavior.

Use one of these two designs:

- a public `AppAction` enum plus `dispatch(action)`
- a rich set of direct methods that fully covers the workflow

The preferred direction is an action enum because it keeps the surface coherent and easy to extend.

Example:

```rust
pub enum AppAction {
    StartSearch,
    CancelSearch,
    StartReplace,
    CancelReplace,
    Reset,
    SetField {
        field: SearchFieldId,
        value: FieldUpdate,
    },
    FocusSection(FocusTarget),
    MoveSelection(SelectionMotion),
    ToggleSelectionInclusion,
    ToggleAllInclusion,
    ToggleMultiselect,
    FlipMultiselectDirection,
    OpenSelected,
    DismissPopup,
}

impl App {
    pub fn dispatch(&mut self, action: AppAction) -> anyhow::Result<Vec<AppEffect>>;
}
```

This lets:

- plugins call explicit actions
- the TUI map keybindings to the same actions

If you keep some direct methods as ergonomic helpers, they should delegate to `dispatch`.

### 1.3 - Define field identifiers and updates explicitly

Plugins should not need access to internal `SearchFields` or widget implementations.

Expose stable IDs and updates instead:

```rust
pub enum SearchFieldId {
    Search,
    Replace,
    FixedStrings,
    MatchWholeWord,
    MatchCase,
    IncludeFiles,
    ExcludeFiles,
}

pub enum FieldUpdate {
    SetText(String),
    InsertText(String),
    SetChecked(bool),
    Toggle,
}
```

That gives frontends a clean way to control state without depending on terminal-editing internals.

## Phase 2: Stable view API

**Goal:** frontends render from immutable snapshots and never inspect internal app structs directly.

### 2.1 - Add `scooter-core/src/view.rs`

Create DTO-style public view structs.

These are snapshots of renderable state, not references to internal field widgets.

```rust
pub struct AppView {
    pub screen: ScreenView,
    pub popup: Option<PopupView>,
    pub toast: Option<ToastView>,
}

pub enum ScreenView {
    Search(SearchScreenView),
    PerformingReplacement(PerformingReplacementView),
    Results(ResultsView),
}

pub struct SearchScreenView {
    pub fields: Vec<FieldView>,
    pub focus: FocusView,
    pub search: Option<SearchProgressView>,
}

pub struct FieldView {
    pub id: SearchFieldId,
    pub kind: FieldKindView,
    pub label: String,
    pub disabled: bool,
    pub error: Option<FieldErrorView>,
}

pub enum FieldKindView {
    Text {
        text: String,
        cursor_column: usize,
    },
    Checkbox {
        checked: bool,
    },
}

pub struct SearchProgressView {
    pub status: SearchStatusView,
    pub results: Vec<SearchResultView>,
    pub selection: SelectionView,
    pub preview_update: Option<PreviewUpdateView>,
}

pub enum SearchStatusView {
    NotStarted,
    InProgress {
        started_at: Instant,
    },
    Complete {
        started_at: Instant,
        completed_at: Instant,
    },
}

pub struct SelectionView {
    pub primary_index: Option<usize>,
    pub selected_indices: Vec<usize>,
    pub multiselect: bool,
}

pub struct SearchResultView {
    pub id: SearchResultId,
    pub path: Option<PathBuf>,
    pub start_line: usize,
    pub end_line: usize,
    pub included: bool,
    pub match_text: String,
    pub replacement_text: String,
    pub preview_error: Option<String>,
    pub replace_result: Option<ReplaceResultView>,
}

pub struct PerformingReplacementView {
    pub completed: usize,
    pub total: usize,
}

pub struct ResultsView {
    pub num_successes: usize,
    pub num_ignored: usize,
    pub errors: Vec<ReplacementErrorView>,
}
```

The exact shapes can change, but the principles should not:

- public views are stable DTOs
- they do not expose `TextField`, `CheckboxField`, `Screen`, `SearchState`, or `UIState`
- they do not include TUI viewport state

### 2.2 - `App::view()` returns immutable snapshots

Expose:

```rust
impl App {
    pub fn view(&self) -> AppView;
}
```

Returning owned DTOs is preferable here.

It avoids lifetime-heavy public APIs and makes FFI or editor-plugin bridges easier.

### 2.3 - Remove viewport state from core

Remove TUI viewport concerns from core state:

- `num_displayed`
- `view_offset`

These belong in each frontend.

Core should still provide enough logical selection data for a frontend to compute its own viewport.

### 2.4 - Keep editing widget internals private

`TextField`, `CheckboxField`, `SearchFields`, `Selected`, `SearchState`, `Screen`, `Popup`, and `UIState` should become internal implementation details unless a type is genuinely needed as a stable frontend-facing concept.

In particular:

- `TextField` should not be part of the public view API
- `CheckboxField` should not be part of the public view API
- `SearchFields` should not be public
- `Screen` should not be public

## Phase 3: Preview and result APIs

**Goal:** expose search results and replacement previews in a frontend-neutral way without putting app-context-dependent behavior on plain result values.

### 3.1 - Do not put preview generation on `SearchResultWithReplacement`

Avoid a public API like:

```rust
impl SearchResultWithReplacement {
    pub fn build_preview(&self, context_lines: usize) -> Vec<PreviewLine>;
}
```

That method would be misleading because preview generation depends on app-owned context:

- current search configuration
- current input source
- file content provider
- replacement caching
- advanced-regex haystack behavior

Instead preview generation should remain app/controller-owned.

### 3.2 - Expose preview generation through `App`

Use an API like:

```rust
pub struct PreviewOptions {
    pub context_lines: usize,
}

pub struct PreviewView {
    pub lines: Vec<PreviewLineView>,
}

impl App {
    pub fn preview_for(
        &self,
        result_id: SearchResultId,
        options: PreviewOptions,
    ) -> anyhow::Result<PreviewView>;
}
```

This keeps preview generation:

- consistent across frontends
- correctly tied to the current app/session context
- reusable by both TUI and plugins

### 3.3 - Give results stable frontend-facing IDs

If frontends need to target specific results for preview, inclusion toggling, or navigation, expose a stable `SearchResultId` instead of making them rely on internal vector positions as the only identity.

Indices may still be used in views for display ordering, but the controller API should have a real ID type available.

## Phase 4: TUI migration

**Goal:** migrate the ratatui frontend to the new controller/view/effect surfaces and verify the design before external plugin adoption.

### 4.1 - TUI input path

Keep:

- crossterm event ingestion
- terminal setup and teardown
- keybinding config

Change:

- translate key events to `AppAction`
- dispatch through the same controller API plugins use
- drain `AppEffect`s rather than reaching into internal core state/events

### 4.2 - TUI rendering path

Replace direct reads of app internals with `app.view()`.

The ratatui renderer should only consume public view structs.

### 4.3 - TUI viewport ownership

Move scroll position and page sizing fully into the `scooter` crate.

The TUI should compute:

- how many results fit
- which slice is visible
- how the viewport tracks the selected result

That logic should no longer mutate core state during render.

## Phase 5: Documentation and examples

**Goal:** make frontend implementation straightforward without reading `scooter-core` internals.

### 5.1 - Add `scooter-core/FRONTEND_GUIDE.md`

Document:

- architecture overview
- what `App` owns
- what a frontend owns
- action dispatch
- effect handling
- view rendering
- preview generation

Include two integration patterns:

- TUI/event-driven frontend
- plugin/frame-polled frontend

### 5.2 - Add `examples/minimal_frontend.rs`

Create a tiny non-ratatui example that:

- constructs `App`
- dispatches actions directly
- pumps/drains effects
- renders basic output from `AppView`

This should be the canonical "thin frontend" example.

### 5.3 - Add API docs to all public frontend-facing items

Document:

- `AppInit`
- `SessionOptions`
- `AppAction`
- `AppEffect`
- `AppView`
- preview APIs
- result IDs / field IDs

## Cleanup rules during the refactor

As the work proceeds:

- remove any public exposure that exists only for the TUI's current implementation
- do not preserve public API just because it is already there
- prefer small, stable public DTOs over references to internal structs
- prefer one coherent action/effect model over multiple partially overlapping entry points
- keep ratatui-specific concerns in `scooter`, not `scooter-core`

## Deferred / future

### Syntax highlighting ownership

Syntax highlighting can remain in core for now if both the TUI and editor plugins benefit from reusing it.

Revisit later if editor integrations want to defer entirely to editor-native highlighting.

### Config relevance per frontend

Not every config field applies equally to every frontend.

That is fine.

Examples:

- `editor_open` is relevant to the terminal app, but likely irrelevant to an editor plugin
- keybinding config is relevant to the TUI, but not to editor-native bindings
- preview and search behavior may be shared

The frontend guide should document which config areas are commonly relevant, but this does not need to block the abstraction work.

## Success criteria

After this work, a new frontend should only need to:

1. Construct `App` from explicit init data
2. Translate frontend input into `AppAction` values, or call equivalent direct helpers
3. Advance the app and drain `AppEffect`s without owning Scooter's internal async model
4. Render from `app.view()` only
5. Request previews through the public preview API
6. Handle frontend-specific effects such as open-location or exit behavior

It should not need to:

- inspect `Screen`, `SearchState`, `SearchFields`, `Selected`, or `UIState`
- participate in a Tokio `select!` loop
- manage replacement preview logic itself
- implement its own search/replace lifecycle
- own core selection semantics
- know anything about ratatui
