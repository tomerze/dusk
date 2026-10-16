use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::Notify;

pub const REFRESH: Duration = Duration::from_secs(30);
pub const RESOLVE_SPACING: Duration = Duration::from_secs(1);

pub struct Fleet {
    name: String,
    addresses: RwLock<Arc<[SocketAddr]>>,
    stale: Notify,
}

impl Fleet {
    pub fn new(name: String) -> Arc<Fleet> {
        Arc::new(Fleet {
            name,
            addresses: RwLock::new(Arc::from([])),
            stale: Notify::new(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn address(&self, link: u64) -> Option<SocketAddr> {
        let addresses = self
            .addresses
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let count = u64::try_from(addresses.len())
            .ok()
            .filter(|count| *count > 0)?;
        addresses.get(usize::try_from(link % count).ok()?).copied()
    }

    pub fn connect_failed(&self) {
        self.stale.notify_one();
    }

    pub async fn resolve(&self) {
        match tokio::net::lookup_host(self.name.as_str()).await {
            Ok(found) => {
                let mut found: Vec<SocketAddr> = found.collect();
                found.sort();
                found.dedup();
                if found.is_empty() {
                    tracing::warn!(fleet = %self.name, "nightfall's fleet name resolved to no address");
                    return;
                }
                let mut addresses = self
                    .addresses
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if addresses[..] != found[..] {
                    tracing::info!(fleet = %self.name, addresses = ?found, "resolved nightfall's fleet name");
                    *addresses = Arc::from(found);
                }
            }
            Err(error) => {
                tracing::warn!(fleet = %self.name, %error, "couldn't resolve nightfall's fleet name");
            }
        }
    }

    pub async fn keep_resolved(self: Arc<Fleet>) {
        loop {
            tokio::select! {
                () = tokio::time::sleep(REFRESH) => {}
                () = self.stale.notified() => {}
            }
            self.resolve().await;
            tokio::time::sleep(RESOLVE_SPACING).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn spreads_links_over_every_address_it_resolved() {
        let fleet = Fleet::new("127.0.0.1:8443".to_string());
        assert_eq!(fleet.address(0), None);
        fleet.resolve().await;
        let address: SocketAddr = "127.0.0.1:8443".parse().unwrap();
        assert_eq!(fleet.address(0), Some(address));
        assert_eq!(fleet.address(7), Some(address));
        *fleet.addresses.write().unwrap() = Arc::from([
            "10.0.0.1:443".parse().unwrap(),
            "10.0.0.2:443".parse().unwrap(),
        ]);
        assert_eq!(fleet.address(4), Some("10.0.0.1:443".parse().unwrap()));
        assert_eq!(fleet.address(5), Some("10.0.0.2:443".parse().unwrap()));
    }
}
