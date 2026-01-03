# TUI Multiline Search & Replace Implementation Plan

**Status**: Phase 2 Ready to Start
**Last Updated**: 2026-01-03

---

## Overview

This plan addresses the missing TUI functionality for multiline search and replace. While multiline search works in headless mode, the TUI has several critical bugs that prevent it from working correctly.

## Issues Found

1. ✅ **Replacement execution** - Fixed in Phase 1
2. ✅ **Conflict detection** - Fixed in Phase 1
3. ⏳ **Within-line replacement** - Phase 2 (in progress)
4. ⏳ **Preview validation** - Phase 3 (TUI only)
5. ⏳ **Preview rendering** - Phase 4 (TUI only)
6. ⏳ **Results display** - Phase 5 (TUI only)

---

## Phase 1: Fix Replacement Execution + Conflict Detection

**Status**: ✅ IMPLEMENTATION COMPLETE - Ready for manual TUI testing

**Goal**: Make replacements actually work with proper conflict handling

**Summary**: Implemented streaming replacement logic that handles both single-line and multiline matches in a unified code path. Uses O(n) upfront conflict detection and consumes lines from iterator as needed for multiline matches.

### Unit Tests Added

**✅ COMPLETE**: All 12 comprehensive unit tests added and passing:
- ✅ Single-line replacements still work (`test_mixed_single_and_multiline`)
- ✅ Non-overlapping multiline matches work (`test_non_overlapping_multiline_replacements`)
- ✅ Conflict scenario: 9-11, 10-13, 12-12 (2nd fails, others succeed) (`test_conflict_scenario_9_11_10_13_12_12`)
- ✅ Adjacent non-overlapping: 1-3, 4-6 (both succeed) (`test_adjacent_non_overlapping`)
- ✅ Partial overlap: 1-5, 3-8 (2nd fails) (`test_partial_overlap`)
- ✅ Single-line between multiline: 1-3, 2-2, 4-6 (middle fails) (`test_single_line_between_multiline`)
- ✅ Multiline at end of file (`test_multiline_at_end_of_file`)
- ✅ Multiple multiline matches with gaps (`test_multiple_multiline_with_gaps`)
- ✅ File validation failure (lines changed since search) (`test_file_changed_multiline_validation`)
- ✅ File too short (EOF before expected) (`test_file_too_short_multiline`)
- ✅ Single multiline replacement (`test_single_multiline_replacement`)
- ✅ Overlapping ranges (`test_conflict_overlapping_ranges`)

### Byte-indexed Conflict Detection

**Status**: ✅ COMPLETE
**Goal**: Support multiple matches on same line by tracking byte offsets

**Key changes**:
- Added `byte_offsets: Option<(usize, usize)>` to SearchResult
- `Some((start, end))` = multiline mode with byte tracking
- `None` = line-mode (spans whole line)
- Updated conflict detection with dual-mode logic (byte-level or line-level)

**Tests**:
- ✅ 19 conflict detection tests covering all mode combinations
- ✅ 5 TDD tests written for Phase 2 (currently ignored)

---

## Phase 2: Within-line Replacement Implementation

**Status**: ⏳ NOT STARTED - Critical bugs discovered and diagnosed via logging
**Goal**: Enable multiple replacements on the same line using byte offsets

**TL;DR**: Two bugs conspire to make it LOOK like it works, but tracking is broken:
1. **Bug #1 (app.rs)**: `replace_all()` gives every match the full-line replacement string
2. **Bug #2 (replace.rs)**: HashMap keeps only last match per line, but it has the full-line string from Bug #1
3. **Result**: File is correct, but 5/7 matches show errors ("Failed to find search result in file")
4. **Fix**: Calculate individual replacement strings + use Vec instead of HashMap

**Background**:
Phase 1 added byte offsets to `SearchResult` and implemented dual-mode conflict detection. However, there are TWO bugs preventing proper within-line replacement:

### Bug #1: Replacement string calculation (app.rs:816-818)

**Problem discovered via logging**: When multiple matches exist on the same line, `update_replacements` calls `replacement_if_match` with the FULL line content for each match. The function uses `.replace_all()` which replaces ALL occurrences, so every SearchResultWithReplacement gets the same fully-replaced line string.

