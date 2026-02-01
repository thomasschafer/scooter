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

### Help message when `\n` detected in search regex
When user enters a pattern containing `\n` (or `\r\n`) but multiline mode is off:
- Show a hint suggesting they enable multiline mode
- Similar to ripgrep's behavior
- Only show hint once per session or dismissable

## Test coverage

### E2E tests for multiline CLI flag
Add explicit tests in `tests/e2e-tests.nu` for:
- `--multiline` / `-U` flag with file input
- `--multiline` with `--no-tui` mode
- Multiline replacement producing correct output

### Stdin + multiline e2e tests
Add tests for piped input with multiline flag:
- `echo "foo\nbar" | scooter -U -s "o\nb" -r "x" -N`

### TUI test macro for multiline
Current macros only cover:
- `test_with_both_regex_modes!` - advanced_regex on/off
- `test_with_both_regex_modes_and_fixed_strings!` - advanced_regex × fixed_strings (4 combos)

Need a new macro or expand existing to include multiline dimension. Consider:
- `test_with_multiline_modes!` - multiline on/off
- Or expand to test all relevant combinations

### Fixed strings + multiline tests
Add tests for fixed_strings mode with multiline enabled:
- Literal `\n` in search pattern (not interpreted as newline)
- Multiline fixed string matching
- Ensure fixed_strings + multiline interaction is correct

### Comprehensive mode combination coverage
Ensure tests cover the key combinations:
- Multiline on/off
- Advanced regex on/off
- Fixed strings on/off

Not all 8 combinations are meaningful (e.g., fixed_strings makes regex mode irrelevant), but the valid combinations should be tested.

## Not doing (but could return to in future)

### Multiline status indicator
Add visual indicator at bottom of TUI screen showing multiline mode is active (similar to other mode indicators).
