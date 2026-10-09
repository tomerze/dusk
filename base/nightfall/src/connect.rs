use std::prelude::rust_2024::*;

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use dusk_program::embassy_futures::select::{Either3, select3};
use dusk_program::embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use dusk_program::embassy_sync::mutex::Mutex;
use dusk_program::embassy_time::{self, Instant, Timer};
use dusk_program::namespace::Namespace;
use dusk_program::value::Value;
use dusk_program_kvs_internal::{Kvs, key_id};
use ring::rand::SecureRandom as _;
use rustls::pki_types::ServerName;

use crate::backoff::{Backoff, uniform_up_to};
use crate::identity::{self, Identity};
use crate::link::{self, LinkGuard};
use crate::provisioning::{DeviceReport, Provisioner, ProvisioningError, Token};
use crate::tls::{self, HostPort, embassy_duration};

pub const DEFAULT_HEARTBEAT_TIMEOUT_SECONDS: u32 = 90;
const INITIAL_DELAY_CEILING: Duration = Duration::from_secs(5);
const LINK_BACKOFF_BASE: Duration = Duration::from_secs(1);
const LINK_BACKOFF_CAP: Duration = Duration::from_secs(300);
const LINK_STABLE_AFTER: Duration = Duration::from_secs(60);
const PROVISIONING_BACKOFF_BASE: Duration = Duration::from_secs(1);
const PROVISIONING_BACKOFF_CAP: Duration = Duration::from_secs(600);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(10);
const LONGEST_WALL_CLOCK_SLEEP: Duration = Duration::from_secs(3600);
const MAXIMUM_TOKEN_BYTES: usize = 64 * 1024;

const VERSION_KEY: u64 = key_id("dusk.version");
const IMPL_KEY: u64 = key_id("dusk.impl");
const TARGET_OS_KEY: u64 = key_id("dusk.target.os");
const TARGET_ARCH_KEY: u64 = key_id("dusk.target.arch");

static PROVISIONING: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectArgs {
    pub fleet: String,
    pub fleet_server_name: Option<String>,
    pub provision: String,
    pub provision_server_name: Option<String>,
    pub trust_anchors: String,
    pub install_token_file: Option<String>,
    pub heartbeat_timeout_seconds: u32,
}

pub(crate) struct Settings {
    pub fleet: HostPort,
    pub fleet_server_name: ServerName<'static>,
    pub provisioner: Provisioner,
    pub trust_anchors: String,
    pub install_token_file: Option<String>,
    pub heartbeat_timeout: Duration,
}

impl Settings {
    pub fn new(arguments: &ConnectArgs) -> Result<Settings, String> {
        let fleet = HostPort::parse(&arguments.fleet)?;
        let provision = HostPort::parse(&arguments.provision)?;
        let fleet_server_name = tls::server_name(
            arguments
                .fleet_server_name
                .as_deref()
                .unwrap_or(&fleet.host),
        )?;
        let provision_server_name = tls::server_name(
            arguments
                .provision_server_name
                .as_deref()
                .unwrap_or(&provision.host),
        )?;
        if arguments.trust_anchors.is_empty() {
            return Err(String::from("no trust anchors file was given"));
        }
        if arguments
            .install_token_file
            .as_deref()
            .is_some_and(str::is_empty)
        {
            return Err(String::from("the install token file is an empty path"));
        }
        if arguments.heartbeat_timeout_seconds == 0 {
            return Err(String::from(
                "the heartbeat timeout must be at least 1 second",
            ));
        }
        Ok(Settings {
            fleet,
            fleet_server_name,
            provisioner: Provisioner {
                target: provision,
                server_name: provision_server_name,
                trust_anchors: arguments.trust_anchors.clone(),
            },
            trust_anchors: arguments.trust_anchors.clone(),
            install_token_file: arguments.install_token_file.clone(),
            heartbeat_timeout: Duration::from_secs(u64::from(arguments.heartbeat_timeout_seconds)),
        })
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    match ring::rand::SystemRandom::new().fill(&mut bytes) {
        Ok(()) => u64::from_le_bytes(bytes),
        Err(_) => {
            tracing::warn!("the system random number generator failed; jittering with the clock");
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos().into())
                .unwrap_or(0)
        }
    }
}

