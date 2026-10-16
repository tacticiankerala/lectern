//! Windows integrations: the process start time, the boot mutex that tells a second launch apart,
//! title-bar colours through DWM, keeping a reopened window behind another, bringing a window
//! forward for an agent, DirectWrite font families, the WSL distributions and reading the current
//! user's registry.
//! Handing files to the shell or an editor is in `shell.rs`.
//!
//! Lectern targets Windows only, so this module is compiled unconditionally.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use lectern_core::agent::show::FocusStep;
use lectern_core::ipc::AgentFocus;

use windows::core::{w, BOOL, HSTRING, PCWSTR, PWSTR};
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
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, CreateMutexW, GetCurrentProcess, GetCurrentThreadId, GetProcessTimes,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, MOUSEEVENTF_MOVE, MOUSEINPUT, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, FlashWindowEx, GetForegroundWindow, GetGUIThreadInfo,
    GetWindowThreadProcessId, IsIconic, SetForegroundWindow, SetWindowPos, ShowWindow, FLASHWINFO,
    FLASHW_ALL, FLASHW_TIMERNOFG, GUITHREADINFO, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SW_RESTORE, SW_SHOWNOACTIVATE,
};

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
    default_distro_under(LXSS_KEY)
}

/// Every installed WSL distribution's name, from the `DistributionName` of each subkey of
/// `HKCU\…\Lxss`, sorted case-insensitively. Empty when WSL isn't installed.
pub fn installed_wsl_distros() -> Vec<String> {
    distros_under(LXSS_KEY)
}

/// `wsl_default_distro` of the registration key `lxss` under `HKEY_CURRENT_USER`.
fn default_distro_under(lxss: &str) -> Option<String> {
    let guid = registry_string(HKEY_CURRENT_USER, lxss, "DefaultDistribution")?;
    distro_name(lxss, &guid)
}

/// `installed_wsl_distros` of the registration key `lxss` under `HKEY_CURRENT_USER`.
fn distros_under(lxss: &str) -> Vec<String> {
    let mut names: Vec<String> = subkey_names(lxss)
        .iter()
        .filter_map(|guid| distro_name(lxss, guid))
        .collect();
    names.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    names.dedup();
    names
}

/// The name of the distribution registered under `lxss\<guid>`; `None` for a subkey without one.
fn distro_name(lxss: &str, guid: &str) -> Option<String> {
    registry_string(
        HKEY_CURRENT_USER,
        &format!(r"{lxss}\{guid}"),
        "DistributionName",
    )
    .filter(|name| !name.is_empty())
}

