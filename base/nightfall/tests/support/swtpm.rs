use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const IMAGE: &str = "debian:trixie-slim";
const READY_TIMEOUT: Duration = Duration::from_secs(600);
const RESET_TIMEOUT: Duration = Duration::from_secs(30);
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
swtpm_ioctl -s --tcp 127.0.0.1:2322
wait
chown "$SOCKET_OWNER" /tpm/issuer.der
while :; do
  rm -f /tpm/socket
  swtpm socket --tpm2 --tpmstate dir=/var/lib/tpm --server type=unixio,path=/tpm/socket,mode=0600,uid="${SOCKET_OWNER%:*}",gid="${SOCKET_OWNER#*:}" --ctrl type=unixio,path=/run/swtpm.ctrl --flags not-need-init,startup-clear || exit 1
done
"#;

fn skip(reason: &str) {
    let test = std::thread::current();
    if let Err(error) = writeln!(
        std::io::stderr(),
        "skipping {}: {reason}",
        test.name().unwrap_or("a test that needs swtpm")
    ) {
        eprintln!("couldn't say why a test is skipped: {error}");
    }
}

fn docker(arguments: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new("docker").args(arguments).output()
}

pub(crate) struct Swtpm {
    container: String,
    directory: PathBuf,
}

impl Swtpm {
    pub(crate) fn start() -> Option<Swtpm> {
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
        let image_present =
            docker(&["image", "inspect", IMAGE]).is_ok_and(|output| output.status.success());
        if !image_present {
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
            "dusk-nightfall-swtpm-{}-{}",
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

    pub(crate) fn socket(&self) -> PathBuf {
        self.directory.join("socket")
    }

    pub(crate) fn issuer_certificate(&self) -> Vec<u8> {
        std::fs::read(self.directory.join("issuer.der")).unwrap()
    }

    pub(crate) fn reset(&self) {
        let before = std::fs::metadata(self.socket()).unwrap().ino();
        let shutdown = docker(&[
            "exec",
            &self.container,
            "swtpm_ioctl",
            "-s",
            "--unix",
            "/run/swtpm.ctrl",
        ])
        .unwrap();
        assert!(
            shutdown.status.success(),
            "couldn't stop swtpm: {}",
            String::from_utf8_lossy(&shutdown.stderr)
        );
        let deadline = Instant::now() + RESET_TIMEOUT;
        loop {
            let replaced =
                std::fs::metadata(self.socket()).is_ok_and(|metadata| metadata.ino() != before);
            if replaced && std::os::unix::net::UnixStream::connect(self.socket()).is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "swtpm did not come back within {RESET_TIMEOUT:?}:\n{}",
                self.logs()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
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
