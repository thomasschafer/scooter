# Multiline TODO

## UX enhancements

### ~~Interpret escape sequences in replacement text~~ DONE
Implemented with config option and toggleable command:
- `\n` → newline, `\r` → carriage return, `\t` → tab, `\\` → literal backslash
- Config key: `search.interpret_escape_sequences` (boolean, default: false)
- Added `ToggleInterpretEscapeSequences` command (unbound by default, can be bound via config)
- Handler in `app.rs` shows "Escape sequences: ON/OFF" toast and triggers re-search
- `interpret_escapes()` function in `replace.rs` with comprehensive tests

### ~~Keyboard shortcut to toggle multiline~~ DONE
Implemented with `Alt+M`:
- Added `toggle_multiline` to `KeysSearch` in `keys.rs`
- Added `ToggleMultiline` command in `commands.rs`
- Added handler in `app.rs` (toggles `run_config.multiline`, triggers re-search)
- Added to help menu
- Added `test_toggle_multiline_keybinding` test in `app_runner.rs`

### ~~Help message when `\n` detected in search regex~~ DONE
When user enters a pattern containing `\n` (or `\r\n`) but multiline mode is off:
- Show a hint suggesting they enable multiline mode
- Similar to ripgrep's behavior
- Only show hint once per session or dismissable

### ~~`--interpret-escape-sequences` CLI flag for headless mode~~ DONE
Added `-e` / `--interpret-escape-sequences` CLI flag:
- Flag in `Args` struct, passed through to both `search_config_from_args` (headless) and `AppRunConfig` (TUI)
- CLI flag overrides config file setting (either source can enable it)
- Integration tests in `headless.rs` covering newline, tab, disabled, and file replacement
- E2E tests in `e2e-tests.nu` covering stdin and file replacement with the flag

### Convert to line-by-line find and replace in headless mode
Update `find_and_replace_text` to always search line-by-line when not in multiline, and to always
operate across lines when mulitline is enabled.

## Test coverage

### ~~E2E tests for multiline CLI flag~~ DONE
Added `test_multiline_flag` in `tests/e2e-tests.nu` covering:
- `--multiline` / `-U` flag with stdin input
- `--multiline` with `--no-tui` mode and file input
- Multiline with `--fixed-strings`
- Multiline in TUI immediate mode
- Verification that cross-line patterns don't match without `--multiline`

### ~~Stdin + multiline e2e tests~~ DONE
Covered in `test_multiline_flag` above.

### ~~TUI test macro for multiline~~ DONE
Added `test_with_multiline_modes!` macro in `scooter/tests/utils.rs` - multiline on/off (2 variants).
Can be expanded with additional combination macros (e.g. advanced_regex × multiline) as needed.

### ~~Fixed strings + multiline tests~~ DONE
Added tests in `scooter/tests/headless.rs`:
- `test_text_fixed_strings_multiline_basic` - basic fixed string match with multiline on/off
- `test_text_fixed_strings_multiline_literal_newline` - literal newline in fixed string search pattern
- `test_text_fixed_strings_multiline_no_match_across_lines` - verifies no cross-line match without multiline
- `test_text_fixed_strings_multiline_regex_chars_literal` - regex metacharacters treated literally

### Comprehensive mode combination coverage
Ensure tests cover the key combinations:
- Multiline on/off
- Advanced regex on/off
- Fixed strings on/off

Not all 8 combinations are meaningful (e.g., fixed_strings makes regex mode irrelevant), but the valid combinations should be tested. The new macros make this straightforward to expand.

## Not doing (but could return to in future)

### Multiline status indicator
Add visual indicator at bottom of TUI screen showing multiline mode is active (similar to other mode indicators).

### Large file size guard for multiline mode
Both `search_file` (search.rs) and `replace_all_in_file` (replace.rs) read the entire file into memory
when multiline is enabled, with no file size limit. Should add a configurable `max_file_size` to the
config system (defaulting to the existing 100MB `MAX_FILE_SIZE` constant). Power users can increase
or disable this limit via config.

---

TODO: Remove this file before merging to main
