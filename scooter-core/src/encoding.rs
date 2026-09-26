//! Support for text files that aren't UTF-8 encoded, such as those saved as Latin-1 or
//! Windows-1252, including files that mix UTF-8 with another encoding.
//!
//! Files are decoded line by line (see [`decode_line`]): a line that is valid UTF-8 is decoded as
//! UTF-8, and any other line is decoded using the file's detected "legacy" encoding. Lines that
//! can't be decoded either way are "opaque": they are displayed lossily, but never searched or
//! modified. Every code path (line-by-line search, multiline search, previews and replacement)
//! decodes lines in this way, so they always agree on the text of a file.
//!
//! Text is only decoded with an encoding if encoding it again reproduces the original bytes
//! exactly, and replacements are only written if they can be represented in the encoding of the
//! lines they modify, so files are never silently altered. Lines that aren't modified by a
//! replacement are always written back byte for byte.
//!
//! UTF-8 files never reach the line-by-line decoding, and the legacy encoding of a file is only
//! detected once a line that isn't valid UTF-8 is found, so UTF-8 files are handled just as
//! quickly as if non-UTF-8 files weren't supported.
use std::{
    borrow::Cow,
    cell::OnceCell,
    fs::File,
    io::{self, BufRead, BufReader, Read},
    ops::{Range, RangeInclusive},
    path::{Path, PathBuf},
};

use anyhow::Context;
use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use content_inspector::inspect;
use encoding_rs::{EncoderResult, Encoding, UTF_8};

use crate::{
    line_index::{LineIndex, newline_positions},
    line_reader::LineEnding,
};

/// How a line of a file was decoded
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// The line is valid UTF-8 (which includes lines that are entirely ASCII)
    Utf8,
    /// The line isn't valid UTF-8, and was decoded using the file's legacy encoding
    Legacy(&'static Encoding),
    /// The line couldn't be decoded, so its text is only an approximation for display purposes.
    /// Opaque lines must never be searched or modified.
    Opaque,
}

/// A line of a file, decoded to UTF-8
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedLine {
    pub text: String,
    pub kind: LineKind,
}

/// Decodes a line of a file, excluding its line ending.
///
/// `legacy_encoding` returns the file's legacy encoding (or `None` if it has none), and is only
/// called if the line isn't valid UTF-8, so that detection can be deferred until it's needed.
///
/// None of the supported encodings allow `\r` or `\n` within a multi-byte character, and lossy
/// decoding never consumes an ASCII byte following an invalid sequence, so decoding a line with or
/// without its ending gives the same text (other than the ending itself).
pub fn decode_line(
    content: Vec<u8>,
    legacy_encoding: impl FnOnce() -> Option<&'static Encoding>,
) -> DecodedLine {
    let bytes = match String::from_utf8(content) {
        Ok(text) => {
            return DecodedLine {
                text,
                kind: LineKind::Utf8,
            };
        }
        Err(e) => e.into_bytes(),
    };

    let encoding = legacy_encoding();
    // Text in the legacy encodings we support never contains NUL bytes, so lines containing them
    // are likely to be binary data
    if let Some(encoding) = encoding
        && !bytes.contains(&0)
        && let Some(text) = decode_strict(&bytes, encoding)
    {
        return DecodedLine {
            text,
            kind: LineKind::Legacy(encoding),
        };
    }
    DecodedLine {
        text: decode_lossy(&bytes, encoding),
        kind: LineKind::Opaque,
    }
}

/// Decodes `bytes` with `encoding`, provided that the result can be encoded back into exactly the
/// same bytes
fn decode_strict(bytes: &[u8], encoding: &'static Encoding) -> Option<String> {
    if !encoding.is_ascii_compatible() {
        return None;
    }
    let text = encoding
        .decode_without_bom_handling_and_without_replacement(bytes)?
        .into_owned();
    // Single-byte encodings map each byte to a distinct character, so always round-trip
    (encoding.is_single_byte() || encode(&text, encoding).ok()? == bytes).then_some(text)
}

/// Decodes `bytes` with `encoding` (or UTF-8 if there is none), replacing invalid sequences with �
fn decode_lossy(bytes: &[u8], encoding: Option<&'static Encoding>) -> String {
    match encoding {
        Some(encoding) => encoding.decode_without_bom_handling(bytes).0.into_owned(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Decodes the lines of a file (see [`decode_line`]). The legacy encoding of the file is detected
/// the first time a line that isn't valid UTF-8 is decoded, so detection has no cost for UTF-8
/// files.
pub struct FileDecoder<P: AsRef<Path>> {
    path: P,
    legacy_encoding: OnceCell<Option<&'static Encoding>>,
}

impl<P: AsRef<Path>> FileDecoder<P> {
    pub fn new(path: P) -> Self {
        Self {
            path,
            legacy_encoding: OnceCell::new(),
        }
    }

    /// The detected legacy encoding of the file, or `None` if it has no supported encoding
    fn legacy_encoding(&self) -> Option<&'static Encoding> {
        *self.legacy_encoding.get_or_init(|| {
            let path = self.path.as_ref();
            File::open(path)
                .and_then(|file| detect_legacy_encoding(BufReader::new(file)))
                .unwrap_or_else(|e| {
                    log::warn!("Failed to detect encoding of {}: {e}", path.display());
                    None
                })
        })
    }

    /// Decodes a line of the file, excluding its line ending
    pub fn decode_line(&self, content: Vec<u8>) -> DecodedLine {
        decode_line(content, || self.legacy_encoding())
    }
}

/// Decodes lines for display, using the encoding of the file they came from if known, and
/// otherwise replacing invalid UTF-8 with �
pub struct LineDecoder {
    file_decoder: Option<FileDecoder<PathBuf>>,
}

impl LineDecoder {
    pub fn new(path: Option<&Path>) -> Self {
        Self {
            file_decoder: path.map(|path| FileDecoder::new(path.to_path_buf())),
        }
    }

    /// Decodes a line, excluding its line ending
    pub fn decode(&self, content: Vec<u8>) -> String {
        match &self.file_decoder {
            Some(file_decoder) => file_decoder.decode_line(content).text,
            None => decode_line(content, || None).text,
        }
    }
}

/// Detects the encoding of the content of `reader`, assuming it isn't UTF-8. Returns `None` if
/// the content has a UTF-16 byte order mark, or a NUL byte is found (indicating binary data).
///
/// Detection is relatively slow, so only a sample of the content is used: ASCII-only lines don't
/// affect the result, so only lines containing non-ASCII bytes are included, up to a limit. The
/// amount of content read is also limited, so that large files aren't read in full.
fn detect_legacy_encoding(reader: impl BufRead) -> io::Result<Option<&'static Encoding>> {
    let mut reader = reader.take(MAX_DETECTION_READ_BYTES);
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    let mut remaining = MAX_DETECTION_SAMPLE_BYTES;
    let mut line = Vec::new();
    let mut is_first_line = true;
    let mut reached_end = false;

    while remaining > 0 {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            reached_end = reader.limit() > 0;
            break;
        }
        if line.contains(&0) {
            return Ok(None);
        }
        // UTF-16 can't be encoded by `encoding_rs`, so we couldn't write replacements back
        if is_first_line && Encoding::for_bom(&line).is_some_and(|(enc, _)| enc != UTF_8) {
            return Ok(None);
        }
        is_first_line = false;
        if line.is_ascii() {
            continue;
        }

        let sample = if line.len() > remaining {
            // Truncate at an ASCII byte to avoid splitting a multi-byte character
            let end = line[..remaining]
                .iter()
                .rposition(u8::is_ascii)
                .map_or(remaining, |i| i + 1);
            &line[..end]
        } else {
            &line
        };
        detector.feed(sample, false);
        remaining = remaining.saturating_sub(sample.len().max(1));
    }
    // Only signal the end of the stream if we actually reached it, as documented by `chardetng`
    if reached_end {
        detector.feed(&[], true);
    }
    Ok(Some(detector.guess(None, Utf8Detection::Deny)))
}