**Example**: Searching for "it" in "it it it\n  it it it it\n" with replacement "ITREPLACED":
- Match 1 (line 1, bytes 0-1): replacement = "ITREPLACED ITREPLACED ITREPLACED\n"
- Match 2 (line 1, bytes 3-4): replacement = "ITREPLACED ITREPLACED ITREPLACED\n" (same!)
- Match 3 (line 1, bytes 6-7): replacement = "ITREPLACED ITREPLACED ITREPLACED\n" (same!)
- Matches 4-7 on line 2: all get "  ITREPLACED ITREPLACED ITREPLACED ITREPLACED\n"

**Current code flow**:
```rust
// app.rs:816
let content = res.search_result.content(); // Full line(s) content
match replacement_if_match(&content, file_searcher.search(), file_searcher.replace()) {
    Some(replacement) => res.replacement = replacement, // BUG: replace_all replaces ALL matches
    ...
}

// replace.rs:558-560
SearchType::Fixed(fixed_str) => line.replace(fixed_str, replace), // Replaces ALL
SearchType::Pattern(pattern) => pattern.replace_all(line, replace).to_string(), // Replaces ALL
```

### Bug #2: HashMap keyed by line number (replace.rs:348-352)

**Problem**: When multiple matches exist on the same line, the HashMap construction keeps only the LAST match:
```rust
let mut line_map = results
    .iter_mut()
    .filter(|r| r.replace_result.is_none())
    .map(|r| (r.search_result.start_line_number, r))
    .collect::<HashMap<_, _>>();  // BUG: Duplicate keys, only last wins
```

**Why it appears to work**: The last match's replacement string ALREADY contains all replacements (Bug #1), so writing it produces the correct file. But the other matches never get their `replace_result` set, causing "Failed to find search result in file" errors.

**Test results** (from user):
```
Successful replacements (lines): 2  ← Only 2 matches tracked (1 per line)
Errors: 5  ← Other 5 matches not in HashMap
tmp.txt: ITREPLACED ITREPLACED ITREPLACED  ← But file is correct!
```

### Implementation Plan

### Step 1: Update `update_replacements` in app.rs (Fix Bug #1)

**File**: `scooter-core/src/app.rs` line ~816

**Current code**:
```rust
let content = res.search_result.content();
match replacement_if_match(&content, file_searcher.search(), file_searcher.replace()) {
    Some(replacement) => res.replacement = replacement,
    None => return EventHandlingResult::Rerender,
}
```

**New code**:
```rust
let replacement = if let Some((start_byte, end_byte)) = res.search_result.byte_offsets {
    // Multiline/byte-mode: replace only the matched substring
    let content = res.search_result.content();
    let matched_text = &content[start_byte..=end_byte];
    // Apply replacement to just this match
    match file_searcher.search() {
        SearchType::Fixed(_) => file_searcher.replace().to_string(),
        SearchType::Pattern(p) => p.replace(matched_text, file_searcher.replace()).to_string(),
        SearchType::PatternAdvanced(p) => p.replace(matched_text, file_searcher.replace()).to_string(),
    }
} else {
    // Line-mode: replace all occurrences in the line (preserve current behavior)
    let content = res.search_result.content();
    match replacement_if_match(&content, file_searcher.search(), file_searcher.replace()) {
        Some(r) => r,
        None => return EventHandlingResult::Rerender,
    }
};
res.replacement = replacement;
```

**Critical**: This preserves non-multiline behavior where `.replace_all()` is correct!

### Step 2: Refactor `replace_in_file` to handle multiple per line (Fix Bug #2)

**File**: `scooter-core/src/replace.rs` line ~348

**Current code**:
```rust
let mut line_map = results
    .iter_mut()
    .filter(|r| r.replace_result.is_none())
    .map(|r| (r.search_result.start_line_number, r))
    .collect::<HashMap<_, _>>(); // BUG: only keeps last per line
```

