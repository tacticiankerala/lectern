//! The named pipe agents reach Lectern through (`agent.rs` serves it, `mcp.rs` connects to it):
//! one per user, session and app identifier (`agent_pipe`), which only that user can open, from
//! this machine only.
//!
//! - **The server** creates its first instance with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so another
//!   process that holds the name already (a second server, or a squatter) is found out, and every
//!   instance with a DACL granting the current user alone and `PIPE_REJECT_REMOTE_CLIENTS`.
//!   There is always one instance waiting, so several clients connect at once, and a failure
//!   never lets the name go: the server tries again, with the failed instance held until a fresh
//!   one takes its place.
//! - **Both ends** use overlapped handles: a read waiting for the next line never holds up a write
//!   from another thread, as a synchronous handle's would.
//! - **A client** asks for identification only, so a server it reaches can't act as its user.

use std::io::{self, ErrorKind, Read};
use std::sync::Arc;
use std::time::Duration;

use lectern_core::agent::pipe_name;
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_BROKEN_PIPE, ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING,
    ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ,
    GENERIC_WRITE, HANDLE, HLOCAL, WAIT_EVENT, WAIT_FAILED, WAIT_OBJECT_0, WIN32_ERROR,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenSessionId, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcess, OpenProcessToken, WaitForMultipleObjects,
    WaitForSingleObject, INFINITE, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

/// Each instance's buffers, in bytes.
const BUFFER: u32 = 64 * 1024;
/// How long a client waits for an instance when every one is busy.
const BUSY_WAIT: Duration = Duration::from_secs(2);
/// How many times a client finds every instance busy before it gives up.
const BUSY_TRIES: u32 = 3;
/// The pause before a failed accept is tried again, the first time.
const RETRY_FIRST: Duration = Duration::from_millis(100);
/// The longest pause before a failed accept is tried again.
const RETRY_MOST: Duration = Duration::from_secs(5);

/// The current user's SID, as text (`S-1-5-21-…`).
pub fn user_sid() -> io::Result<String> {
    // SAFETY: the pseudo-handle of the current process is always valid.
    process_user_sid(unsafe { GetCurrentProcess() })
}

/// The Windows session this process runs in: the same user logged on twice has two.
pub fn session_id() -> io::Result<u32> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo-handle of the current process is always valid; `token` is a live local.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(os_error)?;
    let token = Owned(token);
    let mut session = 0u32;
    let mut len = 0u32;
    // SAFETY: `session` is a live u32, the size TokenSessionId writes.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenSessionId,
            Some((&raw mut session).cast()),
            size_of::<u32>() as u32,
            &mut len,
        )
    }
    .map_err(os_error)?;
    Ok(session)
}

/// Lectern's agent pipe for the app `identifier`, as its server and `lectern.exe mcp` both name it
/// (`pipe_name`): the current user's, in this session; with the user's SID, which the client checks
/// the server against.
pub fn agent_pipe(identifier: &str) -> io::Result<(String, String)> {
    let sid = user_sid()?;
    let session = session_id()?;
    Ok((pipe_name(identifier, &sid, session), sid))
}

/// The SID, as text, of the user the process `process` (a handle open for querying) runs as.
fn process_user_sid(process: HANDLE) -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: `process` is a live process handle; `token` is a live local.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.map_err(os_error)?;
    let token = Owned(token);
    let mut len = 0u32;
    // SAFETY: asks only for the size the user's entry needs.
    match unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut len) } {
        Err(e) if e.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {}
        Err(e) => return Err(os_error(e)),
        Ok(()) => return Err(io::Error::other("the token's user has no size")),
    }
    // u64s, so the TOKEN_USER read from it is aligned.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes, the size the first call asked for.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
    }
    .map_err(os_error)?;
    // SAFETY: the call above wrote a TOKEN_USER at the start of `buf`, whose SID points into it.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = PWSTR::null();
    // SAFETY: `sid` is valid while `buf` lives; the string is freed with LocalFree below.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(os_error)?;
    // SAFETY: `text` is the NUL-terminated string the call allocated.
    let sid = unsafe { text.to_string() }.map_err(|e| io::Error::other(e.to_string()));
    // SAFETY: frees the string ConvertSidToStringSidW allocated, once.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    sid
}

