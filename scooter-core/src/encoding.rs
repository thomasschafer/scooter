//! Support for text files that aren't UTF-8 encoded, such as those saved as Latin-1 or
//! Windows-1252.
//!
//! Text is read as UTF-8 wherever possible. Text that isn't valid UTF-8 is decoded using the
//! file's detected encoding so that it can be searched, and any replacements are encoded back into
//! that encoding before being written. An encoding is only used if decoding and then re-encoding
//! reproduces the original bytes exactly, so content is never silently altered.
//!
//! When searching line by line, only the lines that aren't valid UTF-8 are decoded with the
//! detected encoding, and detection only happens once such a line is found. This means that UTF-8
//! files are handled exactly as before, and the lines of files with mixed encodings that were
//! already searchable continue to be treated as UTF-8.
use std::{
    cell::OnceCell,
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use anyhow::Context;
use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use content_inspector::inspect;
use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};

/// The contents of a file decoded to UTF-8, along with the encoding used by the file on disk
#[derive(Debug)]
pub struct DecodedText {
    pub text: String,
    pub encoding: &'static Encoding,
}

/// Decodes `bytes` as UTF-8 if valid, otherwise using the detected encoding of the content.
///
/// Returns `None` if no suitable encoding could be found, e.g. for UTF-16 content.
pub fn decode(bytes: Vec<u8>) -> Option<DecodedText> {
    match String::from_utf8(bytes) {
        Ok(text) => Some(DecodedText {
            text,
            encoding: UTF_8,
        }),
        Err(e) => {
            let bytes = e.into_bytes();
            decode_legacy(&bytes, detect_legacy_encoding(&bytes)?)
        }
    }
}

/// Detects the encoding of `bytes` (typically the full contents of a file), assuming it isn't
/// UTF-8. Returns `None` if the content looks like binary data, or has a UTF-16 byte order mark.
fn detect_legacy_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    // Text in the ASCII-compatible encodings we support never contains NUL bytes, so this
    // avoids treating binary data as text (and is fast, exiting at the first NUL)
    if bytes.contains(&0) || inspect(bytes).is_binary() {
        return None;
    }
    detect_legacy_encoding_streaming(bytes).ok().flatten()
}

/// Detects the encoding of the content of `reader`, assuming it isn't UTF-8. Returns `None` if
/// the content has a UTF-16 byte order mark, or a NUL byte is found (indicating binary data).
///
/// Detection is relatively slow, so only a sample of the content is used: ASCII-only lines don't
/// affect the result, so only lines containing non-ASCII bytes are included, up to a limit. This
/// means that only as much of the content as is needed is read.
fn detect_legacy_encoding_streaming(
    mut reader: impl BufRead,
) -> io::Result<Option<&'static Encoding>> {
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    let mut remaining = MAX_DETECTION_SAMPLE_BYTES;
    let mut line = Vec::new();
    let mut is_first_line = true;

    while remaining > 0 {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
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
    detector.feed(&[], true);
    Ok(Some(detector.guess(None, Utf8Detection::Deny)))
}

/// Maximum number of bytes used to detect the encoding of a file
const MAX_DETECTION_SAMPLE_BYTES: usize = 1024 * 1024;

/// Decodes `bytes` with `encoding`, provided that the result can be encoded back into exactly the
/// same bytes.
///
/// Windows-1252 maps every byte, so is used as a fallback allowing at least ASCII text to be
/// searched and replaced when `encoding` doesn't round-trip.
fn decode_legacy(bytes: &[u8], encoding: &'static Encoding) -> Option<DecodedText> {
    [encoding, WINDOWS_1252].into_iter().find_map(|encoding| {
        if !encoding.is_ascii_compatible() {
            return None;
        }
        let text = encoding
            .decode_without_bom_handling_and_without_replacement(bytes)?
            .into_owned();
        // Single-byte encodings map each byte to a distinct character, so always round-trip
        (encoding.is_single_byte() || encode(&text, encoding).ok()? == bytes)
            .then_some(DecodedText { text, encoding })
    })
}

/// Decodes the lines of a file that aren't valid UTF-8.
///
/// The encoding of the file is detected the first time that a line is decoded, so this has no
/// cost for UTF-8 files.
pub struct LegacyLineDecoder<P: AsRef<Path>> {
    path: P,
    /// The detected encoding of the file, or `None` if it has no supported encoding
    encoding: OnceCell<Option<&'static Encoding>>,
}