**New code**:
```rust
// Build map: line_number -> Vec<replacement> (sorted right-to-left)
let mut line_map: HashMap<usize, Vec<&mut SearchResultWithReplacement>> = HashMap::new();
for result in results.iter_mut().filter(|r| r.replace_result.is_none()) {
    line_map
        .entry(result.search_result.start_line_number)
        .or_default()
        .push(result);
}

// Sort each line's replacements by byte offset in REVERSE (right-to-left)
for replacements in line_map.values_mut() {
    replacements.sort_by_key(|r| {
        std::cmp::Reverse(r.search_result.byte_offsets.map(|(start, _)| start))
    });
}
```

### Step 3: Update replacement logic to apply multiple per line

**In the file processing loop**, handle both cases:
```rust
if let Some(mut replacements) = line_map.remove(&line_number) {
    // Read the line(s)
    let num_lines = replacements[0].search_result.end_line_number - line_number + 1;
    let mut actual_lines = vec![...]; // existing line reading logic

    if replacements.len() == 1 && replacements[0].search_result.byte_offsets.is_none() {
        // Line-mode: single replacement for entire line(s) - existing behavior
        // (validation + write replacement logic as before)
    } else {
        // Byte-mode: multiple replacements on same line, apply right-to-left
        let mut line_content = actual_lines[0].content.clone();

        for res in replacements {
            if let Some((start, end)) = res.search_result.byte_offsets {
                line_content.replace_range(start..=end, &res.replacement);
                res.replace_result = Some(ReplaceResult::Success);
            } else {
                // Should be caught by conflict detection
                res.replace_result = Some(ReplaceResult::Error(...));
            }
        }

        writer.write_all(line_content.as_bytes())?;
        writer.write_all(actual_lines[0].line_ending.as_bytes())?;
    }
}
```

### Step 4: Enable TDD tests

Remove `#[ignore]` from the 5 Phase 2 tests and verify they all pass.

### Expected Results After Fix:
- ✅ All 7 matches tracked individually
- ✅ Each gets correct individual replacement string ("ITREPLACED", not full line)
- ✅ All 7 marked as Success
- ✅ File correctly shows all replacements
- ✅ Non-multiline mode unchanged

---

## Phase 3-5: TUI-Specific Fixes

**Status**: ⏳ NOT STARTED - TUI-only fixes, lower priority

These phases fix TUI preview and display issues. The core replacement logic works correctly.

### Phase 3: Preview Validation
- Fix: Extract multiline content for validation instead of single line
- Files: `scooter/src/ui/view.rs` (~lines 723-727, 999-1007, 1037-1045)

### Phase 4: Preview Window Rendering
- Fix: Calculate window to include all lines of multiline match
- Files: `scooter/src/ui/view.rs` (~lines 714, 983)

### Phase 5: Results Display Polish
- Fix: Show line ranges (`:5-8`) instead of just start line (`:5`)
- Files: `scooter/src/ui/view.rs` (~line 1193)

---

## Key Design Decisions

### SearchResult Structure
- `lines: Vec<Line>` stores complete lines that the match touches
- Each `Line` has `content` (without line ending) and `line_ending` separate
- `content()` method reconstructs full text by joining lines with their endings
- Design principle: "Scooter operates on lines, so we should always include all lines"

### Replacement Approach
- **Streaming**: No need to load entire file into memory
- **Unified code path**: Single-line and multiline use same logic
- **Iterator consumption**: Read multiline matches by calling `.next()` multiple times
- **O(n) conflict detection**: After sorting, if `start <= last_end`, it's a conflict

### Line Endings
- Replacements written **as-is** with no automatic line ending manipulation
- Users control line structure by including/excluding `\n` in replacement strings
- Enables n != m line replacements (replace 3 lines with 1, or vice versa)

---

## Status Summary

**Phase 1**: ✅ COMPLETE
- Multiline replacement working
- Conflict detection (line-level and byte-level)
- 330 tests passing, 5 TDD tests for Phase 2

**Phase 2**: ⏳ READY TO START
- Fix replacement string calculation (app.rs)
- Fix HashMap to handle multiple per line (replace.rs)
- Enable 5 ignored TDD tests

**Phase 3-5**: ⏳ NOT STARTED
- TUI-only fixes (preview validation, rendering, display)
- Lower priority - core logic works