/// A handle closed when dropped.
struct Owned(HANDLE);

// SAFETY: a kernel handle may be used and closed from any thread.
unsafe impl Send for Owned {}
// SAFETY: as above; the handles here are events and pipes, safe to share.
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle is owned here and closed once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// A manual-reset event, not signalled.
fn event() -> io::Result<Owned> {
    // SAFETY: an unnamed event with default security.
    unsafe { CreateEventW(None, true, false, None) }
        .map(Owned)
        .map_err(os_error)
}

/// One end of a connection: a pipe handle opened for overlapped I/O, read and written with each
/// call waiting for its own operation, so one thread may read while another writes.
pub struct Pipe(Owned);

impl Pipe {
    /// Reads what has arrived, at most `buf.len()` bytes, waiting for some; 0 once the other end
    /// has closed.
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_waiting(buf, None)
    }

    /// Reads as `read` does, but waits at most `wait` for something to arrive: `TimedOut` when
    /// nothing has, the connection still open.
    pub fn read_within(&self, buf: &mut [u8], wait: Duration) -> io::Result<usize> {
        self.read_waiting(buf, Some(wait))
    }

    /// `read`, waiting at most `within` when given.
    fn read_waiting(&self, buf: &mut [u8], within: Option<Duration>) -> io::Result<usize> {
        let len = buf.len().min(u32::MAX as usize);
        let read = self.overlapped(
            |ov| {
                // SAFETY: `buf` and `ov` outlive the operation, which `overlapped` waits for.
                unsafe { ReadFile(self.0 .0, Some(&mut buf[..len]), None, Some(ov)) }
            },
            within,
        );
        match read {
            Ok(n) => Ok(n as usize),
            Err(e) if is(&e, ERROR_BROKEN_PIPE) || is(&e, ERROR_NO_DATA) => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Writes all of `bytes`.
    pub fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let len = bytes.len().min(u32::MAX as usize);
            let chunk = &bytes[..len];
            let written = self.overlapped(
                |ov| {
                    // SAFETY: `chunk` and `ov` outlive the operation, which `overlapped` waits for.
                    unsafe { WriteFile(self.0 .0, Some(chunk), None, Some(ov)) }
                },
                None,
            )?;
            if written == 0 {
                return Err(ErrorKind::WriteZero.into());
            }
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }

    /// Cancels every read and write under way on this end, which then fail.
    pub fn cancel(&self) {
        // SAFETY: cancels whatever I/O is pending on a handle this owns.
        let _ = unsafe { CancelIoEx(self.0 .0, None) };
    }

    /// Starts an overlapped operation with `start` and waits for it to finish, whether it
    /// finished at once or is pending: how many bytes it moved. Given `within`, it waits that long
    /// at most: an operation not done by then is cancelled, `TimedOut`, unless it finished as it
    /// was cancelled.
    fn overlapped(
        &self,
        start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
        within: Option<Duration>,
    ) -> io::Result<u32> {
        let done = event()?;
        let mut ov = OVERLAPPED {
            hEvent: done.0,
            ..OVERLAPPED::default()
        };
        match start(&mut ov) {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(e) => return Err(os_error(e)),
        }
        let mut late = None;
        if let Some(within) = within {
            let millis = u32::try_from(within.as_millis())
                .unwrap_or(u32::MAX)
                .min(INFINITE - 1);
            // SAFETY: the operation's own event, which this owns.
            let woke = unsafe { WaitForSingleObject(done.0, millis) };
            if woke != WAIT_OBJECT_0 {
                late = Some(if woke == WAIT_FAILED {
                    io::Error::last_os_error()
                } else {
                    io::Error::new(ErrorKind::TimedOut, "nothing arrived in time")
                });
                // SAFETY: cancels this operation alone, which is settled below before `ov` goes.
                let _ = unsafe { CancelIoEx(self.0 .0, Some(&raw const ov)) };
            }
        }
        let mut moved = 0u32;
        // SAFETY: `ov` is the operation's own, alive until it is done, which this waits for.
        match unsafe { GetOverlappedResult(self.0 .0, &ov, &mut moved, true) } {
            Ok(()) => Ok(moved),
            Err(e) => match late {
                Some(late) if e.code() == ERROR_OPERATION_ABORTED.to_hresult() => Err(late),
                _ => Err(os_error(e)),
            },
        }
    }
}

