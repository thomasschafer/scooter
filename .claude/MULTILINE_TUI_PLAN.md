# TUI Multiline Search & Replace Implementation Plan

**Status**: In Progress
**Last Updated**: 2025-12-30

---

## Overview

This plan addresses the missing TUI functionality for multiline search and replace. While multiline search works in headless mode, the TUI has several critical bugs that prevent it from working correctly.

## Issues Found

1. ❌ **Replacement execution** - `replace_in_file()` can't handle multiline matches (line-by-line processing)
2. ❌ **Conflict detection** - No detection of overlapping line ranges
3. ❌ **Preview validation** - Compares single line vs multiline content (always fails)
4. ❌ **Preview rendering** - Only centers on start line, doesn't show full match
5. ❌ **Results display** - Only shows start line number (no multiline indication)

---

## Phase 0: Setup ✅

**Status**: ✅ COMPLETE

- ✅ Created this plan document
- ✅ Set up todo tracking

---

## Phase 1: Fix Replacement Execution + Conflict Detection

**Status**: ✅ IMPLEMENTATION COMPLETE - Ready for manual TUI testing

**Goal**: Make replacements actually work with proper conflict handling

**Key Insight from User's Commit**: SearchResult now stores **complete lines** (`lines: Vec<Line>`), not just matched text. Each `Line` has `content` (without line ending) and `line_ending` separate. The `content()` method reconstructs the full text by concatenating all lines with their endings.

**Key Design Decision**: Single-line and multiline replacements can use the SAME code path. Multiline is just a generalization of single-line (where start_line == end_line). No need for two separate functions or in-memory loading.

### Implementation Plan

**File**: `scooter-core/src/replace.rs` - modify `replace_in_file()` function

#### Step 1: Upfront conflict detection (before file I/O)

Sort results by `start_line_number`, then mark conflicts in single O(n) pass:

```rust
results.sort_by_key(|r| r.search_result.start_line_number);

let mut last_end_line = 0;
for result in results.iter_mut() {
    let start = result.search_result.start_line_number;
    let end = result.search_result.end_line_number;

    if start <= last_end_line {
        // This replacement starts within or overlaps with previous range
        result.replace_result = Some(ReplaceResult::Error(
            "Conflicts with previous replacement".to_owned(),
        ));
    } else {
        last_end_line = end;
    }
}
```

**Why this works**: After sorting by start line, if replacement N starts at line ≤ last_end_line, it MUST conflict with a previous replacement. O(n) pass marks all conflicts.

**Conflict example** (lines 9-11, 10-13, 12-12 after sorting):
- 9-11: start=9 > last_end(0) → OK, set last_end=11
- 10-13: start=10 ≤ last_end(11) → CONFLICT
- 12-12: start=12 > last_end(11) → OK, set last_end=12

#### Step 2: Streaming line-by-line replacement (handles both single and multiline)

```rust
// Build map of non-conflicting replacements by start line
let mut replacement_map: HashMap<usize, &mut SearchResultWithReplacement> = results
    .iter_mut()
    .filter(|r| r.replace_result.is_none())
    .map(|r| (r.search_result.start_line_number, r))
    .collect();

let mut lines_iter = reader.lines_with_endings().enumerate();

while let Some((idx, line_result)) = lines_iter.next() {
    let line_number = idx + 1; // 1-indexed

    if let Some(result) = replacement_map.get_mut(&line_number) {
        // This line starts a replacement (single or multiline)
        let end_line = result.search_result.end_line_number;
        let num_lines = end_line - line_number + 1;

        // Accumulate all lines for this match
        let (first_line, first_ending) = line_result?;
        let mut actual_lines = vec![Line {
            content: String::from_utf8(first_line)?,
            line_ending: first_ending,
        }];

        // Read additional lines if multiline (num_lines > 1)
        for _ in 1..num_lines {
            if let Some((_, next_result)) = lines_iter.next() {
                let (line_bytes, ending) = next_result?;
                actual_lines.push(Line {
                    content: String::from_utf8(line_bytes)?,
                    line_ending: ending,
                });
            } else {
                // File shorter than expected
                result.replace_result = Some(ReplaceResult::Error(
                    "File changed since last search".to_owned(),
                ));
                break;
            }
        }

        // Validate: actual lines match expected
        if actual_lines == result.search_result.lines {
            writer.write_all(result.replacement.as_bytes())?;
            result.replace_result = Some(ReplaceResult::Success);
        } else {
            result.replace_result = Some(ReplaceResult::Error(
                "File changed since last search".to_owned(),
            ));
            // Write original lines (file validation failed)
            for line in &actual_lines {
                writer.write_all(line.content.as_bytes())?;
                writer.write_all(line.line_ending.as_bytes())?;
            }
        }
    } else {
        // No replacement for this line, copy as-is
        let (line_bytes, line_ending) = line_result?;
        writer.write_all(&line_bytes)?;
        writer.write_all(line.line_ending.as_bytes())?;
    }
}
```

