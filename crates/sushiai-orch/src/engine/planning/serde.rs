use crate::policy::routing::Tier;
use crate::task::checks::types::Check;

/// A criterion is a string or an object `{text, visual}`; the object keeps
/// its text here and `visual` is read by [`parse_plan`].
pub(crate) fn lenient_criteria<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<String>, D::Error> {
    let v: Option<Vec<serde_json::Value>> = serde::Deserialize::deserialize(d).unwrap_or(None);
    Ok(v.unwrap_or_default()
        .into_iter()
        .filter_map(|c| match c {
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Object(o) => o.get("text")?.as_str().map(str::to_string),
            _ => None,
        })
        .collect())
}

pub(super) fn lenient_checks<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<Check>, D::Error> {
    let v: Option<Vec<serde_json::Value>> = serde::Deserialize::deserialize(d).unwrap_or(None);
    Ok(v.unwrap_or_default()
        .into_iter()
        .filter_map(|c| serde_json::from_value(c).ok())
        .collect())
}

pub(super) fn lenient_tests<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<crate::task::checks::types::PlannedTest>, D::Error> {
    let v: Option<Vec<serde_json::Value>> = serde::Deserialize::deserialize(d).unwrap_or(None);
    Ok(v.unwrap_or_default()
        .into_iter()
        .filter_map(|c| serde_json::from_value(c).ok())
        .collect())
}

pub(super) fn lenient_check<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Check>, D::Error> {
    let v: Option<serde_json::Value> = serde::Deserialize::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

pub(crate) fn lenient_tier<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Tier>, D::Error> {
    let v: Option<serde_json::Value> = serde::Deserialize::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}