impl Pipe {
    /// The SID, as text, of the user the process serving this pipe runs as (a client end): what
    /// tells Lectern's own pipe from one another user made first under its name.
    pub fn server_user(&self) -> io::Result<String> {
        let mut pid = 0u32;
        // SAFETY: a client end of a pipe this owns; `pid` is a live local.
        unsafe { GetNamedPipeServerProcessId(self.0 .0, &mut pid) }.map_err(os_error)?;
        // SAFETY: opens the server's process for querying alone; closed when `process` drops.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            .map(Owned)
            .map_err(os_error)?;
        process_user_sid(process.0)
    }
}

impl Read for &Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Pipe::read(self, buf)
    }
}

/// `e` as the `io::Error` of its Win32 error, so its kind (`NotFound`, `PermissionDenied`, …) is
/// known: windows' own conversion keeps the `HRESULT`, which no kind matches.
fn os_error(e: windows::core::Error) -> io::Error {
    match WIN32_ERROR::from_error(&e) {
        Some(code) => io::Error::from_raw_os_error(code.0 as i32),
        None => io::Error::other(e),
    }
}

/// Whether `e` is the Win32 error `code`.
fn is(e: &io::Error, code: WIN32_ERROR) -> bool {
    e.raw_os_error() == Some(code.0 as i32)
}

/// Opens the pipe `name` as a client, waiting a moment when every instance is busy, a few times
/// at most. `NotFound` when no server has it; `PermissionDenied` when its DACL keeps this process
/// out; `TimedOut` when every instance stays busy.
pub fn connect(name: &str) -> io::Result<Pipe> {
    let wide = HSTRING::from(name);
    let flags: FILE_FLAGS_AND_ATTRIBUTES =
        FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION;
    for _ in 0..BUSY_TRIES {
        // SAFETY: `wide` is a NUL-terminated name; no security attributes or template.
        let opened = unsafe {
            CreateFileW(
                &wide,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                flags,
                None,
            )
        };
        match opened {
            Ok(handle) => return Ok(Pipe(Owned(handle))),
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                // SAFETY: as above.
                let freed = unsafe { WaitNamedPipeW(&wide, BUSY_WAIT.as_millis() as u32) };
                if !freed.as_bool() {
                    break;
                }
            }
            Err(e) => return Err(os_error(e)),
        }
    }
    Err(io::Error::new(
        ErrorKind::TimedOut,
        "every instance is busy",
    ))
}

/// The server end: the instance a client will reach next, and what makes more.
pub struct Listener {
    name: HSTRING,
    security: Security,
    /// The instance waiting for the next client.
    next: Option<Pipe>,
    stop: Arc<Owned>,
    #[cfg(test)]
    faults: Arc<Faults>,
}

/// Failures a test makes a [`Listener`] meet: the next `instances` instances it makes, and the
/// next `connects` waits for a client, fail; so do the next `waits` waits on a pending connect, as
/// `WaitForMultipleObjects` fails (`WAIT_FAILED`).
#[cfg(test)]
#[derive(Default)]
pub struct Faults {
    pub instances: std::sync::atomic::AtomicU32,
    pub connects: std::sync::atomic::AtomicU32,
    pub waits: std::sync::atomic::AtomicU32,
}

#[cfg(test)]
impl Faults {
    /// Whether to fail now, by `count`: one fewer failure is then left.
    fn now(count: &std::sync::atomic::AtomicU32) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        count
            .try_update(SeqCst, SeqCst, |n| n.checked_sub(1))
            .is_ok()
    }
}

/// Stops a [`Listener`]'s `accept`, now or when it next waits: for tests, as Lectern's own
/// listens until it exits.
#[cfg(test)]
#[derive(Clone)]
pub struct Stopper(Arc<Owned>);

#[cfg(test)]
impl Stopper {
    pub fn stop(&self) {
        // SAFETY: signals an event this owns.
        let _ = unsafe { windows::Win32::System::Threading::SetEvent(self.0 .0) };
    }
}