**Why this approach is better**:
1. ✅ Streaming - no need to read entire file into memory
2. ✅ Single unified code path for both single-line and multiline
3. ✅ No LineIndex or byte-offset calculations needed
4. ✅ O(n) conflict detection upfront (simpler than checking overlaps)
5. ✅ Natural line ending handling via `lines_with_endings()`
6. ✅ Iterator naturally skips past consumed lines (when we call `.next()` multiple times for multiline matches)

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

### Testing Checkpoint

**Manual TUI testing** (by user):
- [ ] Multiline replacements actually work (file is modified correctly)
- [ ] Conflicting replacements show error message  
- [ ] Non-conflicting replacements succeed
- [ ] Preview rendering issues OK to ignore for now (Phase 2-3)

**Approval**: ⏸️ WAITING FOR MANUAL TUI TESTING AND APPROVAL

### Phase 1 follow-up: Byte-indexed replacements

**Status**: ✅ INVESTIGATION COMPLETE - Ready for implementation
**Goal**: Fix conflict detection for multiple matches on same line by tracking byte offsets

**Problem discovered**:
When multiple matches occur on the same line (e.g., searching for "bar" in "bar baz bar qux"), the current implementation incorrectly marks the second match as conflicting, even though they occur at different byte offsets.

Example:
```txt
foo
bar baz bar qux
bar
bux
```

Searching for "bar" finds 3 matches:
1. Line 2, bytes 4-7
2. Line 2, bytes 12-15
3. Line 3, bytes 20-23

**Current buggy behavior**: Match #2 incorrectly conflicts with #1 ❌
**Expected behavior**: All 3 matches should succeed ✅

**Root cause**: `mark_conflicting_replacements` only checks line numbers, not byte offsets.

**Solution**: Add byte offsets to `SearchResult`

#### Changes Required:

**1. Update SearchResult struct** (`scooter-core/src/search.rs:49`):
```rust
pub struct SearchResult {
    pub path: Option<PathBuf>,
    pub start_line_number: usize,
    pub end_line_number: usize,
    pub start_byte_offset: usize,  // NEW: 0-indexed byte offset within file
    pub end_byte_offset: usize,    // NEW: 0-indexed byte offset within file
    pub lines: Vec<Line>,
    pub included: bool,
}
```

**2. Update `create_search_result_from_bytes`** (already has the byte data!):
- Just pass through `start_byte` and `end_byte` to `SearchResult::new`

**3. Update `search_file` (line-by-line mode)**:
- For non-multiline mode, set `start_byte_offset = 0` and `end_byte_offset = usize::MAX` (or line length)
- This way byte-level conflict detection won't trigger for line-by-line mode

**4. Update conflict detection** (`mark_conflicting_replacements`):
```rust
fn mark_conflicting_replacements(results: &mut [SearchResultWithReplacement]) {
    // Sort by byte offset instead of line number
    let mut last_end_byte = 0;
    for result in results {
        let start = result.search_result.start_byte_offset;
        let end = result.search_result.end_byte_offset;

        if start < last_end_byte {  // Byte-level overlap
            result.replace_result = Some(ReplaceResult::Error(
                "Conflicts with previous replacement".to_owned(),
            ));
        } else {
            last_end_byte = end;
        }
    }
}
```

**5. Update replacement logic** to handle multiple matches per line:
- Group replacements by line
- Within each line, sort by byte offset in REVERSE order
- Apply replacements right-to-left so byte offsets remain valid
- This is only needed for multiline mode; line-by-line mode continues as before

#### Benefits:
- ✅ Fixes multiple matches per line
- ✅ Enables precise diff highlighting (future enhancement)
- ✅ Aligns with how regex engines work
- ✅ No breaking changes for binary users (only affects `--multiline` flag behavior)

