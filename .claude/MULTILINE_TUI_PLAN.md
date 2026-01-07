# TUI Multiline Search & Replace Implementation Plan

**Status**: Phase 2 Complete - Type-Safe Architecture Implemented

---

## Overview

This plan describes the type-safe multiline search and replace implementation using separate code paths for two fundamentally different operations.

---

## Phase 1: COMPLETE

Multiline replacement and conflict detection working with byte offsets.

---

## Phase 2: COMPLETE - Type-Safe Mode Separation

### Key Insight

The implementation separates two fundamentally different operations:

1. **Line-mode**: Replace ALL occurrences on line(s)
   - Needs: Full lines for validation
   - Logic: Stream lines, validate lines, replace whole lines

2. **Byte-mode**: Replace specific byte range
   - Needs: Only those bytes for validation
   - Logic: Stream bytes, validate bytes, replace only those bytes

**Solution**: Make the mode explicit in the type system and use separate implementations.

---

## Type Structure

### SearchResult with MatchContent Enum

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub path: Option<PathBuf>,
    pub content: MatchContent,
    pub included: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchContent {
    /// Line-mode: Replace all occurrences of pattern on this line
    Lines {
        line_number: usize,
        content: String,      // Line content WITHOUT line ending
        line_ending: LineEnding,
    },
    /// Byte-mode: Replace only the specific byte range
    ByteRange {
        start_line: usize,    // For display only
        end_line: usize,
        byte_start: usize,    // Absolute position in file
        byte_end: usize,
        expected_content: String,  // Just the matched bytes
    },
}
```

### Benefits

1. **Type-safe**: Can't accidentally mix modes
2. **Efficient**: Each mode stores exactly what it needs
3. **Clear**: Variant tells you exactly how to process
4. **Separate paths**: Each mode gets optimized implementation

---

## Implementation Details

### replace_in_file Architecture

**File**: `scooter-core/src/replace.rs`

```rust
pub fn replace_in_file(results: &mut [SearchResultWithReplacement]) -> anyhow::Result<()> {
    // Determine mode from first result - all must be same type
    match &results[0].search_result.content {
        MatchContent::Lines { .. } => replace_line_mode(file_path, results),
        MatchContent::ByteRange { .. } => replace_byte_mode(file_path, results),
    }
}
```

### replace_line_mode

Simplified line-based logic:
- Build HashMap of replacements by line number
- Stream lines from file
- Validate line content matches
- Write replacement (does NOT include line ending)
- Always append line ending after writing (from file's original ending)

### replace_byte_mode

Byte-streaming approach:
- Sort by byte_start (ascending)
- Detect conflicts (overlapping byte ranges) via `mark_conflicting_replacements`
- Stream bytes directly using `read_to_end` with `take`
- Validate expected bytes match
- Write replacements (or original bytes if mismatch)
- On EOF: write partial bytes read, leave `replace_result` as `None`
- `calculate_statistics` marks unprocessed results as errors

### app.rs update_replacements

```rust
let replacement = match &res.search_result.content {
    MatchContent::ByteRange { expected_content, .. } => {
        debug_assert!(contains_search(expected_content, search));
        replacement_for_match(expected_content, search, replace)
    }
    MatchContent::Lines { content, .. } => {
        replace_all_if_match(content, search, replace)
            .unwrap_or_else(|| panic!("Pattern did not match previously found content"))
    }
};
```

Key functions:
- `replace_all_if_match`: For line-mode, replaces ALL occurrences in line
- `replacement_for_match`: For byte-mode, replaces specific match only

---

## Key Design Decisions

1. **Line-mode replacements do NOT include line endings** - the code appends them after writing
2. **Byte-mode uses `read_to_end` + `take`** - handles EOF gracefully, writes partial bytes
3. **EOF leaves `replace_result` as `None`** - `calculate_statistics` handles error reporting
4. **Panics in `update_replacements` if pattern doesn't match** - this is an invariant violation

---

## Migration Completed

- Updated `SearchResult` type with `MatchContent` enum
- Simplified `MatchContent::Lines` to single line (not Vec)
- Updated `SearchResult` constructors and methods
- Updated `search_multiline` to create `ByteRange` variants
- Implemented new `replace_in_file` dispatcher
- Implemented `replace_line_mode` (simplified from main)
- Implemented `replace_byte_mode` (byte-streaming logic)
- Updated `app.rs` to work with `MatchContent`
- Updated all tests
- Fixed double line ending issue in line-mode

---

## Next Steps

Future TUI improvements (if needed):
- Preview rendering for multiline matches
- Display line ranges in results (`:5-8` instead of `:5`)
