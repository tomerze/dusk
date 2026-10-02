fn main() {
    std::process::exit(dusk_node::dusk_run(dusk_node::dusk_new(), std::ptr::null_mut()) as i32);
}