/// Maximum number of non-ASCII bytes used to detect the encoding of a file
const MAX_DETECTION_SAMPLE_BYTES: usize = 1024 * 1024;

/// Maximum number of bytes read from a file when detecting its encoding
const MAX_DETECTION_READ_BYTES: u64 = 16 * 1024 * 1024;

/// Splits a line at `\n` into its content and ending, consistent with
/// [`crate::line_reader::BufReadExt::lines_with_endings`]. `piece` excludes the `\n`, and
/// `is_last` indicates that it wasn't followed by one.
fn split_piece(piece: &[u8], is_last: bool) -> (&[u8], LineEnding) {
    if is_last {
        (piece, LineEnding::None)
    } else if let Some(content) = piece.strip_suffix(b"\r") {
        (content, LineEnding::CrLf)
    } else {
        (piece, LineEnding::Lf)
    }
}

/// Splits `bytes` into lines, returning the content and ending of each line (see
/// [`split_piece`]). Unlike [`crate::line_reader::BufReadExt::lines_with_endings`], a final empty
/// line is returned after a trailing `\n` (or for empty input), so that every byte offset belongs
/// to exactly one line, consistent with [`LineIndex::line_number_at`].
fn split_lines(bytes: &[u8]) -> impl Iterator<Item = (&[u8], LineEnding)> {
    let mut pieces = bytes.split(|&b| b == b'\n').peekable();
    std::iter::from_fn(move || {
        let piece = pieces.next()?;
        Some(split_piece(piece, pieces.peek().is_none()))
    })
}

/// The contents of a file, decoded to UTF-8
#[derive(Debug)]
pub enum DecodedFile {
    Utf8(String),
    NonUtf8(NonUtf8File),
}

/// A file that isn't valid UTF-8, decoded line by line (see [`decode_line`])
#[derive(Debug)]
pub struct NonUtf8File {
    /// The contents of the file on disk
    original: Vec<u8>,
    text: String,
    legacy_encoding: Option<&'static Encoding>,
    /// Whether any line containing non-ASCII characters is valid UTF-8. If so, UTF-8 is used when
    /// inserting non-ASCII text into lines that are entirely ASCII, rather than the legacy
    /// encoding.
    has_utf8_lines: bool,
    /// Indices of the lines (from 0) that couldn't be decoded, in ascending order
    opaque_lines: Vec<usize>,
}

/// Decodes the contents of a file. This never fails: lines that can't be decoded are opaque.
///
/// Use [`decode_text`] instead to decode files for searching.
pub fn decode(bytes: Vec<u8>) -> DecodedFile {
    match String::from_utf8(bytes) {
        Ok(text) => DecodedFile::Utf8(text),
        Err(e) => DecodedFile::NonUtf8(NonUtf8File::decode(e.into_bytes())),
    }
}

/// Decodes the contents of a file for searching. Unlike [`decode`], this returns `None` for
/// content that isn't valid UTF-8 if it looks like binary data, or has no supported encoding.
pub fn decode_text(bytes: Vec<u8>) -> Option<DecodedFile> {
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return Some(DecodedFile::Utf8(text)),
        Err(e) => e.into_bytes(),
    };
    // Checked before decoding, as these checks are much faster
    if bytes.contains(&0) || inspect(&bytes).is_binary() {
        return None;
    }
    let file = NonUtf8File::decode(bytes);
    file.legacy_encoding
        .is_some()
        .then_some(DecodedFile::NonUtf8(file))
}

/// Reads the file at `path` and decodes it for searching (see [`decode_text`])
pub fn read_text(path: &Path) -> anyhow::Result<DecodedFile> {
    let bytes = std::fs::read(path)?;
    decode_text(bytes).with_context(|| format!("Unsupported file encoding: {}", path.display()))
}

/// A replacement of a range of bytes in the decoded text of a file
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement<'a> {
    pub range: Range<usize>,
    pub text: Cow<'a, str>,
}

/// The result of applying replacements to a file
#[derive(Debug)]
pub struct ReplacedFile {
    /// The new contents of the file
    pub bytes: Vec<u8>,
    /// The outcome of each replacement, in the order they were given
    pub outcomes: Vec<Result<(), String>>,
}

/// Error for a replacement that would modify an opaque line
const OPAQUE_LINE_ERROR: &str = "Can't replace text in a line whose encoding isn't recognised";

/// Error for a replacement that would modify lines with different encodings
const MIXED_ENCODINGS_ERROR: &str = "Can't replace text spanning lines with different encodings";

