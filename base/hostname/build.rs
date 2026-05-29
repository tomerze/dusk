fn main() {
    dusk_build::build(
        "capnp/hostname.capnp",
        &[dusk_build::CapnpDep {
            schema: concat!(env!("CARGO_MANIFEST_DIR"), "/../sh/capnp/sh.capnp"),
            crate_name: "dusk_program_sh",
            schema_ids: &[0xb25a041190c0e845],
        }],
    );
}