async fn sleep(duration: Duration) {
    Timer::after(embassy_duration(duration)).await;
}

async fn sleep_until_unix_ms(deadline: u64) {
    loop {
        let now = now_unix_ms();
        if now >= deadline {
            return;
        }
        sleep(Duration::from_millis(deadline - now).min(LONGEST_WALL_CLOCK_SLEEP)).await;
    }
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) async fn run(settings: Settings, namespace: Rc<Namespace>, kvs: Arc<Kvs>) {
    let connector = Connector {
        settings,
        namespace,
        kvs,
    };
    connector.supervise().await;
}

struct Connector {
    settings: Settings,
    namespace: Rc<Namespace>,
    kvs: Arc<Kvs>,
}

type PendingRenewal<'connector> =
    Pin<Box<dyn Future<Output = Result<Identity, ProvisioningError>> + 'connector>>;

enum LinkEnd {
    Renewed,
    Lost { heard_for: Duration },
}

impl LinkEnd {
    fn was_stable(&self) -> bool {
        match self {
            LinkEnd::Renewed => true,
            LinkEnd::Lost { heard_for } => *heard_for > LINK_STABLE_AFTER,
        }
    }
}

enum Event {
    Released,
    Heartbeat,
    RenewalDue,
    Renewal(Result<Identity, ProvisioningError>),
}

impl Connector {
    async fn supervise(&self) {
        let delay = uniform_up_to(INITIAL_DELAY_CEILING, random_u64());
        tracing::info!(
            fleet = %self.settings.fleet,
            delay_ms = milliseconds(delay),
            "linking to the fleet after a random delay"
        );
        sleep(delay).await;
        let mut link_backoff = Backoff::new(LINK_BACKOFF_BASE, LINK_BACKOFF_CAP);
        let mut provisioning_backoff =
            Backoff::new(PROVISIONING_BACKOFF_BASE, PROVISIONING_BACKOFF_CAP);
        let mut renewal_backoff = Backoff::new(PROVISIONING_BACKOFF_BASE, PROVISIONING_BACKOFF_CAP);
        let mut drawn: Option<(String, u64)> = None;
        loop {
            let identity = self.usable_identity(&mut provisioning_backoff).await;
            let mut renew_at = match &drawn {
                Some((fingerprint, renew_at)) if *fingerprint == identity.leaf.fingerprint => {
                    *renew_at
                }
                _ => {
                    let renew_at = identity::renew_point(
                        identity.leaf.not_before_unix_ms,
                        identity.leaf.not_after_unix_ms,
                        random_u64(),
                    );
                    tracing::info!(
                        device_id = identity.device_id(),
                        installation_id = identity.installation_id(),
                        certificate_fingerprint = identity.leaf.fingerprint.as_str(),
                        not_after_unix_ms = identity.leaf.not_after_unix_ms,
                        renew_at_unix_ms = renew_at,
                        "drew the certificate's renewal point"
                    );
                    renew_at
                }
            };
            if now_unix_ms() >= renew_at {
                let outcome = self.renew(&identity).await;
                if self.renewal_finished(outcome, &mut renew_at, &mut renewal_backoff) {
                    continue;
                }
            }
            let linked = self
                .link(&identity, &mut renew_at, &mut renewal_backoff)
                .await;
            drawn = Some((identity.leaf.fingerprint.clone(), renew_at));
            match linked {
                Ok(end) => {
                    if end.was_stable() {
                        link_backoff.reset();
                    }
                    if let LinkEnd::Lost { .. } = end {
                        self.pause(&mut link_backoff).await;
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        fleet = %self.settings.fleet,
                        device_id = identity.device_id(),
                        installation_id = identity.installation_id(),
                        error = %format_args!("{error:#}"),
                        "couldn't link to the fleet"
                    );
                    self.pause(&mut link_backoff).await;
                }
            }
        }
    }

