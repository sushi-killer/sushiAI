use super::*;

impl App {
    pub(super) async fn handle_ping(&self) -> Result<serde_json::Value, String> {
        // Only slots actually held by a running attempt count -- a task
        // still queued behind the concurrency limit is not "running".
        // Orchestrator chat replies in progress are reported apart in
        // `chatTurns`: a task resumes after a restart, a chat reply does not.
        let running = self.parallel_limit as usize - self.slots.available_permits();
        let chat_turns = self.chat_turns.lock().unwrap().len();
        Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "pid": self.pid,
            "dataDir": self.data_dir.to_string_lossy(),
            "binaryMtimeMs": self.binary_mtime_ms,
            "running": running,
            "chatTurns": chat_turns,
        }))
    }

    /// `classify.probe {state, questions}`: asks the configured classifier
    /// arbitrary questions about an arbitrary state, so a new judgement
    /// point can be measured on real cases before orchd relies on it. A
    /// question with `options` is a pick, otherwise a 0..1 probability.
    pub(super) async fn handle_classify_probe(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct Q {
            name: String,
            prompt: String,
            #[serde(default)]
            options: Vec<String>,
        }
        #[derive(Deserialize)]
        struct P {
            state: serde_json::Value,
            questions: Vec<Q>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let questions: Vec<classify::QuestionSpec> = p
            .questions
            .into_iter()
            .map(|q| match q.options.is_empty() {
                true => classify::QuestionSpec::Noul {
                    name: q.name,
                    prompt: q.prompt,
                },
                false => classify::QuestionSpec::Choice {
                    name: q.name,
                    prompt: q.prompt,
                    options: q.options,
                },
            })
            .collect();
        let settings = self.settings.read().unwrap().classifier.clone();
        let key = self.secrets.read().unwrap().classifier_key.clone();
        let base_url = self.secrets.read().unwrap().classifier_base_url.clone();
        let start = std::time::Instant::now();
        let answers = tokio::task::spawn_blocking(move || {
            classify::decide(
                &settings,
                key.as_deref(),
                base_url.as_deref(),
                &p.state,
                &questions,
            )
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        Ok(json!({"answers": answers, "ms": start.elapsed().as_millis() as u64}))
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
