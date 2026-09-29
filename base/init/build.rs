fn main() {
    for (variable, cargo_variable) in [
        ("DUSK_TARGET_ARCH", "CARGO_CFG_TARGET_ARCH"),
        ("DUSK_TARGET_OS", "CARGO_CFG_TARGET_OS"),
    ] {
        let value = std::env::var(cargo_variable)
            .unwrap_or_else(|error| panic!("{cargo_variable} is not set by cargo: {error}"));
        println!("cargo:rustc-env={variable}={value}");
    }
    dusk_build::build(&[(
        "capnp/init.capnp",
        &[dusk_build::CapnpDep {
            schema: concat!(env!("CARGO_MANIFEST_DIR"), "/../sh/capnp/bytecode.capnp"),
            crate_name: "dusk_program_sh",
            schema_ids: &[0xf5f34f381cd409b5],
        }],
    )]);
}
