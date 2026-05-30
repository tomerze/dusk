fn main() {
    dusk_build::build(&[
        // `logs.capnp` imports `log_record.capnp`; compile it on its own so its
        // Rust module is generated, and declare it as a dependency of
        // `logs.capnp` so references resolve to this crate's `log_record_capnp`.
        ("capnp/log_record.capnp", &[]),
        (
            "capnp/logs.capnp",
            &[
                dusk_build::CapnpDep {
                    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/../sh/capnp/sh.capnp"),
                    crate_name: "dusk_program_sh",
                    schema_ids: &[0xb25a041190c0e845],
                },
                dusk_build::CapnpDep {
                    schema: concat!(env!("CARGO_MANIFEST_DIR"), "/capnp/log_record.capnp"),
                    crate_name: "dusk_program_logs",
                    schema_ids: &[0xd06f74de2a9b6c32],
                },
            ],
        ),
    ]);
}