fn unrepresentable_error(encoding: &'static Encoding) -> String {
    format!(
        "Text contains characters that can't be represented in the file's encoding ({})",
        encoding.name()
    )
}

impl DecodedFile {
    pub fn text(&self) -> &str {
        match self {
            Self::Utf8(text) => text,
            Self::NonUtf8(file) => &file.text,
        }
    }

    pub fn into_text(self) -> String {
        match self {
            Self::Utf8(text) => text,
            Self::NonUtf8(file) => file.text,
        }
    }

    /// Removes the items that couldn't be replaced because they would modify an opaque line (see
    /// [`touched_lines`]). `range` returns the byte range of an item in the decoded text.
    pub fn retain_replaceable<T>(&self, items: &mut Vec<T>, range: impl Fn(&T) -> Range<usize>) {
        let Self::NonUtf8(file) = self else {
            return;
        };
        if file.opaque_lines.is_empty() {
            return;
        }
        let text_index = LineIndex::new(&file.text);
        items.retain(|item| !file.touches_opaque_line(touched_lines(&text_index, range(item))));
    }

    /// Applies `replacements`, which must be sorted, non-overlapping byte ranges on character
    /// boundaries of the decoded text, returning an error otherwise.
    ///
    /// For files that aren't UTF-8, the lines modified by each replacement are encoded using the
    /// encoding they were decoded with, and all other lines are written back exactly as they
    /// were. Individual replacements fail if they would modify an opaque line or lines with
    /// different encodings, or if they contain characters that can't be represented in the
    /// encoding of the lines they modify.
    pub fn apply_replacements(
        &self,
        replacements: &[Replacement<'_>],
    ) -> anyhow::Result<ReplacedFile> {
        let text = self.text();
        let mut prev_end = 0;
        for Replacement { range, .. } in replacements {
            anyhow::ensure!(
                prev_end <= range.start
                    && range.start <= range.end
                    && text.is_char_boundary(range.start)
                    && text.is_char_boundary(range.end),
                "Invalid replacement range {range:?} (previous replacement ended at {prev_end})"
            );
            prev_end = range.end;
        }

        match self {
            Self::Utf8(text) => {
                let mut bytes = Vec::with_capacity(text.len());
                let mut pos = 0;
                for Replacement { range, text: new } in replacements {
                    bytes.extend_from_slice(&text.as_bytes()[pos..range.start]);
                    bytes.extend_from_slice(new.as_bytes());
                    pos = range.end;
                }
                bytes.extend_from_slice(&text.as_bytes()[pos..]);
                Ok(ReplacedFile {
                    bytes,
                    outcomes: vec![Ok(()); replacements.len()],
                })
            }
            Self::NonUtf8(file) => file.apply_replacements(replacements),
        }
    }
}

/// Returns the indices (from 0) of the lines modified by replacing `range`. This includes the line
/// containing `range.end`, so a replacement that consumes a line ending includes the following
/// line, which the replacement is joined onto.
fn touched_lines(text_index: &LineIndex<'_>, range: Range<usize>) -> RangeInclusive<usize> {
    (text_index.line_number_at(range.start) - 1)..=(text_index.line_number_at(range.end) - 1)
}

/// A set of replacements that modify overlapping lines, so must be applied together
struct LineGroup {
    lines: RangeInclusive<usize>,
    /// Indices of the replacements in the group
    replacements: Vec<usize>,
}

/// Groups sorted, non-overlapping replacements by the lines they modify
fn group_by_lines(text_index: &LineIndex<'_>, replacements: &[Replacement<'_>]) -> Vec<LineGroup> {
    let mut groups: Vec<LineGroup> = vec![];
    for (idx, replacement) in replacements.iter().enumerate() {
        let lines = touched_lines(text_index, replacement.range.clone());
        match groups.last_mut() {
            Some(group) if lines.start() <= group.lines.end() => {
                group.lines = *group.lines.start()..=*lines.end();
                group.replacements.push(idx);
            }
            _ => groups.push(LineGroup {
                lines,
                replacements: vec![idx],
            }),
        }
    }
    groups
}

impl NonUtf8File {
    fn decode(original: Vec<u8>) -> Self {
        let legacy_encoding = detect_legacy_encoding(original.as_slice())
            .unwrap_or_else(|e| unreachable!("Reading from a slice can't fail: {e}"));
        Self::decode_with(original, legacy_encoding)
    }

    fn decode_with(original: Vec<u8>, legacy_encoding: Option<&'static Encoding>) -> Self {
        let has_utf8_lines = split_lines(&original)
            .any(|(line, _)| !line.is_ascii() && std::str::from_utf8(line).is_ok());

        // If every line decodes with the legacy encoding, decoding the whole file at once gives
        // the same text as decoding it line by line, and is much faster
        let (text, opaque_lines) = if !has_utf8_lines
            && let Some(encoding) = legacy_encoding
            && !original.contains(&0)
            && let Some(text) = decode_strict(&original, encoding)
        {
            (text, vec![])
        } else {
            decode_lines(&original, legacy_encoding)
        };

        Self {
            original,
            text,
            legacy_encoding,
            has_utf8_lines,
            opaque_lines,
        }
    }

    fn touches_opaque_line(&self, lines: RangeInclusive<usize>) -> bool {
        let first_candidate = self
            .opaque_lines
            .partition_point(|&idx| idx < *lines.start());
        self.opaque_lines
            .get(first_candidate)
            .is_some_and(|idx| lines.contains(idx))
    }

