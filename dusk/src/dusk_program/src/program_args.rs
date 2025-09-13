#[macro_export]
macro_rules! impl_program_args_server {
    ($type:ty, $program_id:expr) => {
        impl dusk_capnp::dusk_capnp::program_args::Server for $type {
            fn program_id(
                &mut self,
                _params: dusk_capnp::dusk_capnp::program_args::ProgramIdParams,
                mut results: dusk_capnp::dusk_capnp::program_args::ProgramIdResults,
            ) -> capnp::capability::Promise<(), capnp::Error> {
                results.get().set_program_id($program_id);
                capnp::capability::Promise::ok(())
            }
        }
    };
}