#### Tests Added:
- ✅ `search::tests::test_multiple_matches_per_line` - verifies search finds all 3
- ✅ `replace::tests::test_multiple_matches_same_line_shows_conflict` - demonstrates the bug

#### Tests To Add After Fix:
- [ ] `test_multiple_matches_same_line_all_replaced` - verify all get replaced
- [ ] `test_three_matches_same_line`
- [ ] `test_multiple_matches_across_lines_and_within_lines`

---

## Phase 2: Fix Preview Validation

**Status**: ⏳ NOT STARTED (blocked on Phase 1 approval)

**Goal**: Stop "File changed since search" errors for multiline matches

**Updated approach based on SearchResult.lines structure**:
- User already added TODO comments in view.rs showing where updates are needed
- Now that we understand SearchResult stores complete lines, validation should compare:
  - Extract lines from file using line numbers
  - Compare against `search_result.lines` (Vec<Line>) instead of reconstructing from single line
- The `content()` method can be used for simple string comparison when appropriate

### Step 2a: Fix stdin preview validation

**File**: `scooter/src/ui/view.rs` (~lines 723-727)

**Current problem**:
```rust
assert!(
    cur.1 == result.search_result.line,  // cur.1 is single line, line is multiline
    "Expected line didn't match actual",
);
```

**Implementation plan**:
- Extract lines `[start_line_number..=end_line_number]` from content
- Join with newlines
- Compare against `search_result.line`

### Step 2b: Fix file preview validation

**File**: `scooter/src/ui/view.rs` (~lines 999-1007, ~1037-1045)

**Implementation plan**: Same multiline extraction approach

### Unit Tests to Add

- [ ] Validation passes for multiline matches
- [ ] Validation still works for single-line matches
- [ ] Different line ending types (LF, CRLF)

### Testing Checkpoint

**Unit tests**:
- [ ] All new tests pass
- [ ] Existing tests still pass

**Manual TUI testing** (by user):
- [ ] No false "File changed" errors for multiline matches
- [ ] Preview content appears (even if window is wrong)

**Approval**: ⏸️ WAITING FOR USER APPROVAL

---

## Phase 3: Fix Preview Window Rendering

**Status**: ⏳ NOT STARTED (blocked on Phase 2 approval)

**Goal**: Show full multiline match in preview

### Step 3a: Fix stdin preview window

**File**: `scooter/src/ui/view.rs` (~line 714)

**Current problem**:
```rust
let line_idx = result.search_result.start_line_number - 1;
// Only uses start_line, doesn't consider end_line
```

**Implementation plan**:
- Calculate window to include both `start_line_number` and `end_line_number`
- Ensure all lines of match are visible

### Step 3b: Fix file preview window

**File**: `scooter/src/ui/view.rs` (~line 983)

**Implementation plan**: Same window calculation

### Unit Tests to Add

- [ ] Window includes all lines of multiline match
- [ ] Window calculation for various match sizes (2 lines, 5 lines, etc.)
- [ ] Window at start/end of file

### Testing Checkpoint

**Unit tests**:
- [ ] All new tests pass
- [ ] Existing tests still pass

**Manual TUI testing** (by user):
- [ ] Full multiline matches visible in preview
- [ ] Preview window centers appropriately

**Approval**: ⏸️ WAITING FOR USER APPROVAL

---

## Phase 4: Polish Results Display

**Status**: ⏳ NOT STARTED (blocked on Phase 3 approval)

**Goal**: Show line ranges for multiline matches

### Step 4: Update results list display

**File**: `scooter/src/ui/view.rs` (~line 1193)

**Current problem**:
```rust
let line_num = format!(":{}", result.search_result.start_line_number);
// Only shows start line
```

**Implementation plan**:
```rust
let line_num = if result.search_result.start_line_number == result.search_result.end_line_number {
    format!(":{}", result.search_result.start_line_number)
} else {
    format!(":{}-{}", result.search_result.start_line_number, result.search_result.end_line_number)
};
```

### Unit Tests to Add

- [ ] Single-line match shows `:5`
- [ ] Multiline match shows `:5-8`

### Testing Checkpoint

**Unit tests**:
- [ ] All new tests pass
- [ ] Existing tests still pass

**Manual TUI testing** (by user):
- [ ] Results list shows line ranges (e.g., `:5-8`)
- [ ] Single-line matches still show correctly

**Approval**: ⏸️ WAITING FOR USER APPROVAL

---

## Phase 5: Full E2E Testing

**Status**: ⏳ NOT STARTED (blocked on Phase 4 approval)

