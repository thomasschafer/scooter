# Multiline TODO

This file tracks remaining work and decision points for the multiline feature set.

## UX enhancements

- [x] Interpret escape sequences in replacement text.
Notes: `\n` → newline, `\r` → carriage return, `\t` → tab, `\\` → literal backslash. Config key `search.interpret_escape_sequences` default `false`. Added toggle command + toast. `interpret_escapes()` in `replace.rs` with tests.

- [x] Keyboard shortcut to toggle multiline.
Notes: `Alt+M` via `toggle_multiline` in `KeysSearch`, command in `commands.rs`, handler in `app.rs`, help menu entry, TUI test in `app_runner.rs`.

- [x] Hint when `\n` detected in search regex and multiline is off.
Notes: One‑time hint, ripgrep‑style.

- [x] `--interpret-escape-sequences` CLI flag for headless mode.
Notes: `-e`/`--interpret-escape-sequences`, plumbed to headless + TUI config. Tests in `headless.rs` and `e2e-tests.nu`.

- [ ] Enforce line‑by‑line vs multiline across all modes.
Notes: Without multiline, always use line‑by‑line (stdin + files, headless + TUI, all regex modes). With multiline, always use whole‑text matching.

- [x] Document CLI `-e/--interpret-escape-sequences`.
Notes: README mentions config + toggle, but not CLI usage.

- [x] Improve multiline failure messaging.
Notes: When multiline read fails (e.g., non‑UTF‑8), surface a clearer, actionable error.

## Test coverage

- [x] E2E tests for multiline CLI flag (`tests/e2e-tests.nu`).
Notes: `-U`/`--multiline` with stdin + file input, fixed strings, immediate TUI mode, and negative case when multiline is off.

- [x] Stdin + multiline e2e tests.
Notes: Covered in `test_multiline_flag`.

- [x] TUI macro for multiline on/off.
Notes: `test_with_multiline_modes!` in `scooter/tests/utils.rs`.

- [x] Fixed strings + multiline tests.
Notes: `test_text_fixed_strings_multiline_*` in `scooter/tests/headless.rs`.

- [x] Comprehensive mode combination coverage.
Notes: Matrix should assert meaningful behavior in each case (e.g., multiline ON matches across lines, multiline OFF does not, escape sequences ON are interpreted, OFF are literal).

- [x] Matrix tests: make each cell assert both multiline + escape behavior.
Notes: Four variants, try to keep things DRY.
  - [x] TUI files matrix (3×2×2).
  Notes: Update existing tests to assert both behaviors per cell.
  - [x] TUI stdin matrix (3×2×2).
  Notes: New tests; assert both behaviors per cell.
  - [x] Headless stdin matrix (3×2×2).
  Notes: Update existing tests to assert both behaviors per cell.
  - [x] Headless files matrix (3×2×2).
  Notes: New tests; assert both behaviors per cell.

- [x] TUI CRLF multiline replacement coverage.
Notes: Add a TUI test that replaces across `\r\n` boundaries to ensure line endings are preserved end‑to‑end.

## Not doing (but could return to in future)

- [ ] Multiline status indicator in the TUI.
Notes: Add a visual mode badge similar to other toggles.

- [ ] Large file size guard for multiline mode.
Notes: `search_file` and `replace_all_in_file` read entire files when multiline is enabled. Consider a configurable max size (default to `MAX_FILE_SIZE`), with opt‑out for power users.

- [ ] Document multiline UTF‑8 requirement.
Notes: Multiline reads full files via `read_to_string` and fails on non‑UTF‑8; line‑by‑line mode unchanged. Add README note or improve error message.

---

TODO: Remove this file before merging to main.
