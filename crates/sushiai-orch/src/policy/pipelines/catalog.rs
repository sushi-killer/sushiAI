use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
enum StageId {
    Brief,
    Plan,
    BriefCheck,
    BaseChecks,
    Implement,
    Checks,
    HeldOut,
    Review,
    Judge,
    Land,
    Advisor,
    Question,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum StageKind {
    Main,
    Side,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogStage {
    id: StageId,
    label: String,
    kind: StageKind,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Catalog {
    stages: Vec<CatalogStage>,
}

pub(crate) fn compiled() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        Catalog::parse(include_str!("../../../pipelines/catalog.yaml"))
            .expect("pipelines/catalog.yaml is valid")
    })
}

impl Catalog {
    fn parse(text: &str) -> Result<Self, String> {
        let catalog: Self = super::decode(text)?;
        let mut seen = BTreeSet::new();
        for stage in &catalog.stages {
            if !seen.insert(stage.id) {
                return Err(format!("duplicate catalog stage: {:?}", stage.id));
            }
            if stage.label.trim().is_empty() {
                return Err(format!("empty catalog label: {:?}", stage.id));
            }
            let side = matches!(stage.id, StageId::Advisor | StageId::Question);
            if side != (stage.kind == StageKind::Side) {
                return Err(format!("incorrect catalog kind: {:?}", stage.id));
            }
        }
        if seen.len() != 12 {
            return Err("catalog must describe every RPC stage".into());
        }
        Ok(catalog)
    }

    pub(crate) fn stages(&self) -> &[CatalogStage] {
        &self.stages
    }
}

impl CatalogStage {
    pub(crate) fn id(&self) -> &'static str {
        match self.id {
            StageId::Brief => "brief",
            StageId::Plan => "plan",
            StageId::BriefCheck => "brief-check",
            StageId::BaseChecks => "base-checks",
            StageId::Implement => "implement",
            StageId::Checks => "checks",
            StageId::HeldOut => "held-out",
            StageId::Review => "review",
            StageId::Judge => "judge",
            StageId::Land => "land",
            StageId::Advisor => "advisor",
            StageId::Question => "question",
        }
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self.kind {
            StageKind::Main => "main",
            StageKind::Side => "side",
        }
    }
}

#[cfg(test)]
mod tests;