    async fn pause(&self, backoff: &mut Backoff) {
        let delay = backoff.next_delay(random_u64());
        tracing::info!(
            delay_ms = milliseconds(delay),
            "linking to the fleet again after a backoff"
        );
        sleep(delay).await;
    }

    async fn link(
        &self,
        identity: &Identity,
        renew_at: &mut u64,
        renewal_backoff: &mut Backoff,
    ) -> anyhow::Result<LinkEnd> {
        let roots = tls::trust_anchors(&self.settings.trust_anchors).await?;
        let config = tls::client_config(
            roots,
            &[&rustls::version::TLS13],
            Some(identity.certified_key()),
        )?;
        let connected = tls::connect_tls(
            &self.settings.fleet,
            self.settings.fleet_server_name.clone(),
            config,
        )
        .await?;
        let peer = connected.peer;
        let (link, (reader, writer)) = link::watch(connected.stream, Some(connected.socket));
        let guard = LinkGuard(link.clone());
        let task_id = Rc::new(Cell::new(0));
        let session_task = dusk_core::session(
            task_id.clone(),
            self.namespace.clone(),
            Box::pin(reader),
            Box::pin(writer),
        )
        .map_err(|error| {
            anyhow::anyhow!("couldn't serve the link: every session slot is in use ({error:?})")
        })?;
        task_id.set(session_task.id());
        self.namespace.spawner.spawn(session_task);
        let linked_at = Instant::now();
        tracing::info!(
            fleet = %self.settings.fleet,
            peer = %peer,
            device_id = identity.device_id(),
            installation_id = identity.installation_id(),
            namespace_id = self.namespace.id,
            certificate_fingerprint = identity.leaf.fingerprint.as_str(),
            "linked to the fleet"
        );

        let heartbeat_timeout = embassy_duration(self.settings.heartbeat_timeout);
        let mut renewal: Option<PendingRenewal<'_>> = None;
        let (mut end, reason) = loop {
            let heartbeat_deadline = link.last_read() + heartbeat_timeout;
            let event = {
                let released = async {
                    link.released().await;
                    Event::Released
                };
                let heartbeat = async {
                    Timer::at(heartbeat_deadline).await;
                    Event::Heartbeat
                };
                let next_renewal = *renew_at;
                let renewing = async {
                    match renewal.as_mut() {
                        Some(future) => Event::Renewal(future.as_mut().await),
                        None => {
                            sleep_until_unix_ms(next_renewal).await;
                            Event::RenewalDue
                        }
                    }
                };
                match select3(released, heartbeat, renewing).await {
                    Either3::First(event) | Either3::Second(event) | Either3::Third(event) => event,
                }
            };
            match event {
                Event::Released => break (None, "the session ended"),
                Event::Heartbeat => {
                    if Instant::now() >= link.last_read() + heartbeat_timeout {
                        tracing::warn!(
                            device_id = identity.device_id(),
                            installation_id = identity.installation_id(),
                            heartbeat_timeout_seconds = self.settings.heartbeat_timeout.as_secs(),
                            "nothing arrived from the fleet within the heartbeat timeout; closing the link"
                        );
                        break (None, "heartbeat timeout");
                    }
                }
                Event::RenewalDue => renewal = Some(Box::pin(self.renew(identity))),
                Event::Renewal(outcome) => {
                    renewal = None;
                    if self.renewal_finished(outcome, renew_at, renewal_backoff) {
                        break (Some(LinkEnd::Renewed), "renewed the certificate");
                    }
                }
            }
        };
        link.close();
        if let Some(pending) = renewal.take() {
            tracing::info!("finishing the certificate renewal the link was waiting on");
            if self.renewal_finished(pending.await, renew_at, renewal_backoff) {
                end = Some(LinkEnd::Renewed);
            }
        }
        if embassy_time::with_timeout(embassy_duration(RELEASE_TIMEOUT), link.released())
            .await
            .is_err()
        {
            tracing::warn!(
                timeout_seconds = RELEASE_TIMEOUT.as_secs(),
                "the closed link's session has not ended; linking again anyway"
            );
        }
        drop(guard);
        let lived = Duration::from_micros(linked_at.elapsed().as_micros());
        let heard_for = Duration::from_micros(
            link.last_read()
                .saturating_duration_since(linked_at)
                .as_micros(),
        );
        tracing::info!(
            fleet = %self.settings.fleet,
            device_id = identity.device_id(),
            installation_id = identity.installation_id(),
            lived_ms = milliseconds(lived),
            heard_for_ms = milliseconds(heard_for),
            reason,
            "the fleet link is down"
        );
        Ok(end.unwrap_or(LinkEnd::Lost { heard_for }))
    }