/// Creates the pipe `name`'s first instance, which fails if any process has the name already.
pub fn listen(name: &str) -> io::Result<Listener> {
    let security = Security::current_user_only()?;
    let name = HSTRING::from(name);
    let first = instance(&name, &security, true)?;
    Ok(Listener {
        name,
        security,
        next: Some(first),
        stop: Arc::new(event()?),
        #[cfg(test)]
        faults: Arc::default(),
    })
}

impl Listener {
    /// What stops `accept`.
    #[cfg(test)]
    pub fn stopper(&self) -> Stopper {
        Stopper(Arc::clone(&self.stop))
    }

    /// The failures a test makes this meet.
    #[cfg(test)]
    pub fn faults(&self) -> Arc<Faults> {
        Arc::clone(&self.faults)
    }

    /// A new instance, not the first.
    fn fresh(&self) -> io::Result<Pipe> {
        #[cfg(test)]
        if Faults::now(&self.faults.instances) {
            return Err(io::Error::other("an instance that fails, for a test"));
        }
        instance(&self.name, &self.security, false)
    }

    /// Waits for the next client, and makes another instance for the one after it. `None` once
    /// stopped, and nothing else ends it: a failure is logged and tried again after a pause,
    /// doubling from [`RETRY_FIRST`] up to [`RETRY_MOST`], with the name held all along, so `mcp`
    /// never finds it gone and starts a second Lectern.
    pub fn accept(&mut self) -> Option<Pipe> {
        let mut pause = RETRY_FIRST;
        loop {
            let pipe = match self.next.take() {
                Some(pipe) => pipe,
                None => match self.fresh() {
                    Ok(pipe) => pipe,
                    Err(e) => {
                        log::warn!("the agent pipe couldn't make an instance: {e}; trying again");
                        self.pause(&mut pause)?;
                        continue;
                    }
                },
            };
            let failed = match self.connect_client(&pipe) {
                Ok(()) => {
                    // The client is served even when the instance after it can't be made yet:
                    // the next call makes it.
                    match self.fresh() {
                        Ok(next) => self.next = Some(next),
                        Err(e) => log::warn!("the agent pipe couldn't make its next instance: {e}"),
                    }
                    return Some(pipe);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => return None,
                // A client came and left before it was accepted: the instance is made ready for
                // the next one, and kept.
                Err(e) if is(&e, ERROR_NO_DATA) => {
                    // SAFETY: a server end this owns, whose client has gone.
                    match unsafe { DisconnectNamedPipe(pipe.0 .0) } {
                        Ok(()) => {
                            self.next = Some(pipe);
                            continue;
                        }
                        Err(e) => os_error(e),
                    }
                }
                Err(e) => e,
            };
            log::warn!("the agent pipe failed to wait for a client: {failed}; trying again");
            // A fresh instance takes the failed one's place before it goes, so the name is held;
            // without one, the failed one is tried again.
            self.next = Some(self.fresh().unwrap_or_else(|e| {
                log::warn!("the agent pipe couldn't make an instance: {e}; keeping the failed one");
                pipe
            }));
            self.pause(&mut pause)?;
        }
    }

    /// Waits `pause`, unless stopped meanwhile (`None`), and doubles it for the next time, up to
    /// [`RETRY_MOST`].
    fn pause(&self, pause: &mut Duration) -> Option<()> {
        // SAFETY: an event this owns.
        let stopped = unsafe { WaitForSingleObject(self.stop.0, pause.as_millis() as u32) };
        if stopped == WAIT_FAILED {
            // The stop can't be waited for: the pause is still taken.
            std::thread::sleep(*pause);
        }
        *pause = (*pause * 2).min(RETRY_MOST);
        (stopped != WAIT_OBJECT_0).then_some(())
    }

    /// Waits for a client on the instance `pipe`: `Interrupted` when stopped, `ERROR_NO_DATA`
    /// when a client connected and left before this was called.
    fn connect_client(&self, pipe: &Pipe) -> io::Result<()> {
        #[cfg(test)]
        if Faults::now(&self.faults.connects) {
            return Err(io::Error::other("a connect that fails, for a test"));
        }
        let done = event()?;
        let mut ov = OVERLAPPED {
            hEvent: done.0,
            ..OVERLAPPED::default()
        };
        // SAFETY: `ov` lives until the connect is done or cancelled and settled in `wait`.
        match unsafe { ConnectNamedPipe(pipe.0 .0, Some(&mut ov)) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => Ok(()),
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => self.wait(pipe, &ov),
            Err(e) => Err(os_error(e)),
        }
    }

    /// Waits for the pending connect `ov` on `pipe`, or the stop: `Interrupted` when stopped, and
    /// the wait's own error when it fails (`WAIT_FAILED`), the connect cancelled either way.
    fn wait(&self, pipe: &Pipe, ov: &OVERLAPPED) -> io::Result<()> {
        #[cfg(test)]
        let failing = Faults::now(&self.faults.waits);
        #[cfg(not(test))]
        let failing = false;
        let woke = if failing {
            WAIT_FAILED
        } else {
            // SAFETY: both handles are events this owns.
            unsafe { WaitForMultipleObjects(&[ov.hEvent, self.stop.0], false, INFINITE) }
        };
        let mut moved = 0u32;
        if woke != WAIT_OBJECT_0 {
            // Stopped, or the wait itself failed, which is tried again as any failure is: only
            // the stop ends `accept`.
            let failed = if woke == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
                ErrorKind::Interrupted.into()
            } else if failing {
                io::Error::other("a wait for a client that fails, for a test")
            } else {
                io::Error::last_os_error()
            };
            pipe.cancel();
            // SAFETY: settles the cancelled connect, so `ov` may go.
            let _ = unsafe { GetOverlappedResult(pipe.0 .0, ov, &mut moved, true) };
            return Err(failed);
        }
        // SAFETY: the connect is done; this reads its outcome.
        unsafe { GetOverlappedResult(pipe.0 .0, ov, &mut moved, true) }.map_err(os_error)
    }
}

