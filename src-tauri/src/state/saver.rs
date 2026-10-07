//! Writing the workspaces, settings and reading state to disk, debounced, on a thread of its own.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use lectern_core::ipc::Settings;
use lectern_core::store::write_json_atomic;
use lectern_core::workspace::{Workspaces, WORKSPACES_FILE};

use super::profile::{StateFile, SETTINGS_FILE, STATE_FILE};

pub(super) const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

/// Writes the workspaces, settings and state on a thread of its own, a moment after the last
/// change. A saver made with `persist` false writes nothing.
pub(super) struct Saver {
    tx: Option<Sender<Save>>,
}

pub(super) enum Save {
    Workspaces(Box<Workspaces>),
    Settings(Box<Settings>),
    State(Box<StateFile>),
    Flush(Sender<()>),
}

impl Saver {
    pub(super) fn new(dir: PathBuf, persist: bool) -> Self {
        if !persist {
            return Self { tx: None };
        }
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("lectern-save".to_owned())
            .spawn(move || save_loop(&dir, &rx));
        if let Err(e) = spawned {
            log::error!("couldn't start the save thread; settings won't be saved: {e}");
        }
        Self { tx: Some(tx) }
    }

    pub(super) fn send(&self, save: Save) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(save);
        }
    }

    pub(super) fn workspaces(&self, workspaces: &Workspaces) {
        self.send(Save::Workspaces(Box::new(workspaces.clone())));
    }

    pub(super) fn settings(&self, settings: &Settings) {
        self.send(Save::Settings(Box::new(settings.clone())));
    }

    pub(super) fn state(&self, state: &StateFile) {
        self.send(Save::State(Box::new(state.clone())));
    }

    pub(super) fn flush(&self, timeout: Duration) {
        let Some(tx) = &self.tx else {
            return;
        };
        let (ack, acked) = mpsc::channel();
        if tx.send(Save::Flush(ack)).is_ok() {
            let _ = acked.recv_timeout(timeout);
        }
    }
}

pub(super) fn save_loop(dir: &Path, rx: &Receiver<Save>) {
    let mut workspaces: Option<Box<Workspaces>> = None;
    let mut settings: Option<Box<Settings>> = None;
    let mut state: Option<Box<StateFile>> = None;
    let mut due: Option<Instant> = None;
    loop {
        let message = match due {
            Some(due) => match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    write_pending(dir, &mut workspaces, &mut settings, &mut state);
                    return;
                }
            },
            None => match rx.recv() {
                Ok(message) => Some(message),
                Err(_) => return,
            },
        };
        match message {
            Some(Save::Workspaces(w)) => workspaces = Some(w),
            Some(Save::Settings(s)) => settings = Some(s),
            Some(Save::State(s)) => state = Some(s),
            Some(Save::Flush(ack)) => {
                write_pending(dir, &mut workspaces, &mut settings, &mut state);
                due = None;
                let _ = ack.send(());
                continue;
            }
            None => {
                write_pending(dir, &mut workspaces, &mut settings, &mut state);
                due = None;
                continue;
            }
        }
        due.get_or_insert_with(|| Instant::now() + SAVE_DEBOUNCE);
    }
}

/// Writes what is pending: the workspaces, then the settings that mirror the first of them, then
/// the reading state.
pub(super) fn write_pending(
    dir: &Path,
    workspaces: &mut Option<Box<Workspaces>>,
    settings: &mut Option<Box<Settings>>,
    state: &mut Option<Box<StateFile>>,
) {
    if let Some(workspaces) = workspaces.take() {
        if let Err(e) = write_json_atomic(&dir.join(WORKSPACES_FILE), &*workspaces) {
            log::warn!("couldn't save the workspaces: {e}");
        }
    }
    if let Some(settings) = settings.take() {
        if let Err(e) = write_json_atomic(&dir.join(SETTINGS_FILE), &*settings) {
            log::warn!("couldn't save the settings: {e}");
        }
    }
    if let Some(state) = state.take() {
        if let Err(e) = write_json_atomic(&dir.join(STATE_FILE), &*state) {
            log::warn!("couldn't save the reading state: {e}");
        }
    }
}
