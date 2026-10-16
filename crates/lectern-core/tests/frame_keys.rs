//! Lectern's own shortcuts while the focus is inside a frame, where only Rust sees the keys: how a
//! Windows key press is named, which presses are Lectern's, and which of those a PDF's viewer
//! keeps. `ipc_export.rs` writes the whole table for the UI, whose test checks it against
//! `keymap.ts`.

use lectern_core::frame_keys::{decide, frame_key, keys, KeyDecision, Modifiers};
use lectern_core::ipc::{FrameFocus, FrameKey};

const NONE: Modifiers = Modifiers {
    ctrl: false,
    shift: false,
    alt: false,
};
const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };
const SHIFT: Modifiers = Modifiers {
    shift: true,
    ..NONE
};
const ALT: Modifiers = Modifiers { alt: true, ..NONE };
const CTRL_SHIFT: Modifiers = Modifiers {
    ctrl: true,
    shift: true,
    alt: false,
};
const CTRL_ALT: Modifiers = Modifiers {
    ctrl: true,
    shift: false,
    alt: true,
};

const VK_ESCAPE: u32 = 0x1B;
const VK_LEFT: u32 = 0x25;
const VK_F3: u32 = 0x72;
const VK_F5: u32 = 0x74;
const VK_F11: u32 = 0x7A;
const VK_ADD: u32 = 0x6B;
const VK_NUMPAD0: u32 = 0x60;
const VK_OEM_PLUS: u32 = 0xBB;
const VK_OEM_MINUS: u32 = 0xBD;
const VK_OEM_PERIOD: u32 = 0xBE;
const VK_LWIN: u32 = 0x5B;

fn vk(c: char) -> u32 {
    u32::from(c)
}

fn key(key: &str, code: &str, m: Modifiers) -> FrameKey {
    FrameKey {
        key: key.to_owned(),
        code: code.to_owned(),
        ctrl: m.ctrl,
        shift: m.shift,
        alt: m.alt,
    }
}

#[test]
fn a_key_press_is_named_as_a_keydown_would_name_it() {
    assert_eq!(frame_key(vk('O'), CTRL), Some(key("o", "KeyO", CTRL)));
    assert_eq!(
        frame_key(vk('O'), CTRL_SHIFT),
        Some(key("O", "KeyO", CTRL_SHIFT))
    );
    assert_eq!(frame_key(vk('0'), CTRL), Some(key("0", "Digit0", CTRL)));
    assert_eq!(
        frame_key(vk('0'), CTRL_SHIFT),
        Some(key(")", "Digit0", CTRL_SHIFT))
    );
    assert_eq!(frame_key(VK_F5, NONE), Some(key("F5", "F5", NONE)));
    assert_eq!(
        frame_key(VK_ESCAPE, NONE),
        Some(key("Escape", "Escape", NONE))
    );
    assert_eq!(
        frame_key(VK_LEFT, ALT),
        Some(key("ArrowLeft", "ArrowLeft", ALT))
    );
    assert_eq!(frame_key(VK_OEM_PLUS, CTRL), Some(key("=", "Equal", CTRL)));
    assert_eq!(
        frame_key(VK_OEM_PLUS, CTRL_SHIFT),
        Some(key("+", "Equal", CTRL_SHIFT))
    );
    assert_eq!(frame_key(VK_ADD, CTRL), Some(key("+", "NumpadAdd", CTRL)));
    assert_eq!(frame_key(VK_NUMPAD0, CTRL), Some(key("0", "Numpad0", CTRL)));
    assert_eq!(
        frame_key(VK_OEM_PERIOD, CTRL_SHIFT),
        Some(key(">", "Period", CTRL_SHIFT))
    );
    // A key with no name here (the Windows key) is never Lectern's.
    assert_eq!(frame_key(VK_LWIN, NONE), None);
}

fn decided(code: u32, m: Modifiers, focus: FrameFocus) -> Option<(String, bool)> {
    decide(code, m, focus, false).map(|KeyDecision { key, handled }| (key.key, handled))
}

/// AltGr reaches Windows as Ctrl+Alt: a Ctrl+Alt press that types a character on the keyboard
/// layout (AltGr+M is µ on some) is the page's, as keymap.ts leaves it typing.
#[test]
fn a_ctrl_alt_press_that_types_a_character_is_the_pages() {
    for focus in [FrameFocus::Page, FrameFocus::Pdf] {
        for code in [vk('M'), vk('0'), VK_OEM_PLUS, VK_OEM_MINUS] {
            assert_eq!(decide(code, CTRL_ALT, focus, true), None, "{code:#x}");
            assert!(decide(code, CTRL_ALT, focus, false).is_some(), "{code:#x}");
        }
    }
    // It asks only of Ctrl+Alt: Ctrl alone, or Alt alone, is Lectern's whatever a layout types.
    assert!(decide(vk('O'), CTRL, FrameFocus::Page, true).is_some());
    assert!(decide(VK_LEFT, ALT, FrameFocus::Page, true).is_some());
}

