use crate::task::{amendment::Amendment, checks::authority::Origin, Check};
pub(super) struct AmendRequest {
    pub id: String,
    pub amendment: Amendment,
    pub origin: Origin,
}
impl AmendRequest {
    pub(super) fn parse(params: serde_json::Value) -> Result<Self, String> {
        fn field<T: serde::de::DeserializeOwned>(
            params: &serde_json::Value,
            name: &str,
            what: &str,
        ) -> Result<Option<T>, String> {
            match params.get(name) {
                None => Ok(None),
                Some(v) => serde_json::from_value(v.clone())
                    .map(Some)
                    .map_err(|_| format!("{name} must be {what}")),
            }
        }
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "id must be a string".to_string())?
            .to_string();
        let held_out = match params.get("heldOut") {
            None => None,
            Some(serde_json::Value::Null) => Some(None),
            Some(v) => Some(Some(
                serde_json::from_value::<Check>(v.clone())
                    .map_err(|_| "heldOut must be an object {criterion, run} or null")?,
            )),
        };
        let (criteria, visual_criteria) = criteria(&params)?;
        let addenda = match params.get("addendum") {
            None => Vec::new(),
            Some(serde_json::Value::String(text)) if !text.trim().is_empty() => {
                vec![text.trim().to_string()]
            }
            Some(_) => return Err("addendum must be a non-empty string".to_string()),
        };
        let amendment = Amendment {
            criteria,
            visual_criteria,
            verify: field(&params, "verify", "an array of strings")?,
            final_verify: field(&params, "finalVerify", "an array of strings")?,
            screenshot: field(&params, "screenshot", "a string")?,
            checks: field(&params, "checks", "an array of {criterion, run} objects")?,
            held_out,
            depends_on: field(&params, "dependsOn", "an array of strings")?,
            dependency_titles: Vec::new(),
            addenda,
        };
        if amendment.is_empty() {
            return Err(
                "task.amend needs at least one of criteria, verify, finalVerify, screenshot, checks, heldOut, dependsOn, addendum"
                    .to_string(),
            );
        }
        let origin = match params.get("source") {
            None | Some(serde_json::Value::Null) => Origin::Owner,
            Some(serde_json::Value::String(source)) if source.is_empty() => Origin::Owner,
            Some(serde_json::Value::String(_)) => Origin::Other,
            Some(_) => return Err("source must be a string".to_string()),
        };
        Ok(Self {
            id,
            amendment,
            origin,
        })
    }
}
fn criteria(params: &serde_json::Value) -> Result<(Option<Vec<String>>, Vec<String>), String> {
    Ok(match params.get("criteria") {
        None => (None, Vec::new()),
        Some(v) => {
            let bad = || "criteria must be an array of strings or {text, visual} objects";
            let items = v.as_array().ok_or_else(bad)?;
            let mut texts = Vec::new();
            let mut visual = Vec::new();
            for item in items {
                match item {
                    serde_json::Value::String(s) => texts.push(s.clone()),
                    serde_json::Value::Object(o) => {
                        let text = o.get("text").and_then(|t| t.as_str()).ok_or_else(bad)?;
                        if o.get("visual").and_then(|f| f.as_bool()) == Some(true) {
                            visual.push(text.to_string());
                        }
                        texts.push(text.to_string());
                    }
                    _ => return Err(bad().to_string()),
                }
            }
            (Some(texts), visual)
        }
    })
}
