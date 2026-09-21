fn main() {
    dusk_build::build(&[(
        "capnp/sh.capnp",
        &[dusk_build::CapnpDep {
            schema: concat!(env!("CARGO_MANIFEST_DIR"), "/bytecode/capnp/bytecode.capnp"),
            crate_name: "dusk_program_sh_bytecode",
            schema_ids: &[0xf5f34f381cd409b5],
        }],
    )]);
}
