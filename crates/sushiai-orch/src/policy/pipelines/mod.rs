pub(crate) mod catalog;
pub(crate) mod delivery;
pub(crate) mod planning;
pub(crate) mod task;

use serde::de::DeserializeOwned;

pub(super) fn decode<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_saphyr::from_str(text).map_err(|error| error.to_string())
}
