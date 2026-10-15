//! Turning file bytes into text: UTF-16 by its byte order mark, binary detection, BOM stripping
//! and lossy UTF-8.

/// How many leading bytes are checked for a NUL when deciding a file is binary.
const BINARY_SNIFF_LEN: usize = 8 * 1024;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";
const UTF16_LE_BOM: &[u8] = b"\xFF\xFE";
const UTF16_BE_BOM: &[u8] = b"\xFE\xFF";

/// A file's text, plus whether bytes that weren't text had to be replaced to produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    /// True when invalid UTF-8 sequences, or broken UTF-16, were replaced with U+FFFD. The UI
    /// shows a banner.
    pub lossy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The first 8 KiB contain a NUL byte, so nothing is rendered.
    Binary,
}

/// Decodes a file's bytes. One starting with a UTF-16 byte order mark is UTF-16
/// ([`decode_utf16`]), NULs and all. Anything else is UTF-8: a NUL in the first 8 KiB means
/// binary; a leading UTF-8 BOM is dropped; invalid UTF-8 is replaced and flagged as lossy.
pub fn decode(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    if let Some(decoded) = decode_utf16(bytes) {
        return Ok(decoded);
    }
    let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_LEN)];
    if sniff.contains(&0) {
        return Err(DecodeError::Binary);
    }
    Ok(decode_utf8(bytes))
}

/// Decodes a file's bytes as [`decode`] does, but never as binary: a NUL is a character like any
/// other. An HTML page's source is read so, as the browser reads the page whatever it holds.
pub fn decode_lossy(bytes: &[u8]) -> Decoded {
    decode_utf16(bytes).unwrap_or_else(|| decode_utf8(bytes))
}

/// `bytes` as UTF-8, a leading BOM dropped, invalid sequences replaced and flagged as lossy.
fn decode_utf8(bytes: &[u8]) -> Decoded {
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => Decoded {
            text: text.to_owned(),
            lossy: false,
        },
        Err(_) => Decoded {
            text: String::from_utf8_lossy(bytes).into_owned(),
            lossy: true,
        },
    }
}

/// The text of `bytes` when they start with a UTF-16 byte order mark (`FF FE` little-endian,
/// `FE FF` big-endian), the mark dropped; `None` for any other bytes. An unpaired surrogate, or an
/// odd last byte, becomes U+FFFD, and only then is the text lossy.
pub fn decode_utf16(bytes: &[u8]) -> Option<Decoded> {
    let little = bytes.starts_with(UTF16_LE_BOM);
    if !little && !bytes.starts_with(UTF16_BE_BOM) {
        return None;
    }
    let (pairs, odd) = bytes[2..].as_chunks::<2>();
    let units = pairs.iter().map(|&pair| {
        if little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    });
    let mut text = String::with_capacity(bytes.len());
    let mut lossy = !odd.is_empty();
    for unit in char::decode_utf16(units) {
        text.push(unit.unwrap_or_else(|_| {
            lossy = true;
            char::REPLACEMENT_CHARACTER
        }));
    }
    if !odd.is_empty() {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    Some(Decoded { text, lossy })
}
