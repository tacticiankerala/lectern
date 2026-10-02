//! Turning file bytes into text: binary detection, BOM stripping and lossy UTF-8.

/// How many leading bytes are checked for a NUL when deciding a file is binary.
const BINARY_SNIFF_LEN: usize = 8 * 1024;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// A file's text, plus whether invalid UTF-8 had to be replaced to produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    /// True when invalid UTF-8 sequences were replaced with U+FFFD. The UI shows a banner.
    pub lossy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The first 8 KiB contain a NUL byte, so nothing is rendered.
    Binary,
}

/// Decodes a file's bytes as UTF-8. A NUL in the first 8 KiB means binary; a leading UTF-8 BOM is
/// dropped; invalid UTF-8 is replaced and flagged as lossy.
pub fn decode(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_LEN)];
    if sniff.contains(&0) {
        return Err(DecodeError::Binary);
    }
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    Ok(match std::str::from_utf8(bytes) {
        Ok(text) => Decoded {
            text: text.to_owned(),
            lossy: false,
        },
        Err(_) => Decoded {
            text: String::from_utf8_lossy(bytes).into_owned(),
            lossy: true,
        },
    })
}
