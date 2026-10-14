use super::*;

impl App {
    /// `overseer.log {repo?, limit?}` -> the overseer's actions, newest first.
    pub(super) async fn handle_overseer_log(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize, Default)]
        struct P {
            #[serde(default)]
            repo: Option<String>,
            #[serde(default)]
            limit: Option<usize>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let entries = crate::overseer::read(&self.data_dir, p.repo.as_deref(), p.limit);
        serde_json::to_value(entries).map_err(|e| e.to_string())
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
        if p.settings.digest_hour.is_some_and(|h| h > 23) {
            return Err("digestHour must be 0-23 or null".to_string());
        }
        super::owner_notice::quiet_window(&p.settings.quiet_hours)?;
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
        p.settings.check_presets()?;
        if !p.settings.pr_route.is_empty()
            && !p
                .settings
                .routes
                .iter()
                .any(|r| r.id == p.settings.pr_route)
        {
            return Err(format!(
                "prRoute \"{}\" is not a configured route",
                p.settings.pr_route
            ));
        }
        let answering = |s: &Settings| s.auto_answer && self.chat_route(s).is_some();
        let was_answering = answering(&self.settings.read().unwrap());
        {
            let mut settings = self.settings.write().unwrap();
            self.store
                .save_settings(&p.settings)
                .map_err(|e| e.to_string())?;
            self.slots.resize(p.settings.parallel.max(1) as usize);
            self.check_slots
                .resize(p.settings.heavy_check_limit() as usize);
            *settings = p.settings.clone();
        }
        self.refresh_availability();
        set_verify_timeout_secs(p.settings.verify_timeout_secs);
        self.advance_autopilot();
        if !was_answering && answering(&p.settings) {
            super::questions::wake_orchestrator_for_parked(self);
        }
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
            #[serde(default, rename = "projectBranches")]
            project_branches: HashMap<String, String>,
        }
        let p: P = serde_json::from_value(params).unwrap_or_default();
        let mut secrets = self.secrets.write().unwrap();
        *secrets = Secrets {
            profiles: p.profiles,
            accounts: p.accounts,
            projects: p.projects,
            project_mcp: p.project_mcp,
            repo_projects: p.repo_projects,
            received: true,
            project_branches: p
                .project_branches
                .into_iter()
                .map(|(folder, branch)| (folder, branch.trim().to_string()))
                .filter(|(_, branch)| !branch.is_empty())
                .collect(),
        };
        secrets::redact_old_run_mcp(&self.data_dir, &secrets);
        Ok(json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settings_set_refuses_a_malformed_quiet_hours_and_keeps_a_valid_one() {
        let (app, _dir) = test_support::test_app();
        let set = |quiet: &str| {
            let mut settings = app.settings.read().unwrap().clone();
            settings.quiet_hours = quiet.to_string();
            json!({"settings": settings})
        };
        for bad in ["25:00-08:00", "23:00", "09:00-09:00", "late"] {
            let err = app.handle_settings_set(set(bad)).await.unwrap_err();
            assert!(err.contains("quietHours"), "{bad}: {err}");
        }
        let ok = app.handle_settings_set(set("22:30-07:00")).await.unwrap();
        assert_eq!(ok["quietHours"], "22:30-07:00");
        app.handle_settings_set(set("")).await.unwrap();
        assert_eq!(app.settings.read().unwrap().quiet_hours, "");
        assert_eq!(Settings::default().quiet_hours, "23:00-08:00");
    }
}
