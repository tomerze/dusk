#[allow(dead_code)]
mod support;

use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dusk_base::dusk_program_kvs::KvsConfig;
use nightfall_provisioning::identity::DeviceIdKey;
use serde_json::Value;
use support::harness::{
    Environment, FLEET_NAME, PROVISION_NAME, run, shutdown, wait_for_session, wait_until,
};
use support::keys::hex_key;

const IMAGE: &str = "debian:trixie-slim";
const READY_TIMEOUT: Duration = Duration::from_secs(600);
const UNAVAILABLE: &str = "75";
const SCRIPT: &str = r#"set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null && apt-get install -y -qq --no-install-recommends swtpm swtpm-tools tpm2-tools >/dev/null || exit 75
mkdir -p /var/lib/tpm
swtpm_setup --tpm2 --tpmstate /var/lib/tpm --create-ek-cert --create-platform-cert --lock-nvram >/dev/null
swtpm socket --tpm2 --tpmstate dir=/var/lib/tpm --server type=tcp,port=2321 --ctrl type=tcp,port=2322 --flags not-need-init,startup-clear &
export TPM2TOOLS_TCTI=swtpm:host=127.0.0.1,port=2321
for attempt in $(seq 100); do tpm2_getcap properties-fixed >/dev/null 2>&1 && break; sleep 0.1; done
sed '/-----/d' /var/lib/swtpm-localca/issuercert.pem | base64 -d > /tpm/issuer.der
tpm2_nvdefine 0x01c00100 -C o -s "$(stat -c %s /tpm/issuer.der)" -a "ownerread|ownerwrite|authread|authwrite" >/dev/null
tpm2_nvwrite 0x01c00100 -C o -i /tpm/issuer.der
tpm2_readpublic -c 0x81010001 -o /tpm/endorsement-key.public >/dev/null
swtpm_ioctl -s --tcp 127.0.0.1:2322
wait
cp /var/lib/swtpm-localca/swtpm-localca-rootca-cert.pem /tpm/root.pem
chown "$SOCKET_OWNER" /tpm/issuer.der /tpm/root.pem /tpm/endorsement-key.public
while :; do
  rm -f /tpm/socket
  swtpm socket --tpm2 --tpmstate dir=/var/lib/tpm --server type=unixio,path=/tpm/socket,mode=0600,uid="${SOCKET_OWNER%:*}",gid="${SOCKET_OWNER#*:}" --ctrl type=unixio,path=/run/swtpm.ctrl --flags not-need-init,startup-clear || exit 1
done
"#;

fn skip(reason: &str) {
    if let Err(error) = writeln!(
        std::io::stderr(),
        "skipping {}: {reason}",
        std::thread::current()
            .name()
            .unwrap_or("a test that needs swtpm")
    ) {
        eprintln!("couldn't say why a test is skipped: {error}");
    }
}

fn docker(arguments: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("docker").args(arguments).output()
}

struct Swtpm {
    container: String,
    directory: PathBuf,
}