/// A new instance of the pipe `name`, the first when `first`.
fn instance(name: &HSTRING, security: &Security, first: bool) -> io::Result<Pipe> {
    let mut open = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0 .0,
        bInheritHandle: false.into(),
    };
    // SAFETY: the name is NUL-terminated and the attributes, with their descriptor, outlive the
    // call.
    let handle = unsafe {
        CreateNamedPipeW(
            name,
            open,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER,
            BUFFER,
            0,
            Some(&attributes),
        )
    };
    if handle.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    Ok(Pipe(Owned(handle)))
}

/// A security descriptor whose DACL grants the current user, and no one else, everything.
struct Security(PSECURITY_DESCRIPTOR);

// SAFETY: the descriptor is only read once made, and freed once, on drop.
unsafe impl Send for Security {}

impl Security {
    fn current_user_only() -> io::Result<Self> {
        let sddl = HSTRING::from(format!("D:P(A;;GA;;;{})", user_sid()?));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is NUL-terminated; the descriptor is freed on drop.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(os_error)?;
        Ok(Self(descriptor))
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: frees the descriptor the conversion allocated, once.
        unsafe { LocalFree(Some(HLOCAL(self.0 .0))) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread;
    use std::time::Instant;
    use windows::Win32::Foundation::ERROR_ACCESS_DENIED;
    use windows::Win32::Security::{
        CreateRestrictedToken, ImpersonateLoggedOnUser, RevertToSelf, DISABLE_MAX_PRIVILEGE,
        SID_AND_ATTRIBUTES, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE,
    };

    /// A pipe name no other test, and no Lectern, uses.
    fn test_name(what: &str) -> String {
        format!(
            r"\\.\pipe\lectern-test-{what}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        )
        .replace([' ', '{', '}', ':'], "")
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "timed out: {what}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The byte a client sends first, read by the server.
    fn first_byte(pipe: &Pipe) -> Option<u8> {
        let mut buf = [0u8; 1];
        (pipe.read(&mut buf).ok()? == 1).then_some(buf[0])
    }

    #[test]
    fn the_sid_is_a_user_sid() {
        let sid = user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
    }

    /// Lectern and `mcp` name the agent pipe alike: this user's, in this session.
    #[test]
    fn the_agent_pipe_is_this_users_in_this_session() {
        let (name, sid) = agent_pipe("io.github.example.lectern").unwrap();
        assert_eq!(sid, user_sid().unwrap());
        let session = session_id().unwrap();
        assert_eq!(name, pipe_name("io.github.example.lectern", &sid, session));
        assert!(name.ends_with(&format!("-s{session}")), "{name}");
    }

    #[test]
    fn a_client_reaches_the_server_and_both_read_and_write_at_once() {
        let name = test_name("echo");
        let mut listener = listen(&name).unwrap();
        let server = thread::spawn(move || {
            let pipe = listener.accept().expect("a client");
            let mut buf = [0u8; 5];
            let mut got = Vec::new();
            while got.len() < 5 {
                let n = pipe.read(&mut buf).unwrap();
                assert!(n > 0, "closed early");
                got.extend_from_slice(&buf[..n]);
            }
            pipe.write_all(b"world").unwrap();
            got
        });
        let client = Arc::new(connect(&name).unwrap());
        // A read waits on one thread while the other writes.
        let reader = {
            let client = Arc::clone(&client);
            thread::spawn(move || {
                let mut buf = [0u8; 5];
                let mut got = Vec::new();
                while got.len() < 5 {
                    let n = client.read(&mut buf).unwrap();
                    assert!(n > 0, "closed early");
                    got.extend_from_slice(&buf[..n]);
                }
                got
            })
        };
        thread::sleep(Duration::from_millis(100));
        client.write_all(b"hello").unwrap();
        assert_eq!(server.join().unwrap(), b"hello");
        assert_eq!(reader.join().unwrap(), b"world");
    }

    #[test]
    fn a_client_sees_the_server_runs_as_its_own_user() {
        let name = test_name("owner");
        let _listener = listen(&name).unwrap();
        let client = connect(&name).unwrap();
        assert_eq!(client.server_user().unwrap(), user_sid().unwrap());
    }

    #[test]
    fn a_closed_end_reads_as_the_end() {
        let name = test_name("close");
        let mut listener = listen(&name).unwrap();
        let server = thread::spawn(move || listener.accept().expect("a client"));
        let client = connect(&name).unwrap();
        let pipe = server.join().unwrap();
        drop(client);
        let mut buf = [0u8; 8];
        assert_eq!(pipe.read(&mut buf).unwrap(), 0);
    }

    /// A client that connects and leaves before the server accepts it (such as `mcp` refusing
    /// it) doesn't stop the server: the next client is accepted.
    #[test]
    fn a_client_that_leaves_before_it_is_accepted_does_not_stop_the_listener() {
        let name = test_name("left");
        let mut listener = listen(&name).unwrap();
        drop(connect(&name).unwrap());
        let stopper = listener.stopper();
        let server = thread::spawn(move || listener.accept().is_some());
        let client = connect(&name);
        if client.is_err() {
            stopper.stop();
        }
        let accepted = server.join().unwrap();
        assert!(accepted);
        client.unwrap();
    }

    /// When the instance after a client can't be made, that client is still served, and the
    /// listener tries again, pausing between tries, until the next client gets in.
    #[test]
    fn a_failing_instance_neither_drops_a_client_nor_stops_the_listener() {
        let name = test_name("instance-fault");
        let mut listener = listen(&name).unwrap();
        let stopper = listener.stopper();
        listener.faults().instances.store(3, SeqCst);
        let server = thread::spawn(move || {
            let first = listener.accept();
            let second = listener.accept();
            [first, second].map(|pipe| pipe.as_ref().and_then(first_byte))
        });
        let first = connect(&name).unwrap();
        let _ = first.write_all(b"1");
        let second = connect(&name);
        match &second {
            Ok(second) => {
                let _ = second.write_all(b"2");
            }
            Err(_) => stopper.stop(),
        }
        assert_eq!(server.join().unwrap(), [Some(b'1'), Some(b'2')]);
        second.unwrap();
    }

    /// A wait for a client that fails is tried again on a fresh instance, made before the failed
    /// one goes, so no other process can take the name meanwhile.
    #[test]
    fn a_failing_connect_is_tried_again_and_the_name_stays_held() {
        let name = test_name("connect-fault");
        let mut listener = listen(&name).unwrap();
        let stopper = listener.stopper();
        let faults = listener.faults();
        faults.connects.store(2, SeqCst);
        let server = thread::spawn(move || {
            let pipe = listener.accept()?;
            let byte = first_byte(&pipe)?;
            pipe.write_all(&[byte]).ok()?;
            Some(byte)
        });
        wait_until("both failures", || {
            faults.connects.load(SeqCst) == 0 || server.is_finished()
        });
        assert!(listen(&name).is_err(), "the name was let go");
        // A client that reaches an instance as it is replaced is let go, and tries again.
        let served = (0..20).any(|_| {
            let Ok(client) = connect(&name) else {
                return false;
            };
            client.write_all(b"x").is_ok() && first_byte(&client) == Some(b'x')
        });
        if !served {
            stopper.stop();
        }
        assert_eq!(server.join().unwrap(), Some(b'x'));
    }

    /// A read given a time ends when nothing arrives in it, the connection still open, and what
    /// arrives later is read.
    #[test]
    fn a_read_within_a_time_ends_when_nothing_arrives() {
        let name = test_name("within");
        let mut listener = listen(&name).unwrap();
        let server = thread::spawn(move || listener.accept().expect("a client"));
        let client = connect(&name).unwrap();
        let pipe = server.join().unwrap();
        let (done, read) = std::sync::mpsc::channel();
        let reading = thread::spawn(move || {
            let mut buf = [0u8; 4];
            let started = Instant::now();
            let timed_out = pipe.read_within(&mut buf, Duration::from_millis(200));
            let waited = started.elapsed();
            let _ = done.send((timed_out.map_err(|e| e.kind()), waited));
            (pipe, buf)
        });
        let (timed_out, waited) = read
            .recv_timeout(Duration::from_secs(5))
            .expect("the read ends in its time");
        assert_eq!(timed_out, Err(ErrorKind::TimedOut));
        assert!(waited >= Duration::from_millis(150), "{waited:?}");
        let (pipe, mut buf) = reading.join().unwrap();
        client.write_all(b"ok").unwrap();
        assert_eq!(
            pipe.read_within(&mut buf, Duration::from_secs(5)).unwrap(),
            2
        );
        assert_eq!(&buf[..2], b"ok");
    }

    /// A wait for a client that fails (`WAIT_FAILED`) is an error like any other, logged and
    /// tried again: only the stop ends `accept`.
    #[test]
    fn a_failed_wait_for_a_client_is_tried_again_not_taken_for_the_stop() {
        let name = test_name("wait-fault");
        let mut listener = listen(&name).unwrap();
        let stopper = listener.stopper();
        let faults = listener.faults();
        faults.waits.store(1, SeqCst);
        let server = thread::spawn(move || listener.accept().as_ref().and_then(first_byte));
        wait_until("the failure", || {
            faults.waits.load(SeqCst) == 0 || server.is_finished()
        });
        // Kept open until the server has read from them.
        let mut clients = Vec::new();
        let served = (0..20).any(|_| {
            thread::sleep(Duration::from_millis(50));
            let Ok(client) = connect(&name) else {
                return false;
            };
            let wrote = client.write_all(b"w").is_ok();
            clients.push(client);
            wrote
        });
        if !served {
            stopper.stop();
        }
        assert_eq!(server.join().unwrap(), Some(b'w'));
    }

    /// A client that finds every instance busy waits a moment, then gives up.
    #[test]
    fn a_client_finding_every_instance_busy_gives_up() {
        let name = test_name("busy");
        let _listener = listen(&name).unwrap();
        let _first = connect(&name).unwrap();
        let started = Instant::now();
        let err = connect(&name).err().expect("a second instance");
        assert_eq!(err.kind(), ErrorKind::TimedOut, "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_second_server_on_the_name_is_refused() {
        let name = test_name("second");
        let _first = listen(&name).unwrap();
        let second = listen(&name).err().expect("a second first instance");
        assert!(is(&second, ERROR_ACCESS_DENIED), "{second:?}");
    }

    #[test]
    fn no_server_is_not_found() {
        let err = connect(&test_name("absent")).err().expect("no pipe");
        assert_eq!(err.kind(), ErrorKind::NotFound, "{err:?}");
    }

    #[test]
    fn a_stopped_listener_accepts_nothing() {
        let name = test_name("stop");
        let mut listener = listen(&name).unwrap();
        let stopper = listener.stopper();
        let waiting = thread::spawn(move || listener.accept().is_none());
        thread::sleep(Duration::from_millis(100));
        stopper.stop();
        assert!(waiting.join().unwrap());
    }

    #[test]
    fn several_clients_connect_at_once() {
        let name = test_name("many");
        let mut listener = listen(&name).unwrap();
        let server = thread::spawn(move || {
            (0..3)
                .map(|_| listener.accept().expect("a client"))
                .collect::<Vec<_>>()
        });
        let clients: Vec<Pipe> = (0..3).map(|_| connect(&name).unwrap()).collect();
        let served = server.join().unwrap();
        for (i, client) in clients.iter().enumerate() {
            client.write_all(&[b'0' + i as u8]).unwrap();
        }
        let mut seen: Vec<u8> = served
            .iter()
            .map(|pipe| {
                let mut buf = [0u8; 1];
                assert_eq!(pipe.read(&mut buf).unwrap(), 1);
                buf[0]
            })
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, b"012");
    }

    /// A client whose token has the user's SID only for denying (a restricted token) is kept
    /// out by the DACL, which grants that SID alone.
    #[test]
    fn a_client_without_the_user_sid_is_refused() {
        let name = test_name("restricted");
        let _listener = listen(&name).unwrap();
        let restricted_name = name.clone();
        let refused = thread::spawn(move || {
            let mut token = HANDLE::default();
            // SAFETY: the current process's pseudo-handle; `token` is a live local.
            unsafe {
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_IMPERSONATE,
                    &mut token,
                )
            }
            .unwrap();
            let token = Owned(token);
            let mut len = 0u32;
            // SAFETY: as in `user_sid`.
            let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut len) };
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            // SAFETY: as in `user_sid`.
            unsafe {
                GetTokenInformation(
                    token.0,
                    TokenUser,
                    Some(buf.as_mut_ptr().cast()),
                    len,
                    &mut len,
                )
            }
            .unwrap();
            // SAFETY: as in `user_sid`.
            let user = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User };
            let disable = [SID_AND_ATTRIBUTES {
                Sid: user.Sid,
                Attributes: 0,
            }];
            let mut restricted = HANDLE::default();
            // SAFETY: `disable` points into `buf`, alive across the call.
            unsafe {
                CreateRestrictedToken(
                    token.0,
                    DISABLE_MAX_PRIVILEGE,
                    Some(&disable),
                    None,
                    None,
                    &mut restricted,
                )
            }
            .unwrap();
            let restricted = Owned(restricted);
            // SAFETY: impersonates on this thread alone, reverted below.
            unsafe { ImpersonateLoggedOnUser(restricted.0) }.unwrap();
            let opened = connect(&restricted_name);
            // SAFETY: ends this thread's impersonation.
            unsafe { RevertToSelf() }.unwrap();
            opened.err().expect("the restricted client got in")
        })
        .join()
        .unwrap();
        assert_eq!(refused.kind(), ErrorKind::PermissionDenied, "{refused:?}");
        // The same process, as itself, gets in.
        connect(&name).unwrap();
    }

