use dusk_base::dusk_program_init::Args as InitArgs;
use dusk_base::dusk_program_kvs::KvsConfig;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let (Some(address), Some(path)) = (arguments.next(), arguments.next()) else {
        eprintln!("usage: kvs_persistent_node <address:port> <file>");
        std::process::exit(2);
    };
    let disconnected: dusk_capnp::dusk_capnp::dusk::Client = capnp_rpc::new_future_client(async {
        Err(capnp::Error::disconnected(
            "the node compiles its init script before any client connects".to_string(),
        ))
    });
    let init_script = futures::executor::block_on(dusk_program_sh::compile_to_words(
        disconnected,
        &format!("nightfall -l {address}"),
    ))
    .expect("compile the init script");
    let init_program_args = InitArgs::new(&init_script)
        .expect("build init args")
        .as_program_args()
        .expect("build init program_args");
    let exit = dusk_nix::run(
        0,
        move || {
            dusk_base::launcher_set(KvsConfig {
                persistent: Some(path.clone()),
            })
        },
        init_program_args,
    );
    eprintln!("the node exited: {exit:?}");
}
