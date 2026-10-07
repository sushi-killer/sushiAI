use super::*;

/// `overrides` is a partial `Variant` object; its keys replace the defaults'.
pub(super) fn resolve_variant(
    defaults: &Variant,
    overrides: Option<&serde_json::Value>,
) -> Result<Variant, String> {
    let Some(overrides) = overrides else {
        return Ok(defaults.clone());
    };
    let Some(fields) = overrides.as_object() else {
        return Err("variant must be an object".to_string());
    };
    let mut merged = serde_json::to_value(defaults).map_err(|e| e.to_string())?;
    for (k, v) in fields {
        if Variant::RETIRED_KEYS.contains(&k.as_str()) {
            return Err(format!(
                "variant flag {k} was retired: its winning behaviour is now the default (or it was removed), so drop it from the override"
            ));
        }
        // Checked here, not with deny_unknown_fields: a task.json keeps
        // loading after a flag it names is retired.
        if merged.get(k).is_none() && !Variant::OPTIONAL_KEYS.contains(&k.as_str()) {
            return Err(format!("unknown variant flag: {k}"));
        }
        merged[k] = v.clone();
    }
    let variant: Variant =
        serde_json::from_value(merged).map_err(|e| format!("invalid variant: {e}"))?;
    variant.check()?;
    Ok(variant)
}
