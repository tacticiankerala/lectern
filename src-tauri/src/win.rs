//! Windows integrations: the process start time, the boot mutex that tells a second launch apart,
//! title-bar colours through DWM, DirectWrite font families, the default WSL distribution and
//! reading the current user's registry.
//! Handing files to the shell or an editor is in `shell.rs`.
//!
//! Lectern targets Windows only, so this module is compiled unconditionally.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows::core::{w, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, E_FAIL, E_INVALIDARG, FILETIME, HWND,
};
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteLocalizedStrings, DWRITE_FACTORY_TYPE_SHARED,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, GetProcessTimes};

/// 100 ns intervals from 1601-01-01, the `FILETIME` epoch, to the Unix epoch.
const FILETIME_UNIX_OFFSET: u64 = 116_444_736_000_000_000;

/// The WSL registration key under `HKEY_CURRENT_USER`.
const LXSS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";

/// `LOCALE_NAME_MAX_LENGTH`, including the terminating NUL.
const LOCALE_NAME_LEN: usize = 85;

/// When this process was created, in Unix milliseconds, from `GetProcessTimes`.
pub fn process_start_unix_ms() -> Option<f64> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the pseudo-handle of the current process is always valid, and every out-pointer
    // refers to a live local.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    Some(ticks.checked_sub(FILETIME_UNIX_OFFSET)? as f64 / 10_000.0)
}

/// The default WSL distribution's name, from `HKCU\…\Lxss`: `DefaultDistribution` holds a GUID
/// whose subkey holds `DistributionName`. `None` when WSL isn't installed.
pub fn wsl_default_distro() -> Option<String> {
    let guid = registry_string(HKEY_CURRENT_USER, LXSS_KEY, "DefaultDistribution")?;
    registry_string(
        HKEY_CURRENT_USER,
        &format!(r"{LXSS_KEY}\{guid}"),
        "DistributionName",
    )
    .filter(|name| !name.is_empty())
}

/// A `REG_SZ` value under `HKEY_CURRENT_USER`, or `None` when the key or value is missing or of
/// another type.
pub fn current_user_string(subkey: &str, value: &str) -> Option<String> {
    registry_string(HKEY_CURRENT_USER, subkey, value)
}

