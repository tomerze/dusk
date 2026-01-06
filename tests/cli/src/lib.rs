#![allow(unused_imports)]

use assert_cmd::assert::OutputAssertExt;
use dusk_tests::{gen_port, get_dusk_cli_bin, DuskNixImpl, LISTEN_ADDR};
use predicates::prelude::*;
use std::process::Command;

#[test]
fn test_run_ps() {
    let port = gen_port();
    let _dusk = DuskNixImpl::new(LISTEN_ADDR, port);

    let bin_path = get_dusk_cli_bin();

    let mut cmd = Command::new(bin_path);
    cmd.arg(format!("{}:{}", LISTEN_ADDR, port))
        .arg("ps")
        .assert()
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("ps"))
        .stdout(predicate::str::contains("sh"))
        .stdout(predicate::str::contains("program_id"));
}