impl<P: AsRef<Path>> LegacyLineDecoder<P> {
    pub fn new(path: P) -> Self {
        Self {
            path,
            encoding: OnceCell::new(),
        }
    }

    /// The detected encoding of the file, assuming that it isn't UTF-8. Returns `None` if the
    /// file has no supported encoding.
    pub fn encoding(&self) -> Option<&'static Encoding> {
        let path = self.path.as_ref();
        *self.encoding.get_or_init(|| {
            File::open(path)
                .and_then(|file| detect_legacy_encoding_streaming(BufReader::new(file)))
                .ok()
                .flatten()
        })
    }

    /// Decodes `line`, which should be a line from the file that isn't valid UTF-8, returning
    /// the text along with the encoding needed to encode it (or a replacement) back. Returns
    /// `None` for lines that look like binary data.
    pub fn decode(&self, line: &[u8]) -> Option<DecodedText> {
        if line.contains(&0) {
            return None;
        }
        decode_legacy(line, self.encoding()?)
    }
}

/// Reads the file at `path` and decodes it (see [`decode`])
pub fn read_to_string(path: &Path) -> anyhow::Result<DecodedText> {
    let bytes = std::fs::read(path)?;
    decode(bytes).with_context(|| format!("Unsupported file encoding: {}", path.display()))
}

/// Encodes `text` using `encoding`, failing if `text` contains characters that can't be
/// represented in that encoding
pub fn encode(text: &str, encoding: &'static Encoding) -> anyhow::Result<Vec<u8>> {
    if encoding == UTF_8 {
        return Ok(text.as_bytes().to_vec());
    }
    let (bytes, _, had_unmappable_chars) = encoding.encode(text);
    anyhow::ensure!(
        !had_unmappable_chars,
        "Text contains characters that can't be represented in the file's encoding ({})",
        encoding.name()
    );
    Ok(bytes.into_owned())
}

/// Whether `text` can be represented in `encoding`
pub fn can_encode(text: &str, encoding: &'static Encoding) -> bool {
    encoding == UTF_8 || text.is_ascii() || encode(text, encoding).is_ok()
}

/// Wraps a reader, checking whether all bytes read through it form valid UTF-8.
///
/// This allows a file to be validated while it is being streamed, rather than requiring a
/// separate pass.
pub struct Utf8ValidatingReader<R> {
    inner: R,
    /// Bytes of an incomplete character at the end of the previous read
    carry: [u8; 4],
    carry_len: usize,
    valid: bool,
}

impl<R> Utf8ValidatingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            carry: [0; 4],
            carry_len: 0,
            valid: true,
        }
    }

    /// Whether all bytes read so far are valid UTF-8. Should be called once the reader has been
    /// read to the end, otherwise a character split across the final read boundary is treated
    /// as invalid.
    pub fn is_valid(&self) -> bool {
        self.valid && self.carry_len == 0
    }

    fn validate(&mut self, mut bytes: &[u8]) {
        if !self.valid {
            return;
        }
        // Complete the character carried over from the previous read, one byte at a time
        while self.carry_len > 0 && !bytes.is_empty() {
            self.carry[self.carry_len] = bytes[0];
            self.carry_len += 1;
            bytes = &bytes[1..];
            match std::str::from_utf8(&self.carry[..self.carry_len]) {
                Ok(_) => self.carry_len = 0,
                Err(e) if e.error_len().is_none() => {}
                Err(_) => {
                    self.valid = false;
                    return;
                }
            }
        }
        match std::str::from_utf8(bytes) {
            Ok(_) => {}
            Err(e) if e.error_len().is_none() => {
                let rest = &bytes[e.valid_up_to()..];
                self.carry[..rest.len()].copy_from_slice(rest);
                self.carry_len = rest.len();
            }
            Err(_) => self.valid = false,
        }
    }
}

impl<R: Read> Read for Utf8ValidatingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.validate(&buf[..read]);
        Ok(read)
    }
}

/// Decodes the lines of a file for display, using the file's detected encoding for lines that
/// aren't valid UTF-8 and falling back to replacing invalid sequences with �.
pub struct LineDecoder {
    legacy: Option<LegacyLineDecoder<PathBuf>>,
}

impl LineDecoder {
    /// Creates a decoder for lines of the file at `path`. If `path` is `None`, invalid UTF-8 is
    /// always replaced with �.
    pub fn new(path: Option<&Path>) -> Self {
        Self {
            legacy: path.map(|path| LegacyLineDecoder::new(path.to_path_buf())),
        }
    }

