use lectern_core::text::{decode, decode_lossy, decode_utf16, DecodeError, Decoded};

#[test]
fn binary_detected() {
    assert_eq!(
        decode(b"\x00\x05\x16\x07abc").err(),
        Some(DecodeError::Binary)
    );
}

#[test]
fn bom_stripped() {
    assert_eq!(decode("\u{feff}# Hi".as_bytes()).unwrap().text, "# Hi");
}

#[test]
fn lossy_flagged() {
    let d = decode(b"ok \xff\xfe ok").unwrap();
    assert!(d.lossy);
    assert!(d.text.contains("ok"));
}

#[test]
fn valid_utf8_is_not_lossy() {
    let d = decode("résumé ✅".as_bytes()).unwrap();
    assert!(!d.lossy);
    assert_eq!(d.text, "résumé ✅");
}

#[test]
fn nul_after_the_first_8_kib_is_not_binary() {
    let mut bytes = vec![b'a'; 8 * 1024];
    bytes.push(0);
    assert!(decode(&bytes).is_ok());

    let mut bytes = vec![b'a'; 8 * 1024 - 1];
    bytes.push(0);
    assert_eq!(decode(&bytes).err(), Some(DecodeError::Binary));
}

/// `text` in UTF-16, little-endian or big-endian, after its byte order mark.
fn utf16(text: &str, little: bool) -> Vec<u8> {
    let bom: [u8; 2] = if little { [0xFF, 0xFE] } else { [0xFE, 0xFF] };
    bom.into_iter()
        .chain(text.encode_utf16().flat_map(|u| {
            if little {
                u.to_le_bytes()
            } else {
                u.to_be_bytes()
            }
        }))
        .collect()
}

/// A file starting with a UTF-16 byte order mark is UTF-16, either way round, though its ASCII
/// is half NULs: the mark is dropped, and nothing is lossy.
#[test]
fn utf16_by_its_byte_order_mark_in_either_order() {
    let text = "# Café 日本 🎉\r\nLine two, with \u{fffd} as written.\n";
    for little in [true, false] {
        assert_eq!(
            decode(&utf16(text, little)),
            Ok(Decoded {
                text: text.to_owned(),
                lossy: false
            }),
            "little-endian: {little}"
        );
    }
    assert_eq!(decode(&[0xFF, 0xFE]).unwrap().text, "");
}

/// An unpaired surrogate and an odd last byte each become a replacement character, and only then
/// is the text lossy.
#[test]
fn broken_utf16_is_replaced_and_flagged() {
    let le = [0xFF, 0xFE, b'a', 0, 0x00, 0xD8, b'b', 0];
    assert_eq!(
        decode(&le),
        Ok(Decoded {
            text: "a\u{fffd}b".to_owned(),
            lossy: true
        })
    );
    let be = [0xFE, 0xFF, 0, b'a', 0, b'b', b'c'];
    assert_eq!(
        decode(&be),
        Ok(Decoded {
            text: "ab\u{fffd}".to_owned(),
            lossy: true
        })
    );
    assert_eq!(decode_utf16(b"# plain UTF-8"), None);
}

/// An HTML page's source is read whatever it holds, as the browser reads it: a NUL in its head
/// is a character like any other, and UTF-16 is still UTF-16.
#[test]
fn a_source_read_lossily_is_never_binary() {
    let page = b"\xEF\xBB\xBF<head><title>A\0B</title></head><p>Caf\xC3\xA9 \xFF</p>";
    assert_eq!(decode(page), Err(DecodeError::Binary));
    assert_eq!(
        decode_lossy(page),
        Decoded {
            text: "<head><title>A\0B</title></head><p>Café \u{fffd}</p>".to_owned(),
            lossy: true,
        }
    );
    assert_eq!(
        decode_lossy(b"<p>plain</p>"),
        Decoded {
            text: "<p>plain</p>".to_owned(),
            lossy: false,
        }
    );
    assert_eq!(
        decode_lossy(&utf16("<p>Café</p>", true)).text,
        "<p>Café</p>"
    );
}
