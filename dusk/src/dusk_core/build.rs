use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const UNSET: &str = "or unset it and the build makes one up";

fn main() {
    println!("cargo:rerun-if-env-changed=DUSK_FLEET_TOKEN");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let token = match std::env::var("DUSK_FLEET_TOKEN") {
        Ok(token) if token.is_empty() => fail(&format!(
            "DUSK_FLEET_TOKEN is set but empty. Set it to the fleet token, {UNSET}"
        )),
        Ok(token) => token,
        Err(std::env::VarError::NotUnicode(_)) => fail(&format!(
            "DUSK_FLEET_TOKEN is not valid UTF-8. Set it to the fleet token as UTF-8 text, {UNSET}"
        )),
        Err(std::env::VarError::NotPresent) => random_token(&out_dir.join("random_fleet_token")),
    };
    write(&out_dir.join("fleet_token"), &token);
}

fn random_token(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(token) if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            return token;
        }
        Ok(_) => fail(&format!(
            "{} does not hold a token this build made: remove it to make a new one, or set \
             DUSK_FLEET_TOKEN",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => fail(&format!(
            "couldn't read the fleet token this build made, {}: {error}. Set DUSK_FLEET_TOKEN, \
             or make the file readable",
            path.display()
        )),
    }
    let mut bytes = [0u8; 32];
    if let Err(error) = getrandom::fill(&mut bytes) {
        fail(&format!(
            "the build machine's random source failed: {error}. Set DUSK_FLEET_TOKEN to the fleet token"
        ));
    }
    let token = bytes.iter().fold(String::new(), |mut token, byte| {
        write!(token, "{byte:02x}").expect("writing to a String does not fail");
        token
    });
    let temporary = path.with_extension("new");
    write(&temporary, &token);
    if let Err(error) = std::fs::rename(&temporary, path) {
        fail(&format!(
            "couldn't move {} to {}: {error}",
            temporary.display(),
            path.display()
        ));
    }
    token
}

fn write(path: &Path, contents: &str) {
    if let Err(error) = std::fs::write(path, contents) {
        fail(&format!("couldn't write {}: {error}", path.display()));
    }
}

fn fail(reason: &str) -> ! {
    eprintln!("error: {reason}");
    std::process::exit(1)
}