impl Swtpm {
    fn start() -> Option<Swtpm> {
        match docker(&["info", "--format", "{{.ServerVersion}}"]) {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                skip(&format!(
                    "Docker is unavailable: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
                return None;
            }
            Err(error) => {
                skip(&format!("Docker is unavailable: {error}"));
                return None;
            }
        }
        if !docker(&["image", "inspect", IMAGE]).is_ok_and(|output| output.status.success()) {
            let pull = docker(&["pull", IMAGE]).unwrap();
            if !pull.status.success() {
                skip(&format!(
                    "Docker could not pull {IMAGE}: {}",
                    String::from_utf8_lossy(&pull.stderr).trim()
                ));
                return None;
            }
        }
        let container = format!(
            "nightfall-swtpm-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let directory = std::env::temp_dir().join(&container);
        std::fs::create_dir_all(&directory).unwrap();
        let metadata = std::fs::metadata(&directory).unwrap();
        let processors = std::thread::available_parallelism().map_or(1, |count| count.get());
        let run = docker(&[
            "run",
            "--detach",
            "--name",
            &container,
            "--cpuset-cpus",
            &format!("0-{}", processors.min(10) - 1),
            "--cpu-shares",
            "2",
            "--volume",
            &format!("{}:/tpm", directory.display()),
            "--env",
            &format!("SOCKET_OWNER={}:{}", metadata.uid(), metadata.gid()),
            IMAGE,
            "sh",
            "-c",
            SCRIPT,
        ])
        .unwrap();
        let swtpm = Swtpm {
            container,
            directory,
        };
        assert!(
            run.status.success(),
            "Docker could not run {IMAGE}: {}",
            String::from_utf8_lossy(&run.stderr).trim()
        );
        let deadline = Instant::now() + READY_TIMEOUT;
        while !swtpm.socket().exists() {
            if let Some(exit_code) = swtpm.exit_code() {
                if exit_code == UNAVAILABLE {
                    skip(&format!(
                        "swtpm could not be installed in {IMAGE}:\n{}",
                        swtpm.logs()
                    ));
                    return None;
                }
                panic!(
                    "the swtpm container exited with {exit_code} before its TPM was ready:\n{}",
                    swtpm.logs()
                );
            }
            assert!(
                Instant::now() < deadline,
                "swtpm was not ready within {READY_TIMEOUT:?}:\n{}",
                swtpm.logs()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
        Some(swtpm)
    }

    fn socket(&self) -> PathBuf {
        self.directory.join("socket")
    }

    fn root(&self) -> PathBuf {
        self.directory.join("root.pem")
    }

    fn endorsement_key(&self) -> Vec<u8> {
        let public = std::fs::read(self.directory.join("endorsement-key.public")).unwrap();
        let (size, area) = public.split_first_chunk::<2>().unwrap();
        assert_eq!(usize::from(u16::from_be_bytes(*size)), area.len());
        area.to_vec()
    }

    fn exit_code(&self) -> Option<String> {
        let inspect = docker(&[
            "inspect",
            "--format",
            "{{.State.Running}} {{.State.ExitCode}}",
            &self.container,
        ])
        .unwrap();
        let state = String::from_utf8_lossy(&inspect.stdout).trim().to_string();
        state.strip_prefix("false ").map(String::from)
    }

    fn logs(&self) -> String {
        let logs = docker(&["logs", "--tail", "40", &self.container]).unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&logs.stdout),
            String::from_utf8_lossy(&logs.stderr)
        )
    }
}

impl Drop for Swtpm {
    fn drop(&mut self) {
        match docker(&["rm", "--force", &self.container]) {
            Ok(output) if output.status.success() => {}
            Ok(output) => eprintln!(
                "couldn't remove the container {}: {}",
                self.container,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(error) => eprintln!("couldn't remove the container {}: {error}", self.container),
        }
        if let Err(error) = std::fs::remove_dir_all(&self.directory) {
            eprintln!("couldn't remove {}: {error}", self.directory.display());
        }
    }
}

fn start_node(script: String, kvs_file: &Path) {
    dusk_base::link_anchors();
    let persistent = kvs_file.to_str().unwrap().to_string();
    std::thread::spawn(move || {
        let disconnected: dusk_capnp::dusk_capnp::dusk::Client =
            capnp_rpc::new_future_client(async {
                Err(capnp::Error::disconnected(String::from(
                    "the test compiles its init script without a node",
                )))
            });
        let init_script = futures::executor::block_on(
            dusk_base::dusk_program_sh::compile_to_words(disconnected, &script),
        )
        .expect("compile the init script");
        let init = dusk_base::dusk_program_init::Args::new(&init_script)
            .expect("build the init args")
            .as_program_args()
            .expect("build the init program args");
        dusk_nix::run(
            dusk_base::dusk_program::handle::new_handle(),
            move || {
                dusk_base::launcher_set(KvsConfig {
                    persistent: Some(persistent.clone()),
                })
            },
            init,
        )
    });
}

fn enrollments(environment: &Environment) -> Vec<Value> {
    environment
        .broker
        .records("dusk.enrollments")
        .into_iter()
        .filter_map(|record| record.payload)
        .map(|payload| serde_json::from_slice(&payload).unwrap())
        .collect()
}

#[test]
fn a_node_enrolls_with_its_tpm_and_renews_with_the_same_key() {
    let Some(tpm) = Swtpm::start() else {
        return;
    };
    let environment = Environment::new();
    let paths = environment.paths();
    std::fs::write(
        paths.file("fleet-tokens.toml"),
        format!(
            "[[token]]\nname = \"tpm\"\nvalue_sha256 = \"{}\"\n",
            nightfall_ledger::entry::sha256_hex(dusk_core::fleet_token::fleet_token().as_bytes())
        ),
    )
    .unwrap();
    let step_ca = support::step_ca::FakeStepCa::start(
        environment.provisioner.public_jwk(),
        environment.pki.fleet_client.clone(),
    );
    std::fs::write(paths.file("step-ca-root.crt"), &step_ca.root_pem).unwrap();
    let mut config = environment.config("nightfall-0", 0);
    config.step_ca.url = step_ca.url.clone();
    config.step_ca.certificate_lifetime = String::from("30s");
    config.provision.tpm_endorsement_roots = tpm.root();
    let (instance, _log) = environment.start(config);
    let address = instance.addresses.fleet;
    start_node(
        format!(
            "nightfall -c {address} --server-name {FLEET_NAME} --provision {address} --provision-server-name {PROVISION_NAME} --ca {} --tpm {}",
            paths.file("fleet-server-ca.crt").display(),
            tpm.socket().display(),
        ),
        &paths.file("node.kvs"),
    );
    run(async {
        wait_until("a renewal", Duration::from_secs(120), || {
            enrollments(&environment)
                .iter()
                .any(|event| event["operation"] == "renew" && event["outcome"] != "rate_limited")
        })
        .await;
        wait_for_session(&instance).await;
    });
    shutdown(instance);

    let events = enrollments(&environment);
    let outcomes: Vec<(&str, &str)> = events
        .iter()
        .map(|event| {
            (
                event["operation"].as_str().unwrap(),
                event["outcome"].as_str().unwrap(),
            )
        })
        .filter(|(_, outcome)| *outcome != "rate_limited")
        .collect();
    assert_eq!(
        outcomes[..3],
        [
            ("assign", "assigned"),
            ("enroll", "issued"),
            ("renew", "issued")
        ],
        "{events:#?}"
    );
    let enrolled = &events
        .iter()
        .find(|event| event["operation"] == "enroll")
        .unwrap();
    assert_eq!(enrolled["credential_kind"], "fleet_token");
    let signed = step_ca.signed.lock().unwrap().clone();
    assert!(signed.len() >= 2, "{signed:#?}");
    assert!(
        signed.iter().all(|claims| claims["attestation"] == "tpm"),
        "{signed:#?}"
    );
    let fingerprint = nightfall_provisioning::credential::sha256(&tpm.endorsement_key());
    let device_id = DeviceIdKey::from_hex(&hex_key(0x3c))
        .unwrap()
        .device_id(&fingerprint);
    assert_eq!(enrolled["device_id"], device_id.as_str());
    assert_eq!(
        enrolled["hardware_fingerprint_hash"],
        nightfall_ledger::entry::sha256_hex(&fingerprint).as_str()
    );
    let connections = environment.connections();
    assert!(
        connections
            .iter()
            .any(|event| event["device_id"] == device_id.as_str()),
        "{connections:#?}"
    );
}
