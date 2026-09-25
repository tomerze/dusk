fn main() {
    dusk_build::build(&[(
        "capnp/sh.capnp",
        &[dusk_build::CapnpDep {
            schema: concat!(env!("CARGO_MANIFEST_DIR"), "/compiler/capnp/bytecode.capnp"),
            crate_name: "dusk_program_sh_compiler",
            schema_ids: &[0xf5f34f381cd409b5],
        }],
    )]);
}