    pub fn decode(&mut self, bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).unwrap_or_else(|e| {
            self.legacy
                .as_ref()
                .and_then(|legacy| legacy.decode(e.as_bytes()))
                .map_or_else(
                    || String::from_utf8_lossy(e.as_bytes()).into_owned(),
                    |decoded| decoded.text,
                )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_decode_utf8() {
        let decoded = decode("mini était".as_bytes().to_vec()).unwrap();
        assert_eq!(decoded.text, "mini était");
        assert_eq!(decoded.encoding, UTF_8);
    }

    #[test]
    fn test_decode_latin1() {
        let decoded = decode(b"mini \xe9tait\n".to_vec()).unwrap();
        assert_eq!(decoded.text, "mini était\n");
        assert_eq!(decoded.encoding, WINDOWS_1252);
    }

    #[test]
    fn test_decode_shift_jis() {
        let original = "こんにちは、世界。これは日本語のテキストです。\n";
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(original);
        let decoded = decode(bytes.into_owned()).unwrap();
        assert_eq!(decoded.text, original);
        assert_eq!(decoded.encoding, encoding_rs::SHIFT_JIS);
    }

    #[test]
    fn test_decode_all_bytes_round_trip() {
        // Every byte value other than NUL (which indicates binary content) should decode and
        // round-trip
        let bytes: Vec<u8> = (1..=255u8).collect();
        let decoded = decode(bytes.clone()).unwrap();
        assert_eq!(encode(&decoded.text, decoded.encoding).unwrap(), bytes);
    }

    #[test]
    fn test_legacy_line_decoder() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"plain\nmini \xe9tait\n").unwrap();
        let mut decoder = LegacyLineDecoder::new(file.path());
        let decoded = decoder.decode(b"mini \xe9tait").unwrap();
        assert_eq!(decoded.text, "mini était");
        assert_eq!(decoded.encoding, WINDOWS_1252);
    }

    #[test]
    fn test_legacy_line_decoder_rejects_utf16() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"\xff\xfem\x00i\x00\n\x00").unwrap();
        let mut decoder = LegacyLineDecoder::new(file.path());
        assert!(decoder.decode(b"\xff\xfem\x00i\x00").is_none());
    }

    #[test]
    fn test_single_byte_encodings_round_trip() {
        // `decode_legacy` relies on this to skip re-encoding for single-byte encodings
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
    fn test_decode_rejects_binary() {
        assert!(decode(b"mini \xe9tait\x00".to_vec()).is_none());
        assert!(decode(b"%PDF-1.4 \xe9".to_vec()).is_none());
    }

    #[test]
    fn test_decode_rejects_utf16() {
        assert!(decode(b"\xff\xfem\x00i\x00".to_vec()).is_none());
        assert!(decode(b"\xfe\xff\x00m\x00i".to_vec()).is_none());
    }

    #[test]
    fn test_encode_unmappable() {
        assert_eq!(encode("été", WINDOWS_1252).unwrap(), b"\xe9t\xe9");
        assert!(encode("世界", WINDOWS_1252).is_err());
        assert!(can_encode("é", WINDOWS_1252));
        assert!(!can_encode("世界", WINDOWS_1252));
        assert!(can_encode("世界", UTF_8));
    }

    fn validate_in_chunks(bytes: &[u8], chunk_size: usize) -> bool {
        let mut reader = Utf8ValidatingReader::new(bytes);
        let mut buf = vec![0; chunk_size];
        while reader.read(&mut buf).unwrap() > 0 {}
        reader.is_valid()
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
        assert!(reader.is_valid());
    }

    #[test]
    fn test_line_decoder() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"plain\nmini \xe9tait\nd\xe9j\xe0\n")
            .unwrap();
        let mut decoder = LineDecoder::new(Some(file.path()));
        assert_eq!(decoder.decode(b"plain".to_vec()), "plain");
        assert_eq!(decoder.decode("été".as_bytes().to_vec()), "été");
        assert_eq!(decoder.decode(b"mini \xe9tait".to_vec()), "mini était");
        assert_eq!(decoder.decode(b"d\xe9j\xe0".to_vec()), "déjà");

        let mut decoder = LineDecoder::new(None);
        assert_eq!(
            decoder.decode(b"mini \xe9tait".to_vec()),
            "mini \u{FFFD}tait"
        );
    }
}