    async fn renew(&self, identity: &Identity) -> Result<Identity, ProvisioningError> {
        let _provisioning = PROVISIONING.lock().await;
        if let Some(stored) = identity::load(&self.kvs).await?
            && stored.leaf.fingerprint != identity.leaf.fingerprint
        {
            tracing::info!(
                device_id = stored.device_id(),
                installation_id = stored.installation_id(),
                certificate_fingerprint = stored.leaf.fingerprint.as_str(),
                "another nightfall on this node replaced the certificate; using its certificate instead of renewing"
            );
            return Ok(stored);
        }
        tracing::info!(
            device_id = identity.device_id(),
            installation_id = identity.installation_id(),
            certificate_fingerprint = identity.leaf.fingerprint.as_str(),
            "renewing the certificate"
        );
        let renewed = self.settings.provisioner.renew(identity, &self.kvs).await?;
        tracing::info!(
            device_id = renewed.device_id(),
            installation_id = renewed.installation_id(),
            certificate_fingerprint = renewed.leaf.fingerprint.as_str(),
            not_after_unix_ms = renewed.leaf.not_after_unix_ms,
            "renewed the certificate"
        );
        Ok(renewed)
    }

    fn renewal_finished(
        &self,
        outcome: Result<Identity, ProvisioningError>,
        renew_at: &mut u64,
        backoff: &mut Backoff,
    ) -> bool {
        match outcome {
            Ok(_) => {
                backoff.reset();
                true
            }
            Err(error) => {
                let delay = self.retry_delay(&error, backoff);
                *renew_at = now_unix_ms().saturating_add(milliseconds(delay));
                self.report_provisioning_failure(&error, delay, "renewing the certificate failed");
                false
            }
        }
    }

    fn retry_delay(&self, error: &ProvisioningError, backoff: &mut Backoff) -> Duration {
        match error {
            ProvisioningError::Refused(_) => backoff.at_cap(random_u64()),
            ProvisioningError::Failed(_) => backoff.next_delay(random_u64()),
        }
    }

    fn report_provisioning_failure(
        &self,
        error: &ProvisioningError,
        delay: Duration,
        message: &str,
    ) {
        match error {
            ProvisioningError::Refused(reason) => tracing::error!(
                provision = %self.settings.provisioner.target,
                reason = reason.as_str(),
                retry_in_ms = milliseconds(delay),
                "{message}: nightfall refused"
            ),
            ProvisioningError::Failed(failure) => tracing::warn!(
                provision = %self.settings.provisioner.target,
                error = %format_args!("{failure:#}"),
                retry_in_ms = milliseconds(delay),
                "{message}"
            ),
        }
    }

