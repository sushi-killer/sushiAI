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

    /// `costs.summary {repo?, taskId?, sinceDays? | from? / to? (UTC YYYY-MM-DD, inclusive), groupBy: [stage|model|route|repo|task|day]}`
    /// -> `{rows: [{key, keys, costUsd, runs, tokens, cacheHitRate}], totals,
    /// leadTouch: {touched, marked, rate, byRepo, byWeek},
    /// tasksBySource: {source: {tasks, costUsd}}}`.
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
            from: Option<String>,
            #[serde(default)]
            to: Option<String>,
            #[serde(default)]
            group_by: Vec<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        crate::costs::check_group_by(&p.group_by)?;
        let (from_day, to_day) =
            crate::costs::check_window(p.since_days, p.from.as_deref(), p.to.as_deref())?;
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
            from_day,
            to_day,
            group_by: p.group_by,
        };
        let mut summary = crate::costs::summarize(&records, &query, now_ms());
        summary["tasksBySource"] = crate::costs::source_summary(&tasks, &query, now_ms());
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
        if !p.settings.daily_budget_usd.is_finite() || p.settings.daily_budget_usd < 0.0 {
            return Err("dailyBudgetUsd must be a finite number, 0 or more".to_string());
        }
        if p.settings.verify_timeout_secs == 0 {
            return Err("verifyTimeoutSecs must be at least 1".to_string());
        }
        p.settings.experiments.check()?;
        let mut tool_ids = std::collections::HashSet::new();
        for tool in &p.settings.chat_tools {
            if tool.id.trim().is_empty() || !tool_ids.insert(tool.id.as_str()) {
                return Err("chatTools ids must be unique and not empty".to_string());
            }
            if !tool.server.is_object() {
                return Err("a chat tool's server must be an object".to_string());
            }
        }
        p.settings.experiments.check_routes(&p.settings.routes)?;
        self.store
            .save_settings(&p.settings)
            .map_err(|e| e.to_string())?;
        *self.settings.write().unwrap() = p.settings.clone();
        self.refresh_availability();
        set_verify_timeout_secs(p.settings.verify_timeout_secs);
        self.advance_autopilot();
        serde_json::to_value(&p.settings).map_err(|e| e.to_string())
    }

    /// A *full* replace (spec): every call overwrites the whole in-memory
    /// secrets state, it never merges into what's already there.
    pub(super) async fn handle_secrets_set(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize, Default)]
        struct P {
            #[serde(default)]
            profiles: HashMap<String, ProfileSecret>,
            #[serde(default)]
            accounts: HashMap<String, AccountSecret>,
            #[serde(default)]
            projects: HashMap<String, HashMap<String, String>>,
            #[serde(default, rename = "projectMcp")]
            project_mcp: HashMap<String, HashMap<String, String>>,
            #[serde(default, rename = "projectRepos")]
            repo_projects: HashMap<String, String>,
        }
        let p: P = serde_json::from_value(params).unwrap_or_default();
        let mut secrets = self.secrets.write().unwrap();
        *secrets = Secrets {
            profiles: p.profiles,
            accounts: p.accounts,
            projects: p.projects,
            project_mcp: p.project_mcp,
            repo_projects: p.repo_projects,
        };
        Ok(json!({}))
    }

    pub(super) async fn handle_shutdown(&self) -> Result<serde_json::Value, String> {
        self.shutdown();
        Ok(json!({}))
    }
}
