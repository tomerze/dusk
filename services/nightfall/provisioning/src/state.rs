#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Enrolled,
    Active,
    Quarantined,
    Retired,
    Revoked,
}

impl Lifecycle {
    pub fn name(self) -> &'static str {
        match self {
            Lifecycle::Enrolled => "enrolled",
            Lifecycle::Active => "active",
            Lifecycle::Quarantined => "quarantined",
            Lifecycle::Retired => "retired",
            Lifecycle::Revoked => "revoked",
        }
    }

    pub fn blocks_certificates(self) -> bool {
        matches!(self, Lifecycle::Retired | Lifecycle::Revoked)
    }
}

pub trait NodeStateView: Send + Sync {
    fn device(&self, device_id: &str) -> Option<Lifecycle>;
    fn installation(&self, device_id: &str, installation_id: &str) -> Option<Lifecycle>;
}
