pub(in crate::engine) mod admission;
mod base_question;
mod commit;
mod delivery;
mod final_results;
pub(in crate::engine) mod finish;
mod retry;

pub(in crate::engine) enum Tail {
    Continue,
    Return,
}

#[cfg(test)]
mod tests;
