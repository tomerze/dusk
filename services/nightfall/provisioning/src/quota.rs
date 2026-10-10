use std::future::Future;
use std::pin::Pin;

use crate::credential::CredentialName;

pub const UNCAPPED: u64 = 9_007_199_254_740_992;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub id: String,
    pub credential: CredentialName,
    pub limit: u64,
    pub device_id: String,
    pub installation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct QuotaUnavailable(pub String);

pub type ReservationOutcome = Pin<Box<dyn Future<Output = Result<bool, QuotaUnavailable>>>>;

pub trait InstallationQuota: Send + Sync {
    fn used(&self, credential: &CredentialName) -> Result<u64, QuotaUnavailable>;
    fn reserve(&self, reservation: Reservation) -> ReservationOutcome;
    fn release(&self, reservation: &Reservation);
    fn record(&self, reservation: &Reservation);
}
