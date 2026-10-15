mod loader;
pub(crate) mod reply;
pub(crate) mod text;

pub use loader::{brief_text, default, engine_text, fill, get, set_data_dir};

#[cfg(test)]
pub(crate) use loader::assert_engine_snapshot;

pub(crate) mod clip;

/// Bump when any brief template or fixed instruction block changes: it is
/// part of every run's `promptHash`.
pub const BRIEF_TEMPLATE_VERSION: u32 = 15;
