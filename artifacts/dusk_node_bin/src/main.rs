use std::ffi::{CString, c_void};

fn main() {
    let argument = std::env::args()
        .nth(1)
        .map(|argument| CString::new(argument).expect("an argv string holds no NUL byte"));
    let user = argument.as_ref().map_or(std::ptr::null_mut(), |argument| {
        argument.as_ptr().cast_mut().cast::<c_void>()
    });
    std::process::exit(unsafe { dusk_node::dusk_node_run(user) });
}
