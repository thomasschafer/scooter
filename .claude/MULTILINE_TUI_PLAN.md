# TUI Multiline Search & Replace Implementation Plan

**Status**: Phase 3 - NOT IMPLEMENTED - Preview rendering needs rework (reverted all attempts)

---

## The Goal: GitHub-Style Diff Preview

For a multiline search and replace, we need to show a **standard diff** like GitHub:

**Example:**
```
File content:
foo
bar
baz

Search: "o\nb" (matches "o" at end of "foo", newline, "b" at start of "bar")
Replace: "x"

Expected diff preview:
- foo     (red text, "o" has RED BACKGROUND - it's being deleted)
- bar     (red text, "b" has RED BACKGROUND - it's being deleted)
+ foxar   (green text, "x" has GREEN BACKGROUND - it's being inserted)
  baz     (context line, unchanged)
```

**Key insight**: The diff shows:
1. **Old lines (-)**: The FULL original lines that will change, with matched chars having red background
2. **New lines (+)**: What those lines BECOME after replacement, with replacement chars having green background

This is NOT about showing "which bytes are matched" - it's about showing **the actual diff between before and after**.

---

## Current State Analysis

### What Works
- `line_diff()` in `diff.rs` - computes character-level diff between two strings with proper background colors
- Single-line preview with character-level diff (for `MatchContent::Lines`)
- Multiline search and replacement execution

### What's Broken
- Multiline preview (`MatchContent::ByteRange`) - completely wrong approach
- Single-line `ByteRange` - lost character-level diff

### Root Cause
The current implementation tries to show "precise byte-position highlighting" - highlighting which parts of the original lines are matched. But what we actually need is a **standard diff** showing:
1. The original affected lines (with matched portion having red bg)
2. The resulting lines after replacement (with replacement portion having green bg)

---

## Correct Approach

### For Single-Line Matches (Lines or single-line ByteRange)

Use existing `line_diff()` which already works correctly:
- Input: old line content, new line content (after replacement)
- Output: styled old line (red text, deleted chars have red bg) + styled new line (green text, inserted chars have green bg)

### For Multi-Line Matches (ByteRange spanning multiple lines)

Need to:
1. **Reconstruct old content**: The full lines that contain the match
2. **Reconstruct new content**: What those lines look like after replacement
3. **Compute diff**: Use `line_diff()` or similar on the joined content

**Example reconstruction:**
```
Match: "o\nb" in lines "foo" and "bar"
- start_line: line 1, byte_pos 2 (after "fo")
- end_line: line 2, byte_pos 1 (after "b")

Old content (full affected lines): "foo\nbar"
New content (after replacement): "fo" + "x" + "ar" = "foxar"

Diff of "foo\nbar" vs "foxar":
- Deleted: "o", "\n", "b" (red background)
- Inserted: "x" (green background)
- Unchanged: "fo", "ar" (normal text with red/green foreground)
```

---

## Implementation Plan

### Step 1: Clean Up Current Mess

Remove all the broken multiline preview code:
- `build_byte_range_diff()` - wrong approach
- `split_lines_for_byte_range()` - not needed
- `split_at_byte_pos()` - might keep for utility
- `highlighted_lines_to_plain()` - not needed

### Step 2: Create Proper Multiline Diff Function

```rust
/// Builds diff for multiline ByteRange by reconstructing before/after content
fn build_multiline_diff(
    affected_lines: &[(usize, String)],  // Full lines from file
    start_line: &LinePos,
    end_line: &LinePos,
    replacement: &str,
) -> (Vec<StyledLine>, Vec<StyledLine>) {
    // 1. Build old content string (the full affected lines)
    let old_content = affected_lines.iter()
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // 2. Build new content string (after replacement)
    // prefix (before match in first line) + replacement + suffix (after match in last line)
    let first_line = &affected_lines[0].1;
    let last_line = &affected_lines[affected_lines.len() - 1].1;
    let prefix = &first_line[..start_line.byte_pos];
    let suffix = &last_line[end_line.byte_pos..];
    let new_content = format!("{}{}{}", prefix, replacement, suffix);

    // 3. Use line_diff or similar to get character-level styled output
    // Split by lines and style each line
    ...
}
```

### Step 3: Update Preview Building

In `build_preview_from_file` and `build_preview_from_str`:
1. For `MatchContent::Lines` and single-line `ByteRange`: Use existing `preview.all_diff_lines()` (cached `line_diff`)
2. For multi-line `ByteRange`: Call `build_multiline_diff()` with full affected lines

### Step 4: Ensure Single-Line ByteRange Uses Cached Diff

Single-line ByteRange should use the same path as `MatchContent::Lines`:
- `build_search_result_preview()` already handles this (checks `is_multiline`)
- Don't intercept it in `build_preview_from_file`

---

## Data We Have Available

### For ByteRange
- `start_line.line` / `end_line.line` - which lines are affected (1-indexed)
- `start_line.byte_pos` / `end_line.byte_pos` - where in each line the match starts/ends
- `expected_content` - the matched bytes (NOT the full lines)
- `replacement` - what to replace with

### In Preview Building
- Full file lines via `read_lines_range_*` - we CAN get the full line content
- The `preview` from `build_search_result_preview()` - contains styled diff lines

---

## Key Files to Modify

1. **`view.rs`**:
   - Remove broken functions
   - Add `build_multiline_diff()`
   - Update `build_preview_from_file` and `build_preview_from_str`

2. **`diff.rs`** (maybe):
   - Could add a multiline variant of `line_diff()` that handles multi-line strings

---

## Test Cases

1. **Single-line non-multiline** (MatchContent::Lines):
   - Search "foo" replace "bar" in "hello foo world"
   - Should show: `- hello foo world` / `+ hello bar world` with char-level diff

2. **Single-line multiline mode** (ByteRange on one line):
   - Same as above but with multiline flag
   - Should behave identically

3. **Multi-line match**:
   - File: "foo\nbar\nbaz", search "o\nb", replace "x"
   - Should show: `- foo` / `- bar` / `+ foxar` with "o", "b" red bg, "x" green bg

4. **Multi-line match spanning 3+ lines**:
   - File: "aa\nbb\ncc\ndd", search "a\nbb\nc", replace "X"
   - Should show: `- aa` / `- bb` / `- cc` / `+ aXc` with proper highlighting