#[test]
fn nothing_is_decided_while_the_focus_is_in_lecterns_own_page() {
    for (code, m) in [(vk('O'), CTRL), (VK_F5, NONE), (VK_ESCAPE, NONE)] {
        assert_eq!(decide(code, m, FrameFocus::None, false), None);
    }
}

#[test]
fn in_a_page_lecterns_shortcuts_are_handled_and_esc_goes_through_too() {
    let page = FrameFocus::Page;
    for (code, m, name) in [
        (vk('O'), CTRL, "o"),
        (vk('F'), CTRL, "f"),
        (vk('P'), CTRL, "p"),
        (vk('H'), CTRL, "h"),
        (vk('M'), CTRL_ALT, "m"),
        (VK_F3, NONE, "F3"),
        (VK_F3, SHIFT, "F3"),
        (VK_F5, NONE, "F5"),
        (VK_F11, NONE, "F11"),
        (VK_LEFT, ALT, "ArrowLeft"),
        (VK_OEM_PLUS, CTRL, "="),
        (VK_OEM_MINUS, CTRL_SHIFT, "_"),
        (vk('0'), CTRL, "0"),
        // Copying the file's path.
        (vk('C'), CTRL_SHIFT, "C"),
    ] {
        assert_eq!(
            decided(code, m, page),
            Some((name.to_owned(), true)),
            "{name}"
        );
    }
    // Esc is Lectern's (it leaves focus mode) and the page's too: never handled.
    assert_eq!(
        decided(VK_ESCAPE, NONE, page),
        Some(("Escape".to_owned(), false))
    );
    // Keys that aren't Lectern's are the page's alone.
    for (code, m) in [
        (vk('A'), CTRL),
        (vk('C'), CTRL),
        (vk('V'), CTRL),
        (vk('0'), CTRL_SHIFT),
        // Copied a path in Windows or WSL form in v0.3.1 to v0.4.2; free since.
        (vk('W'), CTRL_SHIFT),
        (vk('L'), CTRL_SHIFT),
        (VK_LEFT, NONE),
        (VK_F5, CTRL),
        (vk('O'), CTRL_ALT),
    ] {
        assert_eq!(decide(code, m, page, false), None, "{code:#x}");
    }
}

#[test]
fn in_a_pdf_the_viewer_keeps_its_find_print_and_zoom() {
    let pdf = FrameFocus::Pdf;
    for (code, m) in [
        (vk('F'), CTRL),
        (VK_F3, NONE),
        (VK_F3, SHIFT),
        (vk('P'), CTRL),
        (VK_OEM_PLUS, CTRL),
        (VK_OEM_PLUS, CTRL_SHIFT),
        (VK_ADD, CTRL),
        (VK_OEM_MINUS, CTRL),
        (vk('0'), CTRL),
        (VK_NUMPAD0, CTRL),
    ] {
        assert_eq!(decide(code, m, pdf, false), None, "{code:#x} {m:?}");
    }
    // The rest of Lectern's are Lectern's, there too.
    for (code, m, name) in [
        (VK_F5, NONE, "F5"),
        (VK_F11, NONE, "F11"),
        (vk('F'), CTRL_SHIFT, "F"),
        (vk('O'), CTRL, "o"),
        (vk('H'), CTRL, "h"),
        (VK_OEM_PLUS, CTRL_ALT, "="),
    ] {
        assert_eq!(
            decided(code, m, pdf),
            Some((name.to_owned(), true)),
            "{name}"
        );
    }
    assert_eq!(
        decided(VK_ESCAPE, NONE, pdf),
        Some(("Escape".to_owned(), false))
    );
}

#[test]
fn the_table_lists_every_key_named_with_the_modifiers_it_is_lecterns_with() {
    let table = keys();
    let o = table.iter().find(|k| k.code == "KeyO").unwrap();
    assert_eq!((o.key.as_str(), o.shifted.as_str()), ("o", "O"));
    assert_eq!(o.lectern, ["Ctrl", "Ctrl+Shift"]);
    let escape = table.iter().find(|k| k.code == "Escape").unwrap();
    assert_eq!(escape.lectern, ["plain", "Shift"]);
    let a = table.iter().find(|k| k.code == "KeyA").unwrap();
    assert!(a.lectern.is_empty());
    // The table agrees with `decide` on every key and set of modifiers.
    for entry in &table {
        for (name, m) in [
            ("plain", NONE),
            ("Shift", SHIFT),
            ("Ctrl", CTRL),
            ("Ctrl+Shift", CTRL_SHIFT),
            ("Alt", ALT),
            (
                "Alt+Shift",
                Modifiers {
                    alt: true,
                    shift: true,
                    ctrl: false,
                },
            ),
            ("Ctrl+Alt", CTRL_ALT),
            (
                "Ctrl+Alt+Shift",
                Modifiers {
                    ctrl: true,
                    shift: true,
                    alt: true,
                },
            ),
        ] {
            let listed = entry.lectern.contains(&name);
            assert_eq!(
                decide(entry.vk, m, FrameFocus::Page, false).is_some(),
                listed,
                "{} {name}",
                entry.code
            );
        }
    }
}