    fn apply_replacements(&self, replacements: &[Replacement<'_>]) -> anyhow::Result<ReplacedFile> {
        let lines = FileLines::new(self)?;
        let mut outcomes = vec![Ok(()); replacements.len()];
        let mut bytes = Vec::with_capacity(self.original.len());
        let mut original_pos = 0;

        for group in group_by_lines(&lines.text_index, replacements) {
            let (first, last) = (*group.lines.start(), *group.lines.end());
            let original_range = lines.original_span(first).start..lines.original_span(last).end;
            bytes.extend_from_slice(&self.original[original_pos..original_range.start]);
            original_pos = original_range.end;

            let encoding = match self.group_encoding(&lines, group.lines)? {
                Ok(encoding) => encoding,
                Err(error) => {
                    for idx in group.replacements {
                        outcomes[idx] = Err(error.to_owned());
                    }
                    bytes.extend_from_slice(&self.original[original_range]);
                    continue;
                }
            };

            let text_range = lines.text_span(first).start..lines.text_span(last).end;
            let mut new_text = String::with_capacity(text_range.len());
            let mut text_pos = text_range.start;
            for idx in group.replacements {
                let Replacement { range, text } = &replacements[idx];
                new_text.push_str(&self.text[text_pos..range.start]);
                if can_encode(text, encoding) {
                    new_text.push_str(text);
                } else {
                    outcomes[idx] = Err(unrepresentable_error(encoding));
                    new_text.push_str(&self.text[range.clone()]);
                }
                text_pos = range.end;
            }
            new_text.push_str(&self.text[text_pos..text_range.end]);
            encode_into(&new_text, encoding, &mut bytes)
                .context("Failed to encode lines that should be representable")?;
        }
        bytes.extend_from_slice(&self.original[original_pos..]);

        Ok(ReplacedFile { bytes, outcomes })
    }

    /// Determines the encoding to use when rewriting `line_indices`, or the reason they can't be
    /// rewritten. Lines that are entirely ASCII are compatible with any encoding, so the other
    /// lines must all share the same encoding.
    ///
    /// Returns an error (rather than `Ok(Err(..))`) if the decoded text of a line doesn't match
    /// the original file, which should be impossible.
    fn group_encoding(
        &self,
        lines: &FileLines<'_>,
        line_indices: RangeInclusive<usize>,
    ) -> anyhow::Result<Result<&'static Encoding, &'static str>> {
        let mut group_encoding = None;
        for idx in line_indices {
            let (line, is_ascii) = lines.decode(idx)?;
            let line_encoding = match line.kind {
                LineKind::Opaque => return Ok(Err(OPAQUE_LINE_ERROR)),
                LineKind::Utf8 if is_ascii => continue,
                LineKind::Utf8 => UTF_8,
                LineKind::Legacy(encoding) => encoding,
            };
            match group_encoding {
                None => group_encoding = Some(line_encoding),
                Some(encoding) if encoding == line_encoding => {}
                Some(_) => return Ok(Err(MIXED_ENCODINGS_ERROR)),
            }
        }
        Ok(Ok(group_encoding.unwrap_or_else(|| self.default_encoding())))
    }

    /// The encoding used when inserting text into lines that are entirely ASCII, consistent with
    /// the rest of the file
    fn default_encoding(&self) -> &'static Encoding {
        match self.legacy_encoding {
            Some(encoding) if !self.has_utf8_lines => encoding,
            _ => UTF_8,
        }
    }
}

/// The lines of a file that isn't UTF-8, in both its original and decoded forms. Lines are
/// indexed from 0, as returned by [`split_lines`].
struct FileLines<'a> {
    file: &'a NonUtf8File,
    text_index: LineIndex<'a>,
    original_newlines: Vec<usize>,
}

impl<'a> FileLines<'a> {
    fn new(file: &'a NonUtf8File) -> anyhow::Result<Self> {
        let text_index = LineIndex::new(&file.text);
        let original_newlines = newline_positions(&file.original);
        anyhow::ensure!(
            original_newlines.len() == text_index.newline_count(),
            "Decoded text has a different number of lines to the original file"
        );
        Ok(Self {
            file,
            text_index,
            original_newlines,
        })
    }

    /// The byte range of a line in the original file, excluding the `\n`
    fn original_span(&self, idx: usize) -> Range<usize> {
        let start = match idx {
            0 => 0,
            _ => self.original_newlines[idx - 1] + 1,
        };
        let end = self
            .original_newlines
            .get(idx)
            .copied()
            .unwrap_or(self.file.original.len());
        start..end
    }

    /// The byte range of a line in the decoded text, excluding the `\n`
    fn text_span(&self, idx: usize) -> Range<usize> {
        self.text_index.line_span(idx + 1)
    }

    /// Decodes a line of the original file, returning it along with whether it is entirely ASCII.
    /// Returns an error if it doesn't match the decoded text, which should be impossible.
    fn decode(&self, idx: usize) -> anyhow::Result<(DecodedLine, bool)> {
        let (content, ending) = split_piece(
            &self.file.original[self.original_span(idx)],
            idx == self.original_newlines.len(),
        );
        let line = decode_line(content.to_vec(), || self.file.legacy_encoding);
        let expected_ending = match ending {
            LineEnding::CrLf => "\r",
            LineEnding::Lf | LineEnding::None => "",
        };
        anyhow::ensure!(
            self.file.text[self.text_span(idx)].strip_suffix(expected_ending)
                == Some(line.text.as_str()),
            "Decoded text of line {} doesn't match the original file",
            idx + 1
        );
        Ok((line, content.is_ascii()))
    }
}

/// Decodes `bytes` line by line (see [`decode_line`]), returning the text along with the indices
/// of opaque lines
fn decode_lines(bytes: &[u8], legacy_encoding: Option<&'static Encoding>) -> (String, Vec<usize>) {
    let mut text = String::with_capacity(bytes.len());
    let mut opaque_lines = vec![];
    for (idx, (content, ending)) in split_lines(bytes).enumerate() {
        let line = decode_line(content.to_vec(), || legacy_encoding);
        if line.kind == LineKind::Opaque {
            opaque_lines.push(idx);
        }
        text.push_str(&line.text);
        text.push_str(ending.as_str());
    }
    (text, opaque_lines)
}

/// Encodes `text` using `encoding`, failing if `text` contains characters that can't be
/// represented in that encoding
pub fn encode(text: &str, encoding: &'static Encoding) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    encode_into(text, encoding, &mut bytes)?;
    Ok(bytes)
}

