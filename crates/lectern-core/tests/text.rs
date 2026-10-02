use lectern_core::text::{decode, DecodeError};

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
