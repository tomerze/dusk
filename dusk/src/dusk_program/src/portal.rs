pub trait Portal {}

#[macro_export]
macro_rules! impl_portal_server {
    ($t:ty) => {
        impl dusk_capnp::dusk_capnp::portal::Server for $t {
            fn input(
                &mut self,
                _params: dusk_capnp::dusk_capnp::portal::InputParams,
                mut results: dusk_capnp::dusk_capnp::portal::InputResults,
            ) -> capnp::capability::Promise<(), ::capnp::Error> {
                results.get().set_stream(capnp_rpc::new_client(
                    dusk_program::stream::NoopStream::default(),
                ));
                Promise::ok(())
            }

            fn output(
                &mut self,
                params: dusk_capnp::dusk_capnp::portal::OutputParams,
                mut results: dusk_capnp::dusk_capnp::portal::OutputResults,
            ) -> capnp::capability::Promise<(), ::capnp::Error> {
                dusk_capnp::pry!(results.set_pipeline());
                let stream = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_stream());
                Promise::from_future(async move {
                    stream.done_request().send().promise.await?;
                    Ok(())
                })
            }
        }
    };
}
