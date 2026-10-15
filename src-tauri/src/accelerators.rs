//! Lectern's shortcuts while the focus is inside a frame (an HTML page or a PDF). Keys there go to
//! the frame's document, which Lectern's page can't hear, so each window's WebView2 controller
//! reports them here (`AcceleratorKeyPressed`: Ctrl and Alt chords, and keys that type nothing,
//! in every frame). The window's state decides from where its UI says the focus is
//! (`WindowState::frame_key`, `lectern_core::frame_keys`): Lectern's own go to its UI as a
//! `frame-key`, and are kept from the frame but for Esc. A page can't forge one: it comes from the
//! host. AltGr reaches here as Ctrl+Alt, so a Ctrl+Alt press is asked of the keyboard layout first:
//! one that types a character there is the page's.
//!
//! Lectern targets Windows only, so this module is compiled unconditionally.

use std::sync::Arc;

use lectern_core::frame_keys::Modifiers;
use tauri::{Manager, WebviewWindow};
use webview2_com::AcceleratorKeyPressedEventHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, GetKeyboardState, MapVirtualKeyExW, ToUnicodeEx,
    MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
};

use crate::state::App;

/// Has `window`'s webview report the keys pressed in it to the window's state, from now on.
pub fn watch(window: &WebviewWindow) {
    let handle = window.app_handle().clone();
    let label = window.label().to_owned();
    let watched = window.with_webview(move |webview| {
        let controller = webview.controller();
        let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
            let mut vk = 0u32;
            // SAFETY: plain out-parameters of the event's own arguments, valid for this call.
            unsafe {
                args.KeyEventKind(&mut kind)?;
                args.VirtualKey(&mut vk)?;
            }
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
            {
                return Ok(());
            }
            let Some(state) = handle
                .try_state::<Arc<App>>()
                .and_then(|app| app.window(&label))
            else {
                return Ok(());
            };
            let mods = held();
            let types = mods.ctrl && mods.alt && types_character(vk);
            if state.frame_key(vk, mods, types) {
                // SAFETY: as above.
                unsafe { args.SetHandled(true)? };
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: the controller is the webview's own, alive on the thread this runs on; the
        // handler lives as long as the controller holds it.
        if let Err(e) = unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) } {
            log::warn!("couldn't watch the keys pressed in frames: {e}");
        }
    });
    if let Err(e) = watched {
        log::warn!("couldn't reach {}'s webview: {e}", window.label());
    }
}

/// `ToUnicodeEx`'s flag that leaves the keyboard state as it was, so a dead key isn't spent.
const KEEP_KEYBOARD_STATE: u32 = 0x4;

/// Whether the key `vk`, with the keys held now, types a character on the thread's keyboard
/// layout (a dead key counts): AltGr+M is µ on some layouts. Control characters don't count.
fn types_character(vk: u32) -> bool {
    let mut keys = [0u8; 256];
    let mut typed = [0u16; 8];
    // SAFETY: reads this thread's keyboard state and layout, which the key event belongs to, into
    // buffers of the sizes the calls take; the flag leaves the state unchanged.
    let count = unsafe {
        if GetKeyboardState(&mut keys).is_err() {
            return false;
        }
        let layout = GetKeyboardLayout(0);
        let scan = MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC, Some(layout));
        ToUnicodeEx(
            vk,
            scan,
            &keys,
            &mut typed,
            KEEP_KEYBOARD_STATE,
            Some(layout),
        )
    };
    match usize::try_from(count) {
        // Negative: a dead key, which types with the next.
        Err(_) => true,
        Ok(n) => typed
            .iter()
            .take(n)
            .any(|&unit| unit >= 0x20 && unit != 0x7F),
    }
}

/// The modifiers held now, as the key being reported was pressed.
fn held() -> Modifiers {
    let down = |key: VIRTUAL_KEY| {
        // SAFETY: reads the calling thread's keyboard state, which the key event belongs to.
        let state = unsafe { GetKeyState(i32::from(key.0)) };
        state < 0
    };
    Modifiers {
        ctrl: down(VK_CONTROL),
        shift: down(VK_SHIFT),
        alt: down(VK_MENU),
    }
}
