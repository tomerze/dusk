use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::stream;

struct NoopStream {}

impl stream::Server for NoopStream {
    fn send(&mut self, _: stream::SendParams) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }

    fn done(
        &mut self,
        _: stream::DoneParams,
        _: stream::DoneResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        Promise::ok(())
    }
}

pub trait Portal {
    fn get_input_stream(&self, stream: &mut Option<stream::Client>) -> Promise<(), ::capnp::Error> {
        *stream = Some(capnp_rpc::new_client(NoopStream {}));
        Promise::ok(())
    }
    fn set_output_stream(&self, stream: stream::Client) -> Promise<(), ::capnp::Error> {
        Promise::from_future(async move {
            stream.done_request().send().promise.await?;
            Ok(())
        })
    }
}

#[macro_export]
macro_rules! impl_default_io_portal_server {
    ($t:ty) => {
        impl dusk_capnp::dusk_capnp::portal::Server for $t {
            fn input(
                &mut self,
                _params: dusk_capnp::dusk_capnp::portal::InputParams,
                mut results: dusk_capnp::dusk_capnp::portal::InputResults,
            ) -> capnp::capability::Promise<(), ::capnp::Error> {
                let mut stream: Option<dusk_capnp::dusk_capnp::stream::Client> = None;
                let promise = <Self as Portal>::get_input_stream(self, &mut stream);
                if let Some(stream) = stream {
                    results.get().set_stream(stream);
                }
                promise
            }

            fn output(
                &mut self,
                params: dusk_capnp::dusk_capnp::portal::OutputParams,
                mut results: dusk_capnp::dusk_capnp::portal::OutputResults,
            ) -> capnp::capability::Promise<(), ::capnp::Error> {
                dusk_capnp::pry!(results.set_pipeline());
                let stream = dusk_capnp::pry!(dusk_capnp::pry!(params.get()).get_stream());
                <Self as Portal>::set_output_stream(self, stream)
            }
        }
    };
}
