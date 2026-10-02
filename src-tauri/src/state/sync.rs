//! Hand-over primitives between threads: lock helpers that shrug off poisoning, a one-shot
//! `Slot` with a timeout, and a `Gate` threads can wait on.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// A value handed from one thread to another once, waited for with a timeout.
pub struct Slot<T> {
    state: Mutex<SlotState<T>>,
    filled: Condvar,
}

type Later<T> = Box<dyn FnOnce(T) + Send>;

enum SlotState<T> {
    Waiting,
    Full(T),
    /// The taker gave up waiting; the value goes here when it lands.
    Late(Later<T>),
    /// Taken, or given up on for good; a late value is dropped.
    Taken,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(SlotState::Waiting),
            filled: Condvar::new(),
        }
    }
}

impl<T: 'static> Slot<T> {
    pub fn fill(&self, value: T) {
        let mut state = lock(&self.state);
        match std::mem::replace(&mut *state, SlotState::Taken) {
            SlotState::Waiting => {
                *state = SlotState::Full(value);
                self.filled.notify_all();
            }
            SlotState::Late(later) => {
                drop(state);
                later(value);
            }
            kept @ (SlotState::Full(_) | SlotState::Taken) => *state = kept,
        }
    }

    /// The value, waiting up to `timeout` for it; a value landing later is dropped.
    pub fn take(&self, timeout: Duration) -> Option<T> {
        self.take_or_later(timeout, drop)
    }

    /// The value if it lands within `timeout`; otherwise `later` gets it when it does. Only the
    /// first call can get it.
    pub fn take_or_later(
        &self,
        timeout: Duration,
        later: impl FnOnce(T) + Send + 'static,
    ) -> Option<T> {
        let state = lock(&self.state);
        let (mut state, _) = self
            .filled
            .wait_timeout_while(state, timeout, |s| matches!(s, SlotState::Waiting))
            .unwrap_or_else(PoisonError::into_inner);
        match std::mem::replace(&mut *state, SlotState::Taken) {
            SlotState::Full(value) => Some(value),
            SlotState::Waiting => {
                *state = SlotState::Late(Box::new(later));
                None
            }
            kept @ (SlotState::Late(_) | SlotState::Taken) => {
                *state = kept;
                None
            }
        }
    }
}

/// A flag threads can wait on.
#[derive(Default)]
pub(super) struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    pub(super) fn open(&self) {
        *lock(&self.open) = true;
        self.opened.notify_all();
    }

    pub(super) fn is_open(&self) -> bool {
        *lock(&self.open)
    }

    /// Waits up to `timeout` for the gate to open; true when it is.
    pub(super) fn wait(&self, timeout: Duration) -> bool {
        let open = lock(&self.open);
        let (open, _) = self
            .opened
            .wait_timeout_while(open, timeout, |open| !*open)
            .unwrap_or_else(PoisonError::into_inner);
        *open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_slot_hands_over_one_value_and_drops_one_that_lands_after_take_gave_up() {
        let slot = Slot::default();
        slot.fill(1);
        slot.fill(2);
        assert_eq!(slot.take(Duration::ZERO), Some(1));
        assert_eq!(slot.take(Duration::ZERO), None);
        let late: Slot<u8> = Slot::default();
        assert_eq!(late.take(Duration::from_millis(5)), None);
        late.fill(3);
        assert_eq!(late.take(Duration::ZERO), None);
    }

    #[test]
    fn a_slot_hands_a_late_value_to_the_waiter_that_gave_up() {
        let slot: Slot<u8> = Slot::default();
        let got = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&got);
        let taken = slot.take_or_later(Duration::from_millis(5), move |v| {
            *lock(&sink) = Some(v);
        });
        assert_eq!(taken, None);
        slot.fill(7);
        assert_eq!(*lock(&got), Some(7));
        assert_eq!(slot.take(Duration::ZERO), None);
    }
}