/// A `REG_SZ` value, or `None` when the key or value is missing or of another type.
fn registry_string(key: HKEY, subkey: &str, value: &str) -> Option<String> {
    let subkey = HSTRING::from(subkey);
    let value = HSTRING::from(value);
    let mut bytes = 0u32;
    // SAFETY: the strings are NUL-terminated HSTRINGs that outlive the call; only the size is
    // requested.
    let status = unsafe {
        RegGetValueW(
            key,
            &subkey,
            &value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `buf` holds at least `bytes` bytes, the size the first call reported.
    let status = unsafe {
        RegGetValueW(
            key,
            &subkey,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    Some(from_wide_until_nul(&buf))
}

fn from_wide_until_nul(wide: &[u16]) -> String {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

/// `#rrggbb` or `#rgb` as a Win32 `COLORREF`, `0x00BBGGRR`.
pub fn parse_colorref(hex: &str) -> Option<u32> {
    let digits = hex.strip_prefix('#')?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |s: &str| u8::from_str_radix(s, 16).ok();
    let (r, g, b) = match digits.len() {
        6 => (
            channel(&digits[0..2])?,
            channel(&digits[2..4])?,
            channel(&digits[4..6])?,
        ),
        3 => (
            channel(&digits[0..1])? * 17,
            channel(&digits[1..2])? * 17,
            channel(&digits[2..3])? * 17,
        ),
        _ => return None,
    };
    Some(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
}

/// The red, green and blue channels of a `COLORREF`.
pub fn colorref_rgb(color: u32) -> (u8, u8, u8) {
    let [r, g, b, _] = color.to_le_bytes();
    (r, g, b)
}

/// Themes the window's title bar: dark mode for the caption buttons, and the caption and text
/// colours. Windows 10 has no caption or text colour and answers `E_INVALIDARG`; that counts as
/// success (logged once), leaving dark mode alone to apply. Every attribute is attempted; the
/// first real failure is returned.
pub fn set_title_bar_colors(hwnd: HWND, caption: u32, text: u32, dark: bool) -> Result<(), String> {
    static UNSUPPORTED_LOGGED: AtomicBool = AtomicBool::new(false);
    let dark = BOOL::from(dark);
    let attributes = [
        (DWMWA_USE_IMMERSIVE_DARK_MODE, (&raw const dark).cast()),
        (DWMWA_CAPTION_COLOR, (&raw const caption).cast()),
        (DWMWA_TEXT_COLOR, (&raw const text).cast()),
    ];
    let mut first_error = None;
    for (attribute, value) in attributes {
        // SAFETY: each value points at a live 4-byte BOOL or COLORREF, as the attribute expects.
        let set = unsafe { DwmSetWindowAttribute(hwnd, attribute, value, 4) };
        match set {
            Ok(()) => {}
            Err(e) if e.code() == E_INVALIDARG && attribute != DWMWA_USE_IMMERSIVE_DARK_MODE => {
                if !UNSUPPORTED_LOGGED.swap(true, Ordering::Relaxed) {
                    log::info!("this Windows can't colour the title bar (needs Windows 11)");
                }
            }
            Err(e) => {
                first_error
                    .get_or_insert_with(|| format!("DwmSetWindowAttribute({}): {e}", attribute.0));
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Whether another Lectern is already running or starting, by a named mutex this process then
/// holds until it exits. A second launch skips its boot work and lets the single-instance plugin
/// hand its arguments over. `false` when the mutex can't be made, so Lectern still starts.
pub fn another_instance_is_running(identifier: &str) -> bool {
    let name = HSTRING::from(format!(r"Local\{identifier}.boot"));
    // SAFETY: the name is a NUL-terminated HSTRING; the handle is deliberately never closed, so
    // the mutex lives as long as the process.
    match unsafe { CreateMutexW(None, true, &name) } {
        Ok(_handle) => {
            // SAFETY: reads the calling thread's last error, set by CreateMutexW just above.
            let last = unsafe { GetLastError() };
            last == ERROR_ALREADY_EXISTS
        }
        Err(e) => {
            log::warn!("couldn't create the boot mutex: {e}");
            false
        }
    }
}

/// The installed font families, in the user's language (else US English), sorted
/// case-insensitively without duplicates. Enumerated once, then cached.
pub fn system_font_families() -> Vec<String> {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES
        .get_or_init(|| {
            font_families().unwrap_or_else(|e| {
                log::warn!("couldn't list the system fonts: {e}");
                Vec::new()
            })
        })
        .clone()
}

fn font_families() -> windows::core::Result<Vec<String>> {
    // SAFETY: plain COM calls on interfaces DirectWrite hands back; every out-pointer refers to a
    // live local.
    unsafe {
        let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let mut collection = None;
        factory.GetSystemFontCollection(&mut collection, false)?;
        let collection = collection.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
        let locale = user_locale();
        let mut names = Vec::new();
        for i in 0..collection.GetFontFamilyCount() {
            let names_of_family = collection.GetFontFamily(i)?.GetFamilyNames()?;
            if let Some(name) = localized(&names_of_family, &locale) {
                names.push(name);
            }
        }
        names.sort_by(|a, b| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        });
        names.dedup();
        Ok(names)
    }
}

/// The user's locale name, NUL-terminated, or just a NUL when it can't be read.
fn user_locale() -> Vec<u16> {
    let mut buf = vec![0u16; LOCALE_NAME_LEN];
    // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH wide, as the call requires.
    let len = unsafe { GetUserDefaultLocaleName(&mut buf) };
    buf.truncate(usize::try_from(len).unwrap_or(0).max(1));
    if let Some(last) = buf.last_mut() {
        *last = 0;
    }
    buf
}

/// The string for `locale` (NUL-terminated), else for US English, else the first one.
///
/// # Safety
/// `strings` must be a live DirectWrite string list.
unsafe fn localized(strings: &IDWriteLocalizedStrings, locale: &[u16]) -> Option<String> {
    let find = |name: PCWSTR| {
        let mut index = 0u32;
        let mut exists = BOOL::default();
        // SAFETY: `name` is NUL-terminated and the out-pointers refer to live locals.
        unsafe { strings.FindLocaleName(name, &mut index, &mut exists) }.ok()?;
        exists.as_bool().then_some(index)
    };
    let index = find(PCWSTR(locale.as_ptr()))
        .or_else(|| find(w!("en-us")))
        .unwrap_or(0);
    // SAFETY: `index` is below the count, checked first; the buffer has room for the NUL.
    unsafe {
        if index >= strings.GetCount() {
            return None;
        }
        let len = strings.GetStringLength(index).ok()? as usize;
        let mut buf = vec![0u16; len + 1];
        strings.GetString(index, &mut buf).ok()?;
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn colorref_is_0x00bbggrr() {
        assert_eq!(parse_colorref("#1e1f22"), Some(0x0022_1f1e));
        assert_eq!(parse_colorref("#F8F5EE"), Some(0x00ee_f5f8));
        assert_eq!(parse_colorref("#000000"), Some(0));
        assert_eq!(parse_colorref("#fff"), Some(0x00ff_ffff));
        assert_eq!(parse_colorref("#1e2"), Some(0x0022_ee11));
    }

    #[test]
    fn colorref_rejects_anything_but_hex_colours() {
        for bad in [
            "1e1f22", "#1e1f2", "#1e1f222", "#gggggg", "#+1+2+3", "", "#", "#ééé",
        ] {
            assert_eq!(parse_colorref(bad), None, "{bad}");
        }
    }

    #[test]
    fn colorref_round_trips_to_channels() {
        assert_eq!(colorref_rgb(0x0022_1f1e), (0x1e, 0x1f, 0x22));
    }

    #[test]
    fn process_start_is_in_the_recent_past() {
        let started = process_start_unix_ms().expect("GetProcessTimes works");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            * 1000.0;
        assert!(
            started <= now && now - started < 600_000.0,
            "{started} vs {now}"
        );
    }

    #[test]
    fn system_fonts_include_segoe_ui_sorted_and_unique() {
        let fonts = system_font_families();
        assert!(fonts.iter().any(|f| f == "Segoe UI"), "{fonts:?}");
        let mut sorted = fonts.clone();
        sorted.sort_by(|a, b| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        });
        sorted.dedup();
        assert_eq!(fonts, sorted);
    }
}
