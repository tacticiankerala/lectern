//! Lectern's own shortcuts while the focus is inside a frame (an HTML page or a PDF). There, keys
//! go to the frame's document, which Lectern's page can't listen to, so Rust watches them in
//! WebView2 (`AcceleratorKeyPressed`, which sees Ctrl and Alt chords and keys that type nothing)
//! and tells the window about Lectern's (`frame-key`). They come from the host, never from the
//! page: a page can't forge one.
//!
//! A press is named as a `keydown` would name it on a US layout (`frame_key`), and is Lectern's
//! when `keymap.ts` gives it an action. `CHORDS` lists those; `ipc_export.rs` writes the whole
//! table (`keys`) for the UI, whose test checks it against `keymap.ts`. Rust marks Lectern's
//! handled, so the page never sees them, but Esc, which is the page's as well; and in a PDF, the
//! viewer keeps its own find, print and zoom keys. A Ctrl+Alt press that types a character (AltGr,
//! on layouts that have it) is the page's.

use crate::ipc::{FrameFocus, FrameKey};

/// The modifiers held with a key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

const C: u8 = 1;
const S: u8 = 2;
const A: u8 = 4;

/// Lectern's shortcuts (`keymap.ts`), by the key as `frame_key` names it and the modifiers held
/// (`C` Ctrl, `S` Shift, `A` Alt).
const CHORDS: &[(&str, u8)] = &[
    ("Escape", 0),
    ("Escape", S),
    ("F11", 0),
    ("F5", 0),
    ("F3", 0),
    ("F3", S),
    ("BrowserBack", 0),
    ("BrowserForward", 0),
    ("ArrowLeft", A),
    ("ArrowRight", A),
    // The text size, and with Alt, the sidebars'.
    ("=", C),
    ("=", C | A),
    ("+", C),
    ("+", C | S),
    ("+", C | A),
    ("+", C | A | S),
    ("-", C),
    ("-", C | S),
    ("-", C | A),
    ("-", C | A | S),
    ("_", C | S),
    ("_", C | A | S),
    ("0", C),
    ("0", C | A),
    ("m", C | A),
    (">", C | S),
    ("o", C),
    ("O", C | S),
    ("n", C),
    ("N", C | S),
    ("p", C),
    ("f", C),
    ("F", C | S),
    ("T", C | S),
    ("b", C),
    ("e", C),
    ("C", C | S),
    ("L", C | S),
    ("r", C),
    (",", C),
    ("M", C | S),
    ("q", C),
];

/// Keys named by a table rather than a range: virtual-key code, `key` without and with Shift,
/// and `code`.
const NAMED: &[(u32, &str, &str, &str)] = &[
    (0x08, "Backspace", "Backspace", "Backspace"),
    (0x09, "Tab", "Tab", "Tab"),
    (0x0D, "Enter", "Enter", "Enter"),
    (0x1B, "Escape", "Escape", "Escape"),
    (0x20, " ", " ", "Space"),
    (0x21, "PageUp", "PageUp", "PageUp"),
    (0x22, "PageDown", "PageDown", "PageDown"),
    (0x23, "End", "End", "End"),
    (0x24, "Home", "Home", "Home"),
    (0x25, "ArrowLeft", "ArrowLeft", "ArrowLeft"),
    (0x26, "ArrowUp", "ArrowUp", "ArrowUp"),
    (0x27, "ArrowRight", "ArrowRight", "ArrowRight"),
    (0x28, "ArrowDown", "ArrowDown", "ArrowDown"),
    (0x2D, "Insert", "Insert", "Insert"),
    (0x2E, "Delete", "Delete", "Delete"),
    (0x6A, "*", "*", "NumpadMultiply"),
    (0x6B, "+", "+", "NumpadAdd"),
    (0x6D, "-", "-", "NumpadSubtract"),
    (0x6E, ".", ".", "NumpadDecimal"),
    (0x6F, "/", "/", "NumpadDivide"),
    (0xA6, "BrowserBack", "BrowserBack", "BrowserBack"),
    (0xA7, "BrowserForward", "BrowserForward", "BrowserForward"),
    (0xBA, ";", ":", "Semicolon"),
    (0xBB, "=", "+", "Equal"),
    (0xBC, ",", "<", "Comma"),
    (0xBD, "-", "_", "Minus"),
    (0xBE, ".", ">", "Period"),
    (0xBF, "/", "?", "Slash"),
    (0xC0, "`", "~", "Backquote"),
    (0xDB, "[", "{", "BracketLeft"),
    (0xDC, "\\", "|", "Backslash"),
    (0xDD, "]", "}", "BracketRight"),
    (0xDE, "'", "\"", "Quote"),
];

/// What a US layout types with Shift on the digits 0 to 9.
const SHIFTED_DIGITS: [&str; 10] = [")", "!", "@", "#", "$", "%", "^", "&", "*", "("];

