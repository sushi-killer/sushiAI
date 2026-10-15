use super::clustering::clusters;
use super::evidence::read_live_signals;
use super::job::PreparedProposal;
use crate::engine::state::App;
use crate::policy::routing::{Route, Tier};
use crate::policy::settings::Settings;
use serde_json::json;
use std::collections::HashSet;

impl App {
    /// The route a proposer run uses: `proposerRoute` when it names a
    /// configured route, else the hard tier's.
    fn proposer_route(settings: &Settings) -> Result<Route, String> {
        let named = |id: &str| settings.routes.iter().find(|r| r.id == id).cloned();
        named(&settings.evolution.proposer_route)
            .or_else(|| settings.tiers.get(&Tier::Hard).and_then(|id| named(id)))
            .ok_or_else(|| "no route is configured for the hard tier".to_string())
    }

    pub(crate) async fn handle_evolution_run(&self) -> Result<serde_json::Value, String> {
        let updated = self.measure_proposals().await;
        let settings = self.settings.read().unwrap().clone();
        let stored: HashSet<String> = self
            .store
            .list_proposals()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|proposal| proposal.cluster_key)
            .collect();
        let signals = read_live_signals(&self.data_dir);
        let mut started = Vec::new();
        let mut jobs = Vec::new();
        let limit = settings.evolution.max_proposals as usize;
        for cluster in clusters(&signals) {
            if jobs.len() >= limit {
                break;
            }
            if !cluster.qualifies(&settings.evolution) || stored.contains(&cluster.key) {
                continue;
            }
            if let Some(job) = PreparedProposal::prepare(self, &cluster).await {
                started.push(job.summary(&cluster));
                jobs.push(job);
            }
        }
        if !jobs.is_empty() {
            let route = match Self::proposer_route(&settings) {
                Ok(route) => route,
                Err(error) => {
                    let mut in_flight = self.proposals.lock().unwrap();
                    for job in &jobs {
                        in_flight.remove(&job.job.key);
                    }
                    return Err(error);
                }
            };
            for job in jobs {
                job.launch(self, route.clone());
            }
        }
        Ok(json!({ "started": started, "updated": updated }))
    }
}