/// Appends `text` to `output`, encoded using `encoding`. Fails if `text` contains characters that
/// can't be represented in that encoding, in which case `output` may contain part of the text.
fn encode_into(
    text: &str,
    encoding: &'static Encoding,
    output: &mut Vec<u8>,
) -> anyhow::Result<()> {
    if encoding == UTF_8 {
        output.extend_from_slice(text.as_bytes());
        return Ok(());
    }
    let mut encoder = encoding.new_encoder();
    let mut remaining = text;
    loop {
        let max_len = encoder
            .max_buffer_length_from_utf8_without_replacement(remaining.len())
            .context("Text is too long to encode")?;
        output.reserve(max_len);
        let (result, read) =
            encoder.encode_from_utf8_to_vec_without_replacement(remaining, output, true);
        remaining = &remaining[read..];
        match result {
            EncoderResult::InputEmpty => return Ok(()),
            EncoderResult::OutputFull => {}
            EncoderResult::Unmappable(_) => anyhow::bail!(unrepresentable_error(encoding)),
        }
    }
}

/// Whether `text` can be represented in `encoding`
pub fn can_encode(text: &str, encoding: &'static Encoding) -> bool {
    encoding == UTF_8 || text.is_ascii() || encode(text, encoding).is_ok()
}

/// Wraps a reader, failing with an error (see [`is_invalid_utf8_error`]) as soon as any of the
/// bytes read through it aren't valid UTF-8. This allows a file to be validated while it is being
/// streamed, rather than requiring a separate pass.
pub struct Utf8ValidatingReader<R> {
    inner: R,
    /// Bytes of an incomplete character at the end of the previous read
    carry: [u8; 4],
    carry_len: usize,
}

impl<R> Utf8ValidatingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            carry: [0; 4],
            carry_len: 0,
        }
    }

    /// Checks that `bytes`, following any bytes carried over from the previous read, are valid
    /// UTF-8, carrying over any incomplete character at the end
    fn validate(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        // Complete the character carried over from the previous read, one byte at a time
        while self.carry_len > 0 && !bytes.is_empty() {
            self.carry[self.carry_len] = bytes[0];
            self.carry_len += 1;
            bytes = &bytes[1..];
            match std::str::from_utf8(&self.carry[..self.carry_len]) {
                Ok(_) => self.carry_len = 0,
                Err(e) if e.error_len().is_none() => {}
                Err(_) => return Err(invalid_utf8_error()),
            }
        }
        match std::str::from_utf8(bytes) {
            Ok(_) => Ok(()),
            Err(e) if e.error_len().is_none() => {
                let rest = &bytes[e.valid_up_to()..];
                self.carry[..rest.len()].copy_from_slice(rest);
                self.carry_len = rest.len();
                Ok(())
            }
            Err(_) => Err(invalid_utf8_error()),
        }
    }
}

impl<R: Read> Read for Utf8ValidatingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let read = self.inner.read(buf)?;
        if read == 0 && self.carry_len > 0 {
            // The input ended part way through a character
            return Err(invalid_utf8_error());
        }
        self.validate(&buf[..read])?;
        Ok(read)
    }
}

#[derive(Debug)]
struct InvalidUtf8;

impl std::fmt::Display for InvalidUtf8 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Content is not valid UTF-8")
    }
}

impl std::error::Error for InvalidUtf8 {}

fn invalid_utf8_error() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, InvalidUtf8)
}

