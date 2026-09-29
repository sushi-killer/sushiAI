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

    /// `costs.summary {repo?, taskId?, sinceDays?, groupBy: [stage|model|route|repo|task|day]}`
    /// -> `{rows: [{key, keys, costUsd, runs, tokens, cacheHitRate}], totals,
    /// leadTouch: {touched, marked, rate, byRepo, byWeek}}`.
    pub(super) async fn handle_costs_summary(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct P {
            #[serde(default)]
            repo: Option<String>,
            #[serde(default)]
            task_id: Option<String>,
            #[serde(default)]
            since_days: Option<u32>,
            #[serde(default)]
            group_by: Vec<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        crate::costs::check_group_by(&p.group_by)?;
        let data = self.data_dir.clone();
        let (records, tasks) = tokio::task::spawn_blocking(move || {
            (
                crate::costs::read_all(&data),
                crate::costs::read_tasks(&data),
            )
        })
        .await
        .map_err(|e| e.to_string())?;
        let query = crate::costs::Query {
            repo: p.repo,
            task_id: p.task_id,
            since_days: p.since_days,
            group_by: p.group_by,
        };
        let mut summary = crate::costs::summarize(&records, &query, now_ms());
        summary["leadTouch"] = crate::costs::lead_touch_summary(&tasks, &query, now_ms());
        Ok(summary)
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
