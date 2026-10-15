use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryStep {
    Admit,
    Land,
    Commit,
    Wait,
    Landed,
    Retry,
    Review,
    Cancel,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryOutcome {
    Admitted,
    Unlanded,
    Waiting,
    Landed,
    Failed,
    Rereview,
    Cancelled,
}

impl DeliveryStep {
    fn outcomes(self) -> &'static [DeliveryOutcome] {
        use DeliveryOutcome::{Admitted, Cancelled, Failed, Landed, Rereview, Unlanded, Waiting};
        match self {
            Self::Admit => &[Admitted],
            Self::Land => &[Unlanded, Waiting, Landed, Failed, Rereview, Cancelled],
            _ => &[],
        }
    }

    fn accepts(self, outcome: DeliveryOutcome) -> bool {
        matches!(
            (self, outcome),
            (Self::Land, DeliveryOutcome::Admitted)
                | (Self::Commit, DeliveryOutcome::Unlanded)
                | (Self::Wait, DeliveryOutcome::Waiting)
                | (Self::Landed, DeliveryOutcome::Landed)
                | (Self::Retry, DeliveryOutcome::Failed)
                | (Self::Review, DeliveryOutcome::Rereview)
                | (Self::Cancel, DeliveryOutcome::Cancelled)
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transition {
    from: DeliveryStep,
    on: DeliveryOutcome,
    to: DeliveryStep,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeliveryFlow {
    entry: DeliveryStep,
    transitions: Vec<Transition>,
}

impl DeliveryFlow {
    pub(crate) fn compiled() -> &'static Self {
        static FLOW: OnceLock<DeliveryFlow> = OnceLock::new();
        FLOW.get_or_init(|| {
            Self::parse(include_str!("../../../pipelines/delivery.yaml"))
                .expect("pipelines/delivery.yaml is valid")
        })
    }

    fn parse(text: &str) -> Result<Self, String> {
        let flow: Self = super::decode(text)?;
        flow.validate()?;
        Ok(flow)
    }

    fn validate(&self) -> Result<(), String> {
        if self.entry != DeliveryStep::Admit {
            return Err("delivery must begin with admission".into());
        }
        let mut keys = BTreeSet::new();
        for edge in &self.transitions {
            if !edge.from.outcomes().contains(&edge.on) {
                return Err(format!("{:?} cannot produce {:?}", edge.from, edge.on));
            }
            if !edge.to.accepts(edge.on) {
                return Err(format!("{:?} cannot receive {:?}", edge.to, edge.on));
            }
            if !keys.insert((edge.from, edge.on)) {
                return Err(format!(
                    "duplicate delivery transition {:?}/{:?}",
                    edge.from, edge.on
                ));
            }
        }
        let mut reached = BTreeSet::from([self.entry]);
        let mut pending = vec![self.entry];
        while let Some(step) = pending.pop() {
            for &on in step.outcomes() {
                let edge = self
                    .transitions
                    .iter()
                    .find(|edge| edge.from == step && edge.on == on)
                    .ok_or_else(|| format!("missing delivery transition {step:?}/{on:?}"))?;
                if reached.insert(edge.to) {
                    pending.push(edge.to);
                }
            }
        }
        if self
            .transitions
            .iter()
            .any(|edge| !reached.contains(&edge.from))
        {
            return Err("unreachable delivery transition".into());
        }
        Ok(())
    }

    pub(crate) fn entry(&self) -> DeliveryStep {
        self.entry
    }

    pub(crate) fn next(&self, from: DeliveryStep, on: DeliveryOutcome) -> DeliveryStep {
        self.transitions
            .iter()
            .find(|edge| edge.from == from && edge.on == on)
            .expect("the delivery operation produced a declared outcome")
            .to
    }
}

#[cfg(test)]
#[path = "delivery/tests.rs"]
mod tests;
