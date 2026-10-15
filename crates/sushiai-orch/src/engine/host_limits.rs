use crate::policy::settings::Settings;

impl Settings {
    pub fn heavy_check_limit(&self) -> u32 {
        self.heavy_checks.unwrap_or_else(|| {
            Self::auto_heavy_checks(
                std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            )
        })
    }
}