/// Whether `error` was caused by reading invalid UTF-8 from a [`Utf8ValidatingReader`]
pub fn is_invalid_utf8_error(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<io::Error>().and_then(io::Error::get_ref),
        Some(inner) if inner.is::<InvalidUtf8>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::{SHIFT_JIS, WINDOWS_1252};
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Decodes `bytes` (which mustn't be valid UTF-8) with the given legacy encoding, as
    /// detection is unreliable for the short samples used in tests
    fn decode_as(bytes: &[u8], encoding: &'static Encoding) -> DecodedFile {
        assert!(std::str::from_utf8(bytes).is_err());
        DecodedFile::NonUtf8(NonUtf8File::decode_with(bytes.to_vec(), Some(encoding)))
    }

    fn non_utf8(decoded: &DecodedFile) -> &NonUtf8File {
        match decoded {
            DecodedFile::NonUtf8(file) => file,
            DecodedFile::Utf8(_) => panic!("Expected non-UTF-8 file"),
        }
    }

    fn shift_jis(text: &str) -> Vec<u8> {
        let (bytes, _, had_errors) = SHIFT_JIS.encode(text);
        assert!(!had_errors);
        bytes.into_owned()
    }

    fn replace(decoded: &DecodedFile, replacements: &[(Range<usize>, &str)]) -> ReplacedFile {
        let replacements: Vec<_> = replacements
            .iter()
            .map(|(range, text)| Replacement {
                range: range.clone(),
                text: Cow::Borrowed(*text),
            })
            .collect();
        decoded.apply_replacements(&replacements).unwrap()
    }

    /// Returns the byte range of the `n`th (from 0) occurrence of `needle` in the decoded text
    fn nth_range(decoded: &DecodedFile, needle: &str, n: usize) -> Range<usize> {
        let (start, _) = decoded
            .text()
            .match_indices(needle)
            .nth(n)
            .unwrap_or_else(|| panic!("Couldn't find {needle:?} in {:?}", decoded.text()));
        start..start + needle.len()
    }

    #[test]
    fn test_decode_utf8() {
        let decoded = decode("mini était".as_bytes().to_vec());
        assert!(matches!(&decoded, DecodedFile::Utf8(text) if text == "mini était"));
    }

    #[test]
    fn test_decode_latin1() {
        let decoded = decode(b"mini \xe9tait\n".to_vec());
        assert_eq!(decoded.text(), "mini était\n");
        let file = non_utf8(&decoded);
        assert_eq!(file.legacy_encoding, Some(WINDOWS_1252));
        assert!(!file.has_utf8_lines);
        assert!(file.opaque_lines.is_empty());
    }

    #[test]
    fn test_decode_shift_jis() {
        let original = "こんにちは、世界。これは日本語のテキストです。\n";
        let decoded = decode(shift_jis(original));
        assert_eq!(decoded.text(), original);
        assert_eq!(non_utf8(&decoded).legacy_encoding, Some(SHIFT_JIS));
    }

    #[test]
    fn test_decode_mixed() {
        let decoded = decode_as(b"caf\xc3\xa9\nd\xe9j\xe0 vu\r\nascii", WINDOWS_1252);
        assert_eq!(decoded.text(), "café\ndéjà vu\r\nascii");
        let file = non_utf8(&decoded);
        assert!(file.has_utf8_lines);
        assert!(file.opaque_lines.is_empty());
    }

    #[test]
    fn test_decode_opaque_lines() {
        let mut bytes = shift_jis("これは日本語のテキストです。\n");
        // A lone lead byte, which isn't valid Shift_JIS
        bytes.extend(b"bad \x81\r\n");
        bytes.extend(shift_jis("日本語\n"));
        let decoded = decode_as(&bytes, SHIFT_JIS);
        assert_eq!(non_utf8(&decoded).opaque_lines, [1]);
        assert_eq!(
            decoded.text(),
            "これは日本語のテキストです。\nbad \u{FFFD}\r\n日本語\n"
        );
    }

    #[test]
    fn test_decode_all_bytes_round_trip() {
        // Every byte value other than NUL (which indicates binary content) should decode and
        // round-trip
        let bytes: Vec<u8> = (1..=255u8).collect();
        let decoded = decode(bytes.clone());
        assert!(non_utf8(&decoded).opaque_lines.is_empty());
        assert_eq!(replace(&decoded, &[]).bytes, bytes);
    }

    #[test]
    fn test_decode_text_rejects_unsupported_content() {
        // Binary data
        assert!(decode_text(b"mini \xe9tait\x00".to_vec()).is_none());
        assert!(decode_text(b"%PDF-1.4 \xe9".to_vec()).is_none());
        // UTF-16
        assert!(decode_text(b"\xff\xfem\x00i\x00".to_vec()).is_none());
        assert!(decode_text(b"\xfe\xff\x00m\x00i".to_vec()).is_none());

        assert!(decode_text(b"mini \xe9tait".to_vec()).is_some());
        assert!(decode_text(b"caf\xc3\xa9\nd\xe9j\xe0".to_vec()).is_some());
    }

    #[test]
    fn test_fast_path_matches_line_by_line_decoding() {
        let cases = [
            b"mini \xe9tait\r\nascii\n\nd\xe9j\xe0".to_vec(),
            shift_jis("こんにちは\r\n世界\nascii\n"),
        ];
        for bytes in cases {
            let decoded = decode(bytes.clone());
            let file = non_utf8(&decoded);
            assert_eq!(
                (decoded.text().to_owned(), vec![]),
                decode_lines(&bytes, file.legacy_encoding),
            );
        }
    }

    #[test]
    fn test_whole_file_decoding_matches_line_decoder() {
        // Multiline search decodes the whole file, whereas line-by-line search and previews decode
        // individual lines, so these must agree
        let mut shift_jis_with_bad_line =
            shift_jis("こんにちは\n世界。これは日本語のテキストです。\n");
        // Ends with a lone lead byte before the line ending
        shift_jis_with_bad_line.extend(b"bad \x81\r\nascii\n");
        let cases = [
            b"caf\xc3\xa9\nd\xe9j\xe0 vu\r\n\nascii".to_vec(),
            b"mini \xe9tait\r\nascii\n".to_vec(),
            shift_jis_with_bad_line,
        ];
        for bytes in cases {
            let mut file = NamedTempFile::new().unwrap();
            file.write_all(&bytes).unwrap();
            let line_decoder = LineDecoder::new(Some(file.path()));

            let by_line: String = split_lines(&bytes)
                .map(|(content, ending)| line_decoder.decode(content.to_vec()) + ending.as_str())
                .collect();
            assert_eq!(decode(bytes).text(), by_line);
        }
    }

    #[test]
    fn test_line_decoder() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"plain\nmini \xe9tait\nd\xe9j\xe0\n")
            .unwrap();
        let decoder = LineDecoder::new(Some(file.path()));
        assert_eq!(decoder.decode(b"plain".to_vec()), "plain");
        assert_eq!(decoder.decode("été".as_bytes().to_vec()), "été");
        assert_eq!(decoder.decode(b"mini \xe9tait".to_vec()), "mini était");
        assert_eq!(decoder.decode(b"d\xe9j\xe0".to_vec()), "déjà");

        let decoder = LineDecoder::new(None);
        assert_eq!(
            decoder.decode(b"mini \xe9tait".to_vec()),
            "mini \u{FFFD}tait"
        );
    }

    #[test]
    fn test_file_decoder() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"plain\nmini \xe9tait\n").unwrap();
        let decoder = FileDecoder::new(file.path());
        assert_eq!(
            decoder.decode_line(b"plain".to_vec()),
            DecodedLine {
                text: "plain".to_owned(),
                kind: LineKind::Utf8
            }
        );
        assert_eq!(
            decoder.decode_line(b"mini \xe9tait".to_vec()),
            DecodedLine {
                text: "mini était".to_owned(),
                kind: LineKind::Legacy(WINDOWS_1252)
            }
        );
        assert_eq!(
            decoder.decode_line(b"nul \xe9\x00".to_vec()).kind,
            LineKind::Opaque
        );
    }

    #[test]
    fn test_file_decoder_utf16() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"\xff\xfem\x00i\x00\n\x00").unwrap();
        let decoder = FileDecoder::new(file.path());
        assert_eq!(
            decoder.decode_line(b"\xff\xfem\x00i\x00".to_vec()).kind,
            LineKind::Opaque
        );
    }

    #[test]
    fn test_apply_replacements_latin1() {
        let decoded = decode_as(b"mini \xe9tait\nmini\n", WINDOWS_1252);
        let replaced = replace(
            &decoded,
            &[
                (nth_range(&decoded, "mini", 0), "été"),
                (nth_range(&decoded, "mini", 1), "déjà"),
            ],
        );
        assert_eq!(replaced.outcomes, [Ok(()), Ok(())]);
        // Replacements in ASCII lines use the file's encoding
        assert_eq!(replaced.bytes, b"\xe9t\xe9 \xe9tait\nd\xe9j\xe0\n");
    }

    #[test]
    fn test_apply_replacements_mixed() {
        let bytes = b"caf\xc3\xa9 mini\nd\xe9j\xe0 mini\nascii mini\n\xff\x00 mini\n";
        let decoded = decode_as(bytes, WINDOWS_1252);
        assert_eq!(non_utf8(&decoded).opaque_lines, [3]);
        let replaced = replace(
            &decoded,
            &[
                (nth_range(&decoded, "mini", 0), "été"),
                (nth_range(&decoded, "mini", 1), "été"),
                (nth_range(&decoded, "mini", 2), "été"),
                (nth_range(&decoded, "mini", 3), "été"),
            ],
        );
        assert_eq!(
            replaced.outcomes,
            [Ok(()), Ok(()), Ok(()), Err(OPAQUE_LINE_ERROR.to_owned())]
        );
        // Each line keeps its own encoding, and ASCII lines use UTF-8 as the file contains UTF-8
        assert_eq!(
            replaced.bytes,
            [
                "café été\n".as_bytes(),
                b"d\xe9j\xe0 \xe9t\xe9\n",
                "ascii été\n".as_bytes(),
                b"\xff\x00 mini\n",
            ]
            .concat()
        );
    }

    #[test]
    fn test_apply_replacements_unrepresentable() {
        let decoded = decode_as(b"mini \xe9tait\nmaxi\n", WINDOWS_1252);
        let replaced = replace(
            &decoded,
            &[
                (nth_range(&decoded, "mini", 0), "世界"),
                (nth_range(&decoded, "maxi", 0), "été"),
            ],
        );
        assert_eq!(
            replaced.outcomes,
            [Err(unrepresentable_error(WINDOWS_1252)), Ok(())]
        );
        assert_eq!(replaced.bytes, b"mini \xe9tait\n\xe9t\xe9\n");
    }

    #[test]
    fn test_apply_replacements_across_lines() {
        let bytes = b"ascii start\nd\xe9j\xe0 end\ncaf\xc3\xa9 start\nd\xe9j\xe0 end\n".to_vec();
        let decoded = decode_as(&bytes, WINDOWS_1252);
        let replaced = replace(
            &decoded,
            &[
                // ASCII line and legacy line
                (nth_range(&decoded, "start\ndéjà", 0), "é"),
                // UTF-8 line and legacy line
                (nth_range(&decoded, "start\ndéjà", 1), "é"),
            ],
        );
        assert_eq!(
            replaced.outcomes,
            [Ok(()), Err(MIXED_ENCODINGS_ERROR.to_owned())]
        );
        assert_eq!(
            replaced.bytes,
            [
                b"ascii \xe9 end\n".as_slice(),
                "café start\n".as_bytes(),
                b"d\xe9j\xe0 end\n",
            ]
            .concat()
        );
    }

    #[test]
    fn test_apply_replacements_consuming_line_ending() {
        // Consuming a line ending joins the following line, which must be included when checking
        // encodings, even though the match doesn't contain any of its text
        let bytes =
            b"caf\xc3\xa9 end\nd\xe9j\xe0\ncaf\xc3\xa9 end\r\nd\xe9j\xe0\nascii end\n\xff\x00\n";
        let decoded = decode_as(bytes, WINDOWS_1252);
        let matches = [
            // Joins a UTF-8 line and a legacy line
            nth_range(&decoded, "end\n", 0),
            // Consumes only the `\n` of a CRLF line ending, joining a UTF-8 and a legacy line
            nth_range(&decoded, "\n", 2),
            // Joins an ASCII line and an opaque line
            nth_range(&decoded, "end\n", 1),
        ];
        assert_eq!(
            &decoded.text()[..matches[1].end],
            "café end\ndéjà\ncafé end\r\n"
        );
        let replaced = replace(
            &decoded,
            &[
                (matches[0].clone(), ""),
                (matches[1].clone(), ""),
                (matches[2].clone(), ""),
            ],
        );
        assert_eq!(
            replaced.outcomes,
            [
                Err(MIXED_ENCODINGS_ERROR.to_owned()),
                Err(MIXED_ENCODINGS_ERROR.to_owned()),
                Err(OPAQUE_LINE_ERROR.to_owned())
            ]
        );
        assert_eq!(replaced.bytes, bytes);

        let mut items = matches.to_vec();
        decoded.retain_replaceable(&mut items, Clone::clone);
        assert_eq!(items, &matches[..2]);
    }

    #[test]
    fn test_apply_replacements_shared_lines_are_grouped() {
        let decoded = decode_as(b"a \xe9 b\nc\n", WINDOWS_1252);
        let replaced = replace(
            &decoded,
            &[
                (nth_range(&decoded, "a", 0), "é"),
                (nth_range(&decoded, "b\nc", 0), "è"),
            ],
        );
        assert_eq!(replaced.outcomes, [Ok(()), Ok(())]);
        assert_eq!(replaced.bytes, b"\xe9 \xe9 \xe8\n");
    }

    #[test]
    fn test_apply_replacements_insertions() {
        let decoded = decode_as(b"\xe9\n", WINDOWS_1252);
        let len = decoded.text().len();
        let replaced = replace(&decoded, &[(0..0, "è"), (len..len, "à")]);
        assert_eq!(replaced.outcomes, [Ok(()), Ok(())]);
        assert_eq!(replaced.bytes, b"\xe8\xe9\n\xe0");
    }

    #[test]
    fn test_apply_replacements_shift_jis_with_bad_line() {
        let mut bytes = shift_jis("日本語 one\n");
        bytes.extend(b"bad \x81 two\n");
        bytes.extend(shift_jis("日本語 three\n"));
        let decoded = decode_as(&bytes, SHIFT_JIS);
        let replaced = replace(
            &decoded,
            &[
                (nth_range(&decoded, "one", 0), "世界"),
                (nth_range(&decoded, "two", 0), "x"),
                (nth_range(&decoded, "three", 0), "世界"),
            ],
        );
        assert_eq!(
            replaced.outcomes,
            [Ok(()), Err(OPAQUE_LINE_ERROR.to_owned()), Ok(())]
        );
        let mut expected = shift_jis("日本語 世界\n");
        expected.extend(b"bad \x81 two\n");
        expected.extend(shift_jis("日本語 世界\n"));
        assert_eq!(replaced.bytes, expected);
    }

    #[test]
    fn test_apply_replacements_bom() {
        let decoded = decode_as(b"\xef\xbb\xbfmini\n\xe9\n", WINDOWS_1252);
        let replaced = replace(
            &decoded,
            &[(0..3, ""), (nth_range(&decoded, "mini", 0), "maxi")],
        );
        assert_eq!(replaced.outcomes, [Ok(()), Ok(())]);
        assert_eq!(replaced.bytes, b"maxi\n\xe9\n");
    }

    #[test]
    fn test_apply_replacements_utf8() {
        let decoded = decode(b"one two".to_vec());
        let replaced = replace(&decoded, &[(0..3, "1"), (4..7, "2")]);
        assert_eq!(replaced.outcomes, [Ok(()), Ok(())]);
        assert_eq!(replaced.bytes, b"1 2");
    }

    #[test]
    fn test_apply_replacements_rejects_invalid_ranges() {
        let decoded = decode_as(b"d\xe9j\xe0 vu", WINDOWS_1252);
        let invalid = [
            vec![(0..1, "x"), (0..1, "y")],
            vec![(Range { start: 2, end: 1 }, "x")],
            vec![(2..3, "x")],
            vec![(0..100, "x")],
        ];
        for replacements in invalid {
            let replacements: Vec<_> = replacements
                .into_iter()
                .map(|(range, text)| Replacement {
                    range,
                    text: Cow::Borrowed(text),
                })
                .collect();
            assert!(decoded.apply_replacements(&replacements).is_err());
        }
    }

    #[test]
    fn test_retain_replaceable() {
        let decoded = decode_as(b"a\n\xff\x00b\nc\nd", WINDOWS_1252);
        let text = decoded.text();
        let a = 0..1;
        let a_and_line_ending = 0..2;
        let b = nth_range(&decoded, "b", 0);
        let c = nth_range(&decoded, "c", 0);
        let insertion_before_c = c.start..c.start;
        let end = text.len()..text.len();
        let mut items = vec![
            a.clone(),
            a_and_line_ending,
            b,
            c.clone(),
            insertion_before_c.clone(),
            end.clone(),
        ];
        decoded.retain_replaceable(&mut items, Clone::clone);
        assert_eq!(items, [a, c, insertion_before_c, end]);
    }

    #[test]
    fn test_single_byte_encodings_round_trip() {
        // `decode_strict` relies on this to skip re-encoding for single-byte encodings
        let single_byte_encodings = [
            encoding_rs::IBM866,
            encoding_rs::ISO_8859_2,
            encoding_rs::ISO_8859_3,
            encoding_rs::ISO_8859_4,
            encoding_rs::ISO_8859_5,
            encoding_rs::ISO_8859_6,
            encoding_rs::ISO_8859_7,
            encoding_rs::ISO_8859_8,
            encoding_rs::ISO_8859_8_I,
            encoding_rs::ISO_8859_10,
            encoding_rs::ISO_8859_13,
            encoding_rs::ISO_8859_14,
            encoding_rs::ISO_8859_15,
            encoding_rs::ISO_8859_16,
            encoding_rs::KOI8_R,
            encoding_rs::KOI8_U,
            encoding_rs::MACINTOSH,
            encoding_rs::WINDOWS_874,
            encoding_rs::WINDOWS_1250,
            encoding_rs::WINDOWS_1251,
            encoding_rs::WINDOWS_1252,
            encoding_rs::WINDOWS_1253,
            encoding_rs::WINDOWS_1254,
            encoding_rs::WINDOWS_1255,
            encoding_rs::WINDOWS_1256,
            encoding_rs::WINDOWS_1257,
            encoding_rs::WINDOWS_1258,
            encoding_rs::X_MAC_CYRILLIC,
        ];
        for encoding in single_byte_encodings {
            assert!(encoding.is_single_byte());
            for byte in 1..=255u8 {
                let bytes = [byte];
                let Some(text) =
                    encoding.decode_without_bom_handling_and_without_replacement(&bytes)
                else {
                    continue;
                };
                assert_eq!(
                    encode(&text, encoding).unwrap(),
                    [byte],
                    "{} byte {byte:#x}",
                    encoding.name()
                );
            }
        }
    }

    #[test]
    fn test_encode_unmappable() {
        assert_eq!(encode("été", WINDOWS_1252).unwrap(), b"\xe9t\xe9");
        assert!(encode("世界", WINDOWS_1252).is_err());
        assert!(can_encode("é", WINDOWS_1252));
        assert!(!can_encode("世界", WINDOWS_1252));
        assert!(can_encode("世界", UTF_8));
    }

    #[test]
    fn test_split_lines() {
        let lines: Vec<_> = split_lines(b"a\r\nb\n\rc\r").collect();
        assert_eq!(
            lines,
            [
                (b"a".as_slice(), LineEnding::CrLf),
                (b"b", LineEnding::Lf),
                (b"\rc\r", LineEnding::None),
            ]
        );
        let lines: Vec<_> = split_lines(b"a\n").collect();
        assert_eq!(
            lines,
            [(b"a".as_slice(), LineEnding::Lf), (b"", LineEnding::None)]
        );
        let lines: Vec<_> = split_lines(b"").collect();
        assert_eq!(lines, [(b"".as_slice(), LineEnding::None)]);
    }

    fn validate_in_chunks(bytes: &[u8], chunk_size: usize) -> bool {
        let mut reader = Utf8ValidatingReader::new(bytes);
        let mut buf = vec![0; chunk_size];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => return true,
                Ok(_) => {}
                Err(e) => {
                    assert!(is_invalid_utf8_error(&e.into()));
                    return false;
                }
            }
        }
    }

    #[test]
    fn test_utf8_validating_reader() {
        let valid = "aé世🦀b".repeat(3);
        let cases: &[(&[u8], bool)] = &[
            (valid.as_bytes(), true),
            (b"", true),
            (b"mini \xe9tait", false),
            (b"abc\xc3", false),
            (b"\xf0\x9f\xa6", false),
            (b"\xf0\x9f\xa6x", false),
        ];
        for (bytes, expected) in cases {
            for chunk_size in 1..=8 {
                assert_eq!(
                    validate_in_chunks(bytes, chunk_size),
                    *expected,
                    "bytes={bytes:?}, chunk_size={chunk_size}"
                );
            }
        }
    }

    #[test]
    fn test_utf8_validating_reader_passes_through_content() {
        let content = "mini était 世界\n";
        let mut reader = Utf8ValidatingReader::new(content.as_bytes());
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert_eq!(out, content);
    }
}
