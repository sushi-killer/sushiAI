use std::path::PathBuf;
pub(crate) struct PrContent {
    pub(crate) branch: String,
    pub(crate) title: String,
    pub(crate) body: String,
}
pub(crate) struct Publication {
    pub(crate) content: PrContent,
    pub(crate) repo: PathBuf,
    pub(crate) base: String,
    pub(crate) local: String,
}
pub(crate) struct PublishedPr {
    pub(crate) url: String,
    pub(crate) verb: &'static str,
    pub(crate) head: String,
}

/// A validated IO preparation that can dispatch its write without another await.
pub(crate) trait PreparedWrite {
    type Running;
    fn dispatch(self) -> Result<Self::Running, String>;
}

pub(crate) mod open;
pub(crate) mod push;
