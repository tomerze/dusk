use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::dusk;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub fn answered(method: &'static str) {
    metrics::counter!("nightfall_simulator_calls_total", "method" => method).increment(1);
}

pub fn unix_milliseconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

pub struct Node {
    pub namespace_id: u64,
    pub device_id: String,
    last_heard: Cell<Instant>,
}

impl Node {
    pub fn new(namespace_id: u64, device_id: String) -> Rc<Node> {
        Rc::new(Node {
            namespace_id,
            device_id,
            last_heard: Cell::new(Instant::now()),
        })
    }

    pub fn heard(&self) {
        self.last_heard.set(Instant::now());
    }

    pub fn last_heard(&self) -> Instant {
        self.last_heard.get()
    }
}

pub struct Bootstrap {
    node: Rc<Node>,
}

impl Bootstrap {
    pub fn client(node: Rc<Node>) -> dusk::Client {
        capnp_rpc::new_client(Bootstrap { node })
    }
}

impl dusk::Server for Bootstrap {
    fn time(
        &mut self,
        _params: dusk::TimeParams,
        mut results: dusk::TimeResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.time");
        self.node.heard();
        results.get().set_unix_time_ms(unix_milliseconds());
        Promise::ok(())
    }

    fn programs(
        &mut self,
        _params: dusk::ProgramsParams,
        mut results: dusk::ProgramsResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.programs");
        results.get().init_program_entries(0);
        Promise::ok(())
    }

    fn namespace_id(
        &mut self,
        _params: dusk::NamespaceIdParams,
        mut results: dusk::NamespaceIdResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.namespaceId");
        results.get().set_result(self.node.namespace_id);
        Promise::ok(())
    }

    fn dusk(
        &mut self,
        _params: dusk::DuskParams,
        mut results: dusk::DuskResults,
    ) -> Promise<(), capnp::Error> {
        answered("Dusk.dusk");
        results
            .get()
            .set_result(Bootstrap::client(self.node.clone()));
        Promise::ok(())
    }
}
