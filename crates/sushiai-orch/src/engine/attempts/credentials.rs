use crate::engine::state::App;
use crate::harness;
use crate::policy::routing::Route;
use crate::store;
use serde_json::json;
use std::path::Path;

pub(super) fn claude_profile(
    app: &App,
    route: &Route,
    run_dir: &Path,
    key_path: &Path,
) -> Option<serde_json::Value> {
    let profile = route
        .profile_id
        .as_ref()
        .and_then(|pid| app.secrets.read().unwrap().profiles.get(pid).cloned());
    let mut profile_obj = serde_json::Map::new();
    if let Some(p) = &profile {
        if !p.env.is_empty() {
            profile_obj.insert(
                "env".to_string(),
                serde_json::to_value(&p.env).unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some(key) = &p.key {
            if store::files::write_secret_file(key_path, key).is_ok() {
                profile_obj.insert(
                    "apiKeyHelper".to_string(),
                    serde_json::Value::String(format!(
                        "cat {}",
                        harness::shell_quote(&key_path.to_string_lossy())
                    )),
                );
            }
        }
    }
    if let Some(account) = route
        .account_id
        .as_ref()
        .and_then(|id| app.secrets.read().unwrap().accounts.get(id).cloned())
    {
        match account.kind.as_str() {
            "apiKey" => {
                if store::files::write_secret_file(key_path, &account.value).is_ok() {
                    profile_obj.insert(
                        "apiKeyHelper".into(),
                        json!(format!(
                            "cat {}",
                            harness::shell_quote(&key_path.to_string_lossy())
                        )),
                    );
                }
            }
            "subscription" => {
                let _ =
                    store::files::write_secret_file(&run_dir.join("oauth-token"), &account.value);
            }
            _ => {}
        }
    }
    if profile_obj.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(profile_obj))
    }
}
