fn main() {
    let namespace_id = dusk_node::dusk_new();
    if namespace_id == 0 {
        eprintln!("couldn't draw a namespace id from the operating system's random source");
        std::process::exit(-1);
    }
    let result = dusk_node::dusk_run(namespace_id, std::ptr::null_mut());
    let status = result & dusk_node::DUSK_STATUS_MASK;
    if status != dusk_node::DUSK_RUN_OK {
        std::process::exit(status);
    }
    std::process::exit(result >> dusk_node::DUSK_EXIT_CODE_SHIFT);
}