    async fn usable_identity(&self, backoff: &mut Backoff) -> Identity {
        loop {
            match self.try_identity().await {
                Ok(identity) => {
                    backoff.reset();
                    return identity;
                }
                Err(error) => {
                    let delay = self.retry_delay(&error, backoff);
                    self.report_provisioning_failure(&error, delay, "provisioning failed");
                    sleep(delay).await;
                }
            }
        }
    }

    async fn try_identity(&self) -> Result<Identity, ProvisioningError> {
        let _provisioning = PROVISIONING.lock().await;
        let Some(stored) = identity::load(&self.kvs).await? else {
            return self.enroll().await;
        };
        if !stored.is_expired(now_unix_ms()) {
            return Ok(stored);
        }
        tracing::info!(
            device_id = stored.device_id(),
            installation_id = stored.installation_id(),
            not_after_unix_ms = stored.leaf.not_after_unix_ms,
            "the certificate has expired; renewing it"
        );
        match self.settings.provisioner.renew(&stored, &self.kvs).await {
            Ok(renewed) => {
                tracing::info!(
                    device_id = renewed.device_id(),
                    installation_id = renewed.installation_id(),
                    certificate_fingerprint = renewed.leaf.fingerprint.as_str(),
                    not_after_unix_ms = renewed.leaf.not_after_unix_ms,
                    "renewed the expired certificate"
                );
                Ok(renewed)
            }
            Err(error) if error.is_beyond_grace() => {
                tracing::warn!(
                    device_id = stored.device_id(),
                    installation_id = stored.installation_id(),
                    reason = %error,
                    "nightfall will not renew a certificate this long expired; enrolling a new installation"
                );
                self.enroll().await
            }
            Err(error) => Err(error),
        }
    }

    async fn enroll(&self) -> Result<Identity, ProvisioningError> {
        let token = self.token().await?;
        let report = self.device_report().await?;
        tracing::info!(
            provision = %self.settings.provisioner.target,
            installation_hint = report.installation_hint.as_str(),
            credential = token.kind(),
            "enrolling"
        );
        let identity = self
            .settings
            .provisioner
            .enroll(&token, &report, &self.kvs)
            .await?;
        tracing::info!(
            device_id = identity.device_id(),
            installation_id = identity.installation_id(),
            certificate_fingerprint = identity.leaf.fingerprint.as_str(),
            not_after_unix_ms = identity.leaf.not_after_unix_ms,
            "enrolled"
        );
        Ok(identity)
    }

    async fn token(&self) -> anyhow::Result<Token> {
        let Some(path) = &self.settings.install_token_file else {
            return Ok(Token::Fleet(String::from(
                dusk_core::fleet_token::fleet_token(),
            )));
        };
        let contents = crate::file::read(path, MAXIMUM_TOKEN_BYTES).await?;
        let token = core::str::from_utf8(&contents)
            .map_err(|_| anyhow::anyhow!("the install token file `{path}` is not UTF-8"))?
            .trim();
        anyhow::ensure!(
            !token.is_empty(),
            "the install token file `{path}` is empty"
        );
        Ok(Token::Install(token.to_string()))
    }

