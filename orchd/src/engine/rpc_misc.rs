use super::*;

impl App {
    pub(super) async fn handle_ping(&self) -> Result<serde_json::Value, String> {
        // Only slots actually held by a running attempt count -- a task
        // still queued behind the concurrency limit is not "running".
        // An orchestrator chat reply in progress counts too: replacing the
        // daemon under it would lose the reply.
        let running = self.parallel_limit as usize - self.slots.available_permits()
            + self.chat_turns.lock().unwrap().len();
        Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "pid": self.pid,
            "dataDir": self.data_dir.to_string_lossy(),
            "binaryMtimeMs": self.binary_mtime_ms,
            "running": running,
        }))
    }

    pub(super) async fn handle_settings_get(&self) -> Result<serde_json::Value, String> {
        let s = self.settings.read().unwrap().clone();
        serde_json::to_value(&s).map_err(|e| e.to_string())
    }

    /// The built-in defaults, never the saved settings: the panel compares
    /// against these to show what an owner's older save has frozen.
    pub(super) async fn handle_settings_defaults(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(Settings::default()).map_err(|e| e.to_string())
    }

    pub(super) async fn handle_settings_set(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            settings: Settings,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        p.settings.experiments.check()?;
        p.settings.experiments.check_routes(&p.settings.routes)?;
        self.store
            .save_settings(&p.settings)
            .map_err(|e| e.to_string())?;
        *self.settings.write().unwrap() = p.settings.clone();
        serde_json::to_value(&p.settings).map_err(|e| e.to_string())
    }

    /// A *full* replace (spec): every call overwrites the whole in-memory
    /// secrets state, it never merges into what's already there.
    pub(super) async fn handle_secrets_set(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct ClassifierSecretIn {
            #[serde(default)]
            key: Option<String>,
            #[serde(default)]
            base_url: Option<String>,
        }
        #[derive(Deserialize, Default)]
        struct P {
            #[serde(default)]
            classifier: Option<ClassifierSecretIn>,
            #[serde(default)]
            profiles: HashMap<String, ProfileSecret>,
        }
        let p: P = serde_json::from_value(params).unwrap_or_default();
        let mut secrets = self.secrets.write().unwrap();
        *secrets = Secrets {
            classifier_key: p.classifier.as_ref().and_then(|c| c.key.clone()),
            classifier_base_url: p.classifier.as_ref().and_then(|c| c.base_url.clone()),
            profiles: p.profiles,
        };
        Ok(json!({}))
    }

    pub(super) async fn handle_shutdown(&self) -> Result<serde_json::Value, String> {
        self.shutdown();
        Ok(json!({}))
    }
}