    /// `name` as a client reaches it through the network: `\\localhost\pipe\…`.
    fn through_the_network(name: &str) -> String {
        name.replacen(r"\\.\", r"\\localhost\", 1)
    }

    /// The pipe refuses clients from the network, which `\\localhost\pipe\…` goes through. A
    /// pipe like it but taking remote clients, as a control, is reached that way; where it isn't
    /// (no file sharing on this machine), the test says so and passes.
    #[test]
    fn a_remote_client_is_refused() {
        let name = test_name("remote");
        let _listener = listen(&name).unwrap();
        let control = test_name("remote-control");
        let security = Security::current_user_only().unwrap();
        let wide = HSTRING::from(control.as_str());
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security.0 .0,
            bInheritHandle: false.into(),
        };
        // SAFETY: as in `instance`, without PIPE_REJECT_REMOTE_CLIENTS.
        let handle = unsafe {
            CreateNamedPipeW(
                &wide,
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER,
                BUFFER,
                0,
                Some(&attributes),
            )
        };
        assert!(!handle.is_invalid(), "{:?}", io::Error::last_os_error());
        let _control = Owned(handle);
        if let Err(e) = connect(&through_the_network(&control)) {
            eprintln!("skipped: the network doesn't reach this machine's pipes here ({e})");
            return;
        }
        let refused = connect(&through_the_network(&name))
            .err()
            .expect("a remote client got in");
        assert!(is(&refused, ERROR_ACCESS_DENIED), "{refused:?}");
    }
}