**Goal**: Verify everything works together

### Test Scenarios

**Pattern variations**:
- [ ] 2-line matches
- [ ] 3+ line matches
- [ ] Regex patterns spanning lines
- [ ] Fixed string patterns spanning lines

**Edge cases**:
- [ ] Matches at start of file
- [ ] Matches at end of file
- [ ] Adjacent matches (no gap)
- [ ] Matches with gaps

**Conflict scenarios**:
- [ ] Overlapping matches
- [ ] Nested matches
- [ ] Adjacent but non-overlapping

**Performance**:
- [ ] Large files with many multiline matches
- [ ] Files with 100+ results

### Testing Checkpoint

**Manual TUI testing** (by user):
- [ ] All scenarios work correctly
- [ ] Performance is acceptable
- [ ] No crashes or errors

**Approval**: ⏸️ WAITING FOR USER APPROVAL

---

## Notes & Discovered Issues

### Phase 1 Implementation Notes (2026-01-02)

**Key Architectural Change Discovered**:
User's commit "Include multiple lines in search result" fundamentally changed how SearchResult works:
- **Old**: `line: String` - just the matched text
- **New**: `lines: Vec<Line>` - ALL complete lines that the match touches
- Each `Line` = `{content: String, line_ending: LineEnding}`
- `content()` method reconstructs full text by joining lines with their endings

**Scooter Design Principle Confirmed**:
"Scooter operates on lines, so we should always include all lines" - user
- When a pattern matches part of a line, the ENTIRE line is included in the replacement
- This is consistent with how TUI shows results and what users expect

**Why conflict detection is necessary**:
- User can select overlapping multiline matches in TUI
- Example: matches at lines 9-11, 10-13, 12-12
- First match succeeds, second conflicts (overlaps), third succeeds (no overlap)
- Without conflict detection, would try to replace already-modified content

**Evolution of Implementation Approach**:

1. **First attempt (rejected)**: Byte-offset approach
   - Read file into memory, build line-to-byte map, extract byte ranges
   - Too complicated - SearchResult stores complete lines, not byte ranges

2. **Second attempt (rejected)**: LineIndex + in-memory
   - Read entire file into memory, use `search::LineIndex` to extract lines
   - Overcomplicated - why read entire file when we can stream?
   - User pointed out: "Can't we just iterate upwards over lines?"

3. **Final approach (current plan)**: Streaming with iterator consumption
   - Single code path for both single-line and multiline (multiline is the generalization)
   - Consume lines from iterator as we go: when we hit a multiline match spanning lines 9-11, call `.next()` three times
   - No need for in-memory loading, LineIndex, or separate code paths
   - O(n) upfront conflict detection: after sorting, if start ≤ last_end, it's a conflict
   - Much simpler and more elegant!

**Critical Line Ending Design Decision**:
During implementation, discovered that replacements must be written **as-is** with no automatic line ending manipulation:
- Users have complete control over line structure by including/excluding `\n` in replacement strings
- Example: replacing lines 9-11 with `"x"` (no newline) allows line 12 to concatenate: `"foo\nxbar\n"`
- This enables n != m line replacements (replace 3 lines with 1, or 1 line with 4)
- Test helper writes replacements exactly as provided

**Implementation completed**:
- ✅ Streaming approach implemented in `replace_in_file()` (scooter-core/src/replace.rs:247-362)
- ✅ 12 comprehensive unit tests added and passing
- ✅ Test helper consolidated: `create_search_result_with_replacement()` handles both single and multiline

**Next steps**:
- ⏸️ User will test manually in TUI
- ⏸️ If issues found, iterate on the implementation
- ⏸️ Once approved, proceed to Phase 2 (preview validation)

---

## Completion Checklist

- ⏳ Phase 1: Replacement + conflicts (basic implementation complete, byte-indexed fix needed)
  - ✅ Basic multiline replacement working
  - ✅ Conflict detection for overlapping line ranges
  - ⏳ Byte-indexed replacements for multiple matches per line
- [ ] Phase 2: Preview validation
- [ ] Phase 3: Preview rendering
- [ ] Phase 4: Display polish
- [ ] Phase 5: E2E testing
- ✅ Phase 1 basic unit tests passing (13/13 tests)
- [ ] Phase 1 byte-indexed tests
- [ ] All manual tests passing
- [ ] No regressions in existing functionality
- [ ] Documentation updated (if needed)