/// A key's names: `key` without and with Shift, and `code`.
fn names(vk: u32) -> Option<(String, String, String)> {
    let digit = |base: u32| char::from_digit(vk - base, 10).map(String::from);
    match vk {
        0x30..=0x39 => {
            let d = digit(0x30)?;
            let shifted = SHIFTED_DIGITS[usize::try_from(vk - 0x30).ok()?];
            Some((d.clone(), shifted.to_owned(), format!("Digit{d}")))
        }
        0x41..=0x5A => {
            let upper = char::from_u32(vk)?;
            let lower = upper.to_ascii_lowercase();
            Some((lower.into(), upper.into(), format!("Key{upper}")))
        }
        0x60..=0x69 => {
            let d = digit(0x60)?;
            Some((d.clone(), d.clone(), format!("Numpad{d}")))
        }
        0x70..=0x87 => {
            let f = format!("F{}", vk - 0x6F);
            Some((f.clone(), f.clone(), f))
        }
        _ => NAMED
            .iter()
            .find(|(code, ..)| *code == vk)
            .map(|&(_, key, shifted, code)| (key.into(), shifted.into(), code.into())),
    }
}

/// The key with virtual-key code `vk`, pressed with `m`, as a `keydown` names it on a US layout;
/// `None` for a key with no name here.
pub fn frame_key(vk: u32, m: Modifiers) -> Option<FrameKey> {
    let (key, shifted, code) = names(vk)?;
    Some(FrameKey {
        key: if m.shift { shifted } else { key },
        code,
        ctrl: m.ctrl,
        shift: m.shift,
        alt: m.alt,
    })
}

fn bits(k: &FrameKey) -> u8 {
    [(k.ctrl, C), (k.shift, S), (k.alt, A)]
        .iter()
        .filter(|(held, _)| *held)
        .fold(0, |bits, (_, bit)| bits | bit)
}

/// Whether `k` is one of Lectern's shortcuts (`keymap.ts`).
fn is_lecterns(k: &FrameKey) -> bool {
    let held = bits(k);
    CHORDS
        .iter()
        .any(|&(key, mods)| key == k.key && mods == held)
}

/// Whether a PDF's viewer keeps `k`, one of Lectern's: its find (Ctrl+F, F3 and Shift+F3), print
/// (Ctrl+P) and zoom (Ctrl with plus, minus or 0).
fn viewer_keeps(k: &FrameKey) -> bool {
    match (k.ctrl, k.alt) {
        (false, false) => k.key == "F3",
        (true, false) => {
            matches!(k.key.as_str(), "=" | "+" | "-" | "_")
                || (!k.shift && matches!(k.key.as_str(), "f" | "p" | "0"))
        }
        _ => false,
    }
}

/// What to do with a key pressed inside a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDecision {
    /// What to tell the window (`frame-key`).
    pub key: FrameKey,
    /// Kept from the frame's document.
    pub handled: bool,
}

/// What to do with the key `vk`, pressed with `m` while the focus is `focus`: `None` unless the
/// focus is in a frame and the key is one of Lectern's shortcuts, which a PDF's viewer doesn't
/// keep; then it is told to the window, and handled unless it is Esc.
///
/// AltGr reaches Windows as Ctrl+Alt, so a Ctrl+Alt press that `types_character` on the keyboard
/// layout (AltGr+M is µ on some) is the frame's, as keymap.ts leaves it typing. Only a Ctrl+Alt
/// press is asked about.
pub fn decide(
    vk: u32,
    m: Modifiers,
    focus: FrameFocus,
    types_character: bool,
) -> Option<KeyDecision> {
    if focus == FrameFocus::None || (m.ctrl && m.alt && types_character) {
        return None;
    }
    let key = frame_key(vk, m)?;
    if !is_lecterns(&key) || (focus == FrameFocus::Pdf && viewer_keeps(&key)) {
        return None;
    }
    let handled = key.key != "Escape";
    Some(KeyDecision { key, handled })
}

/// A key in the table `keys` gives: its names, and the sets of modifiers (`plain`, `Shift`,
/// `Ctrl`, `Ctrl+Shift`, `Alt`, `Alt+Shift`, `Ctrl+Alt`, `Ctrl+Alt+Shift`) it is Lectern's with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEntry {
    pub vk: u32,
    pub key: String,
    pub shifted: String,
    pub code: String,
    pub lectern: Vec<&'static str>,
}

/// Every set of modifiers, by name.
const MODIFIER_SETS: [(&str, u8); 8] = [
    ("plain", 0),
    ("Shift", S),
    ("Ctrl", C),
    ("Ctrl+Shift", C | S),
    ("Alt", A),
    ("Alt+Shift", A | S),
    ("Ctrl+Alt", C | A),
    ("Ctrl+Alt+Shift", C | A | S),
];

/// Every key with a name, by virtual-key code, with the modifiers it is Lectern's with.
pub fn keys() -> Vec<KeyEntry> {
    (0..=0xFF)
        .filter_map(|vk| {
            let (key, shifted, code) = names(vk)?;
            let lectern = MODIFIER_SETS
                .iter()
                .filter(|&&(_, mods)| {
                    let m = Modifiers {
                        ctrl: mods & C != 0,
                        shift: mods & S != 0,
                        alt: mods & A != 0,
                    };
                    frame_key(vk, m).is_some_and(|k| is_lecterns(&k))
                })
                .map(|&(name, _)| name)
                .collect();
            Some(KeyEntry {
                vk,
                key,
                shifted,
                code,
                lectern,
            })
        })
        .collect()
}
