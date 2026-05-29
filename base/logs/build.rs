fn main() {
    dusk_capnp::build_capnp(
        "capnp/logs.capnp",
        &[dusk_capnp::CapnpDep {
            schema: concat!(env!("CARGO_MANIFEST_DIR"), "/../sh/capnp/sh.capnp"),
            crate_name: "dusk_program_sh",
            schema_ids: &[0xb25a041190c0e845],
        }],
    );
}
