use capnpc::CompilerCommand;

fn main() {
    CompilerCommand::new()
        .file("capnp/dusk.capnp")
        .run()
        .unwrap();
}