    async fn device_report(&self) -> anyhow::Result<DeviceReport> {
        let hardware_fingerprint = identity::hardware_fingerprint(&self.kvs).await?;
        let text = |value: Option<Value>| match value {
            Some(Value::String(text) | Value::Text(text)) => text,
            _ => String::new(),
        };
        let hostname = dusk_core::driver::hostname().unwrap_or_else(|error| {
            tracing::warn!(error = %format_args!("{error:#}"), "couldn't read the hostname for the device report");
            String::new()
        });
        Ok(DeviceReport {
            hardware_fingerprint: hardware_fingerprint.to_vec(),
            installation_hint: identity::installation_id(&self.kvs)
                .await
                .unwrap_or_default(),
            dusk_version: text(self.kvs.get(VERSION_KEY).await),
            impl_name: text(self.kvs.get(IMPL_KEY).await),
            target_os: text(self.kvs.get(TARGET_OS_KEY).await),
            target_arch: text(self.kvs.get(TARGET_ARCH_KEY).await),
            hostname,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments() -> ConnectArgs {
        ConnectArgs {
            fleet: String::from("fleet.example:443"),
            fleet_server_name: None,
            provision: String::from("provision.example:443"),
            provision_server_name: None,
            trust_anchors: String::from("/etc/dusk/fleet-server-ca.pem"),
            install_token_file: None,
            heartbeat_timeout_seconds: DEFAULT_HEARTBEAT_TIMEOUT_SECONDS,
        }
    }

    #[test]
    fn server_names_default_to_the_host() {
        let settings = Settings::new(&arguments()).unwrap();
        assert_eq!(
            settings.fleet_server_name,
            ServerName::try_from("fleet.example").unwrap()
        );
        assert_eq!(
            settings.provisioner.server_name,
            ServerName::try_from("provision.example").unwrap()
        );
        assert_eq!(settings.heartbeat_timeout, Duration::from_secs(90));
        let mut overridden = arguments();
        overridden.fleet = String::from("10.0.0.1:8443");
        overridden.fleet_server_name = Some(String::from("fleet.dusk.example"));
        let settings = Settings::new(&overridden).unwrap();
        assert_eq!(
            settings.fleet_server_name,
            ServerName::try_from("fleet.dusk.example").unwrap()
        );
    }

    #[test]
    fn settings_refuse_unusable_arguments() {
        let mutations: Vec<fn(&mut ConnectArgs)> = vec![
            |arguments| arguments.fleet = String::from("fleet.example"),
            |arguments| arguments.provision = String::from("provision.example:0"),
            |arguments| arguments.fleet_server_name = Some(String::from("not a name")),
            |arguments| arguments.trust_anchors = String::new(),
            |arguments| arguments.install_token_file = Some(String::new()),
            |arguments| arguments.heartbeat_timeout_seconds = 0,
        ];
        for mutation in mutations {
            let mut changed = arguments();
            mutation(&mut changed);
            assert!(Settings::new(&changed).is_err(), "{changed:?}");
        }
    }

    #[test]
    fn only_a_renewal_or_a_link_heard_from_for_over_a_minute_was_stable() {
        assert!(LinkEnd::Renewed.was_stable());
        assert!(
            LinkEnd::Lost {
                heard_for: LINK_STABLE_AFTER + Duration::from_millis(1)
            }
            .was_stable()
        );
        assert!(
            !LinkEnd::Lost {
                heard_for: LINK_STABLE_AFTER
            }
            .was_stable()
        );
        assert!(
            !LinkEnd::Lost {
                heard_for: Duration::ZERO
            }
            .was_stable()
        );
    }

    #[test]
    fn links_are_retried_after_up_to_a_second_doubling_to_five_minutes() {
        assert!(uniform_up_to(INITIAL_DELAY_CEILING, u64::MAX) <= Duration::from_secs(5));
        let mut backoff = Backoff::new(LINK_BACKOFF_BASE, LINK_BACKOFF_CAP);
        assert_eq!(backoff.ceiling(), Duration::from_secs(1));
        for _ in 0..20 {
            backoff.next_delay(0);
        }
        assert_eq!(backoff.ceiling(), Duration::from_secs(300));
        assert_eq!(LINK_STABLE_AFTER, Duration::from_secs(60));
    }

    #[test]
    fn provisioning_is_retried_after_up_to_a_second_doubling_to_ten_minutes() {
        let mut backoff = Backoff::new(PROVISIONING_BACKOFF_BASE, PROVISIONING_BACKOFF_CAP);
        assert_eq!(backoff.ceiling(), Duration::from_secs(1));
        for _ in 0..20 {
            backoff.next_delay(0);
        }
        assert_eq!(backoff.ceiling(), Duration::from_secs(600));
    }
}
