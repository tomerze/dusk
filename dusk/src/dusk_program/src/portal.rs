use alloc::boxed::Box;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::portal;
use dusk_capnp::dusk_capnp::stream;
use dusk_capnp::pry;

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
    fn get_input_stream(&self) -> stream::Client {
        capnp_rpc::new_client(NoopStream {})
    }
    fn set_output_stream(&self, _stream: stream::Client) {}
}

impl portal::Server for dyn Portal {
    fn input(
        &mut self,
        _params: portal::InputParams,
        mut results: portal::InputResults,
    ) -> Promise<(), ::capnp::Error> {
        results
            .get()
            .set_stream(<Self as Portal>::get_input_stream(self));
        Promise::ok(())
    }

    fn output(
        &mut self,
        params: portal::OutputParams,
        _results: portal::OutputResults,
    ) -> Promise<(), ::capnp::Error> {
        let stream = pry!(pry!(params.get()).get_stream());
        <Self as Portal>::set_output_stream(self, stream);
        Promise::ok(())
    }
}

impl portal::Server for Box<dyn Portal> {
    fn input(
        &mut self,
        _params: portal::InputParams,
        results: portal::InputResults,
    ) -> Promise<(), ::capnp::Error> {
        <dyn Portal as portal::Server>::input(&mut **self, _params, results)
    }

    fn output(
        &mut self,
        params: portal::OutputParams,
        _results: portal::OutputResults,
    ) -> Promise<(), ::capnp::Error> {
        <dyn Portal as portal::Server>::output(&mut **self, params, _results)
    }
}
