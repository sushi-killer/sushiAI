use crate::task::attempt::VerifyOutcome;

pub(crate) fn machine_load() -> (f64, u32) {
    let mut load = 0.0;
    // getloadavg writes at most one double into this buffer.
    unsafe {
        libc::getloadavg(&mut load, 1);
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    (load, cores)
}

pub(crate) fn timed_out_under_load(outcome: &VerifyOutcome) -> bool {
    outcome.load_timeout
        || (outcome.code.is_none()
            && outcome.tail.starts_with("timed out after ")
            && outcome.cores > 0
            && outcome.load >= f64::from(outcome.cores))
}
