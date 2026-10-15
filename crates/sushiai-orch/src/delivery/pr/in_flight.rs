use crate::delivery::pr::error::PrRefusal;
use std::collections::HashSet;
use std::sync::Mutex as StdMutex;

/// Task ids with a `task.pr` running: a second call is refused.
static IN_FLIGHT: StdMutex<Option<HashSet<String>>> = StdMutex::new(None);

pub(crate) struct InFlight(String);

impl InFlight {
    pub(crate) fn take(id: &str) -> Result<InFlight, PrRefusal> {
        let mut guard = IN_FLIGHT.lock().unwrap();
        if !guard
            .get_or_insert_with(HashSet::new)
            .insert(id.to_string())
        {
            return Err(PrRefusal::InFlight);
        }
        Ok(InFlight(id.to_string()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(set) = IN_FLIGHT.lock().unwrap().as_mut() {
            set.remove(&self.0);
        }
    }
}