/// The names of the subkeys of `HKEY_CURRENT_USER\<subkey>`; none when it can't be opened.
fn subkey_names(subkey: &str) -> Vec<String> {
    let mut key = HKEY::default();
    // SAFETY: the string outlives the call and `key` is a live local.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    // A key's name is at most 255 characters.
    let mut buf = [0u16; 256];
    for index in 0u32.. {
        let mut len = buf.len() as u32;
        // SAFETY: `key` is open, and `len` is the size of `buf`, which the call fills.
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(buf.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        // `ERROR_NO_MORE_ITEMS` once every subkey is listed.
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    // SAFETY: `key` was opened above and is closed once.
    let _ = unsafe { RegCloseKey(key) };
    names
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

/// Puts `window` just behind `front`, moving and resizing neither. If showing `window` made it
/// the active window, `front` is activated again first, which takes the foreground only if
/// Lectern has it. Runs on the thread that owns both windows.
pub fn put_behind(window: HWND, front: HWND) -> Result<(), String> {
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..GUITHREADINFO::default()
    };
    // SAFETY: `info` is a live GUITHREADINFO with its size set; both handles are windows of this
    // thread, alive while it runs this.
    unsafe {
        let active = GetGUIThreadInfo(GetCurrentThreadId(), &mut info).map(|()| info.hwndActive);
        if active == Ok(window) {
            SetWindowPos(front, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
                .map_err(|e| format!("SetWindowPos (activate): {e}"))?;
        }
        SetWindowPos(
            window,
            Some(front),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
    }
    .map_err(|e| format!("SetWindowPos: {e}"))
}

/// What bringing a window forward for an agent (`bring_forward`) does to it, a Win32 call or two
/// at a time: behind a trait, so the ladder's decisions are tested without a desktop.
pub trait Desktop {
    /// It is minimised.
    fn minimized(&self) -> bool;
    /// Restores it from minimised, activating it (`SW_RESTORE`) or not (`SW_SHOWNOACTIVATE`).
    fn restore(&self, activate: bool);
    /// It is the foreground window.
    fn in_front(&self) -> bool;
    /// `SetForegroundWindow`, which Windows grants only to a process it lets take the foreground.
    fn ask_foreground(&self);
    /// Input from this process, which Windows counts towards letting it take the foreground: a
    /// mouse move by nothing (`SendInput`), else a tap of Alt.
    fn nudge(&self);
    /// With this thread's input attached to the foreground window's thread's, `SetForegroundWindow`
    /// and `BringWindowToTop`; then detached.
    fn attach_and_raise(&self);
    /// Topmost, then not: on top of the other windows, without taking the focus.
    fn raise(&self);
    /// Flashes its taskbar button until it is focused.
    fn flash(&self);
}

/// Brings `window` forward for an agent as `mode`, the `agentFocus` setting, has it, and says how
/// far it came.
///
/// - **`front`:** a ladder, checking after each step whether the window is in front. It is
///   restored if minimised; then asked for the foreground; then asked again after a nudge of
///   input; then asked with this thread's input attached to the foreground thread's. When no step
///   works, it settles for `raise` and a taskbar flash, and answers `raised`.
/// - **`raise`:** restored without activation if minimised, and raised without the focus.
/// - **`none`:** only a taskbar flash; the window stays where it is.
///
/// Runs on the thread that owns the window.
pub fn bring_forward(window: &impl Desktop, mode: AgentFocus) -> FocusStep {
    match mode {
        AgentFocus::Front => {
            if window.minimized() {
                window.restore(true);
            }
            if window.in_front() {
                return FocusStep::Foreground;
            }
            let ladder: [&dyn Fn(); 3] = [
                &|| window.ask_foreground(),
                &|| {
                    window.nudge();
                    window.ask_foreground();
                },
                &|| window.attach_and_raise(),
            ];
            for step in ladder {
                step();
                if window.in_front() {
                    return FocusStep::Foreground;
                }
            }
            window.raise();
            window.flash();
            FocusStep::Raised
        }
        AgentFocus::Raise => {
            if window.minimized() {
                window.restore(false);
            }
            window.raise();
            FocusStep::Raised
        }
        AgentFocus::None => {
            window.flash();
            FocusStep::Flashed
        }
    }
}

/// A top-level window of the calling thread, as `bring_forward` works it.
pub struct Win32Window(pub HWND);

impl Desktop for Win32Window {
    fn minimized(&self) -> bool {
        // SAFETY: a plain query of a window handle; a stale one answers false.
        unsafe { IsIconic(self.0) }.as_bool()
    }

    fn restore(&self, activate: bool) {
        let show = if activate {
            SW_RESTORE
        } else {
            SW_SHOWNOACTIVATE
        };
        // SAFETY: a plain call on a window handle; it returns whether it was visible before.
        let _ = unsafe { ShowWindow(self.0, show) };
    }

    fn in_front(&self) -> bool {
        // SAFETY: a plain query.
        let front = unsafe { GetForegroundWindow() };
        front == self.0
    }

    fn ask_foreground(&self) {
        // SAFETY: a plain call on a window handle; refused, it flashes the button instead.
        let _ = unsafe { SetForegroundWindow(self.0) };
    }

    fn nudge(&self) {
        let size = size_of::<INPUT>() as i32;
        let mouse = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: the inputs are live locals, and `size` is an INPUT's.
        if unsafe { SendInput(&[mouse], size) } == 1 {
            return;
        }
        let alt = |flags: KEYBD_EVENT_FLAGS| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_MENU,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: as above.
        let _ = unsafe { SendInput(&[alt(KEYBD_EVENT_FLAGS(0)), alt(KEYEVENTF_KEYUP)], size) };
    }

    fn attach_and_raise(&self) {
        // SAFETY: plain calls on window handles and thread ids; the input is detached again only
        // when it was attached, before this returns.
        unsafe {
            let theirs = GetWindowThreadProcessId(GetForegroundWindow(), None);
            let ours = GetCurrentThreadId();
            let attached =
                theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();
            let _ = SetForegroundWindow(self.0);
            let _ = BringWindowToTop(self.0);
            if attached {
                let _ = AttachThreadInput(ours, theirs, false);
            }
        }
    }

    fn raise(&self) {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        // SAFETY: plain calls on a window handle, moving and sizing nothing.
        unsafe {
            let _ = SetWindowPos(self.0, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
            let _ = SetWindowPos(self.0, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
        }
    }

    fn flash(&self) {
        let info = FLASHWINFO {
            cbSize: size_of::<FLASHWINFO>() as u32,
            hwnd: self.0,
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        // SAFETY: `info` is a live FLASHWINFO with its size set.
        let _ = unsafe { FlashWindowEx(&info) };
    }
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
    use windows::Win32::System::Registry::{
        RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };
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

    /// A key of its own under `HKEY_CURRENT_USER\Software` for a test, deleted with everything
    /// under it when dropped.
    struct TestKey(String);

    impl TestKey {
        fn new(name: &str) -> Self {
            Self(format!(
                r"Software\Lectern-tests-{name}-{}",
                std::process::id()
            ))
        }

        /// Sets the `REG_SZ` value `value` of the key's `subkey` (the key itself when empty).
        fn set(&self, subkey: &str, value: &str, data: &str) {
            let path = if subkey.is_empty() {
                self.0.clone()
            } else {
                format!(r"{}\{subkey}", self.0)
            };
            let mut key = HKEY::default();
            let bytes: Vec<u8> = data
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect();
            // SAFETY: the strings outlive the calls, `key` is a live local, and the key is closed
            // once its value is set.
            unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    &HSTRING::from(path),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_WRITE,
                    None,
                    &mut key,
                    None,
                )
                .ok()
                .unwrap();
                RegSetValueExW(key, &HSTRING::from(value), None, REG_SZ, Some(&bytes))
                    .ok()
                    .unwrap();
                let _ = RegCloseKey(key);
            }
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            // SAFETY: a plain call with a string that outlives it.
            let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(self.0.as_str())) };
        }
    }

    /// Each subkey of `Lxss` registers a distribution under its `DistributionName`; the default
    /// is the one whose subkey `DefaultDistribution` names.
    #[test]
    fn wsl_distros_are_the_lxss_subkeys_names() {
        let lxss = TestKey::new("lxss");
        lxss.set("{b0}", "DistributionName", "Ubuntu");
        lxss.set("{a1}", "DistributionName", "debian-dev");
        // A subkey with no name is no distribution.
        lxss.set("{c2}", "BasePath", r"C:\Users\me\wsl");
        lxss.set("", "DefaultDistribution", "{b0}");
        assert_eq!(distros_under(&lxss.0), ["debian-dev", "Ubuntu"]);
        assert_eq!(default_distro_under(&lxss.0).as_deref(), Some("Ubuntu"));
        drop(lxss);
        // Without WSL, there are none.
        let missing = TestKey::new("no-lxss");
        assert!(distros_under(&missing.0).is_empty());
        assert_eq!(default_distro_under(&missing.0), None);
    }

    /// A window on a pretend desktop: it records each call, and comes in front once the call
    /// `works` is made (never, for none; at once, for "already").
    struct Pretend {
        minimized: bool,
        works: Option<&'static str>,
        calls: std::cell::RefCell<Vec<&'static str>>,
        front: std::cell::Cell<bool>,
    }

    impl Pretend {
        fn new(minimized: bool, works: Option<&'static str>) -> Self {
            Self {
                minimized,
                works,
                calls: std::cell::RefCell::new(Vec::new()),
                front: std::cell::Cell::new(works == Some("already")),
            }
        }

        fn call(&self, name: &'static str) {
            self.calls.borrow_mut().push(name);
            if self.works == Some(name) {
                self.front.set(true);
            }
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    impl Desktop for Pretend {
        fn minimized(&self) -> bool {
            self.minimized
        }
        fn restore(&self, activate: bool) {
            self.call(if activate {
                "restore"
            } else {
                "restore quietly"
            });
        }
        fn in_front(&self) -> bool {
            self.front.get()
        }
        fn ask_foreground(&self) {
            self.call("ask");
        }
        fn nudge(&self) {
            self.call("nudge");
        }
        fn attach_and_raise(&self) {
            self.call("attach");
        }
        fn raise(&self) {
            self.call("raise");
        }
        fn flash(&self) {
            self.call("flash");
        }
    }

    /// Front climbs the ladder only as far as it must, checking after each step.
    #[test]
    fn front_climbs_the_ladder_until_the_window_is_in_front() {
        let table: [(bool, Option<&'static str>, &[&str]); 6] = [
            (false, Some("already"), &[]),
            (true, Some("restore"), &["restore"]),
            (false, Some("ask"), &["ask"]),
            // The ask after the nudge is the one that works.
            (false, Some("nudge"), &["ask", "nudge", "ask"]),
            (false, Some("attach"), &["ask", "nudge", "ask", "attach"]),
            (
                true,
                Some("attach"),
                &["restore", "ask", "nudge", "ask", "attach"],
            ),
        ];
        for (minimized, works, calls) in table {
            let window = Pretend::new(minimized, works);
            assert_eq!(
                bring_forward(&window, AgentFocus::Front),
                FocusStep::Foreground,
                "{works:?}"
            );
            assert_eq!(window.calls(), calls, "{works:?}");
        }
    }

    /// When no step brings it in front, it settles for coming to the top without the focus,
    /// flashing as well.
    #[test]
    fn front_settles_for_raised_and_a_flash_when_every_step_fails() {
        let window = Pretend::new(false, None);
        assert_eq!(bring_forward(&window, AgentFocus::Front), FocusStep::Raised);
        assert_eq!(
            window.calls(),
            ["ask", "nudge", "ask", "attach", "raise", "flash"]
        );
    }

    #[test]
    fn raise_comes_to_the_top_without_activating_and_none_only_flashes() {
        let window = Pretend::new(true, Some("restore"));
        assert_eq!(bring_forward(&window, AgentFocus::Raise), FocusStep::Raised);
        assert_eq!(window.calls(), ["restore quietly", "raise"]);
        let window = Pretend::new(false, None);
        assert_eq!(bring_forward(&window, AgentFocus::Raise), FocusStep::Raised);
        assert_eq!(window.calls(), ["raise"]);
        // Left alone: even a minimised window only flashes.
        let window = Pretend::new(true, None);
        assert_eq!(bring_forward(&window, AgentFocus::None), FocusStep::Flashed);
        assert_eq!(window.calls(), ["flash"]);
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
