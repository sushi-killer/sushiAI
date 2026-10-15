pub(in crate::engine) mod admission;
pub(in crate::engine) mod delivery;
pub(in crate::engine) mod finish;

pub(in crate::engine) enum Tail {
    Continue,
    Return,
}

#[cfg(test)]
mod tests;
