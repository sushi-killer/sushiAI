use super::*;
use tokio::sync::{AcquireError, OwnedSemaphorePermit, TryAcquireError};

struct State {
    capacity: usize,
    debt: usize,
    running: usize,
}

/// A semaphore that retires permits held during a live limit reduction.
/// A zero limit uses Tokio's maximum capacity, so disabling the check limit
/// also wakes existing waiters without replacing their semaphore.
pub(super) struct ResizableSlots {
    semaphore: Arc<Semaphore>,
    state: StdMutex<State>,
}

pub(super) struct SlotPermit {
    slots: Arc<ResizableSlots>,
    permit: Option<OwnedSemaphorePermit>,
}

impl ResizableSlots {
    pub(super) fn new(limit: usize) -> Arc<Self> {
        let capacity = Self::capacity(limit);
        Arc::new(Self {
            semaphore: Arc::new(Semaphore::new(capacity)),
            state: StdMutex::new(State {
                capacity,
                debt: 0,
                running: 0,
            }),
        })
    }

    fn capacity(limit: usize) -> usize {
        if limit == 0 {
            Semaphore::MAX_PERMITS
        } else {
            limit
        }
    }

    pub(super) fn resize(&self, limit: usize) {
        let capacity = Self::capacity(limit);
        let mut state = self.state.lock().unwrap();
        if capacity < state.capacity {
            let remove = state.capacity - capacity;
            state.debt += remove - self.semaphore.forget_permits(remove);
        } else {
            let add = capacity - state.capacity;
            let retire = add.min(state.debt);
            state.debt -= retire;
            self.semaphore.add_permits(add - retire);
        }
        state.capacity = capacity;
    }

    fn admit(self: &Arc<Self>, permit: OwnedSemaphorePermit) -> Option<SlotPermit> {
        let mut state = self.state.lock().unwrap();
        // A queued acquire may already own a permit when the limit shrinks.
        // Retire it before admitting a new owner, just like a returning one.
        if state.debt > 0 {
            state.debt -= 1;
            permit.forget();
            return None;
        }
        state.running += 1;
        Some(SlotPermit {
            slots: self.clone(),
            permit: Some(permit),
        })
    }

    pub(super) async fn acquire_owned(self: Arc<Self>) -> Result<SlotPermit, AcquireError> {
        loop {
            let permit = self.semaphore.clone().acquire_owned().await?;
            if let Some(permit) = self.admit(permit) {
                return Ok(permit);
            }
        }
    }

    pub(super) fn try_acquire_owned(self: Arc<Self>) -> Result<SlotPermit, TryAcquireError> {
        loop {
            let permit = self.semaphore.clone().try_acquire_owned()?;
            if let Some(permit) = self.admit(permit) {
                return Ok(permit);
            }
        }
    }

    pub(super) fn usage(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap();
        (
            state.running,
            if state.capacity == Semaphore::MAX_PERMITS {
                0
            } else {
                state.capacity
            },
        )
    }

    #[cfg(test)]
    pub(super) fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

impl Drop for SlotPermit {
    fn drop(&mut self) {
        let mut state = self.slots.state.lock().unwrap();
        state.running -= 1;
        let permit = self.permit.take().unwrap();
        if state.debt > 0 {
            state.debt -= 1;
            permit.forget();
        } else {
            drop(permit);
        }
    }
}

pub(super) enum Slot {
    Got(SlotPermit),
    Cancelled,
    Closed,
}

/// A global slot for the loop of `task`. While none is free the task shows
/// the `slot` wait (running and limit), and `cancel` ends the wait.
pub(super) async fn take_slot(app: &Arc<App>, task: &mut Task, cancel: &CancelToken) -> Slot {
    if let Ok(permit) = app.slots.clone().try_acquire_owned() {
        return Slot::Got(permit);
    }
    let (running, limit) = app.slots.usage();
    let wait = Wait::Slot {
        running: running as u32,
        limit: limit as u32,
    };
    task.queue.wait = Some(wait.clone());
    if let Err(e) = app.update_task(&task.id, |t| t.queue.wait = Some(wait)) {
        eprintln!("orch: updating task {} failed: {e}", task.id);
    }
    let got = tokio::select! {
        _ = cancel.cancelled() => Slot::Cancelled,
        permit = app.slots.clone().acquire_owned() => match permit {
            Ok(p) => Slot::Got(p),
            Err(_) => Slot::Closed,
        },
    };
    task.queue.wait = None;
    if let Err(e) = app.update_task(&task.id, |t| {
        if matches!(t.queue.wait, Some(Wait::Slot { .. })) {
            t.queue.wait = None;
        }
    }) {
        eprintln!("orch: updating task {} failed: {e}", task.id);
    }
    got
}
