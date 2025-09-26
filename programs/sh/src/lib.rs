#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;

use dusk_capnp::dusk_capnp::process;
#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use anyhow::Result;
use capnp::capability::FromClientHook;
use capnp::capability::Promise;
use dusk_capnp::pry;
use dusk_program::impl_portal_server;
use dusk_program::portal::Portal;
use dusk_program::stream::UndoneStream;
use dusk_program::{basic_launcher, basic_process};

#[cfg(feature = "client")]
pub mod args;

#[cfg(feature = "client")]
pub mod entry;

#[cfg(feature = "client")]
pub mod engine;

#[allow(clippy::all)]
pub mod sh_capnp {
    include!(concat!(env!("OUT_DIR"), "/capnp/sh_capnp.rs"));
}

basic_launcher!(
    ShLauncher,
    sh_capnp::PROGRAM_ID,
    ShProcess,
    sh_capnp::sh_args::Client
);
basic_process!(
    ShProcess,
    sh_capnp::PROGRAM_ID,
    ShPortal,
    sh_capnp::sh_portal::Client,
    sh_capnp::sh_args::Client
);

pub struct ShPortal {
    process: ShProcess,
}

impl ShPortal {
    pub fn new(process: ShProcess) -> Self {
        ShPortal { process }
    }

    async fn command_string_to_program_args(
        engine: sh_capnp::engine::Client,
        command: &str,
    ) -> Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let mut build_from_string_request = engine.build_program_args_from_string_request();
        build_from_string_request.get().set_string(command);
        let build_from_string_reply = build_from_string_request.send().promise.await?;
        let result = build_from_string_reply.get()?.get_program_args()?;
        Ok(result)
    }

    async fn execute_program_args(
        client: dusk_capnp::dusk_capnp::dusk::Client,
        program_args: dusk_capnp::dusk_capnp::program_args::Client,
    ) -> Result<process::Client> {
        let mut process_request = client.process_request();
        process_request.get().set_program_args(program_args);
        let process = capnp_rpc::new_future_client(async move {
            let process_reply = process_request.send().promise.await?;
            let process = process_reply.get()?.get_result()?;
            let mut run_request = client.run_request();
            run_request.get().set_process(process.clone());
            let _run_reply = run_request.send().promise.await?;
            Ok(process)
        });

        Ok(process)
    }

    // TODO change this to pipe io and support input as well
    async fn portal_and_pipe_output(
        process: process::Client,
        output: dusk_capnp::dusk_capnp::stream::Client,
    ) -> Result<()> {
        let portal = capnp_rpc::new_future_client(async move {
            let portal_request = process.portal_request();
            let portal_reply = portal_request.send().promise.await?;
            portal_reply.get()?.get_result()
        });

        let (undone_stream, done_receiver) = UndoneStream::new_with_done_receiver(output);

        let mut output_request = portal.output_request();
        output_request
            .get()
            .set_stream(capnp_rpc::new_client(undone_stream));
        let _output_reply = output_request.send().promise.await?;
        done_receiver.await.map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(())
    }
}

impl Portal for ShPortal {}

impl_portal_server!(ShPortal);

impl sh_capnp::sh_portal::Server for ShPortal {
    fn sh(
        &mut self,
        params: sh_capnp::sh_portal::ShParams,
        _results: sh_capnp::sh_portal::ShResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let command = pry!(pry!(pry!(params.get()).get_command()).to_string());
        let output = pry!(pry!(params.get()).get_output());

        let program_args = self.process.program_args.clone();

        Promise::from_future(async move {
            // TODO actually parse the command and make it work like a shell
            let get_request_result = program_args.get_request().send().promise.await?;
            let engine = get_request_result.get()?.get_engine()?;
            let client = get_request_result.get()?.get_client()?;
            // TODO make the output be run through dissect output in the engine
            let args = Self::command_string_to_program_args(engine, &command)
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))?;
            let process = Self::execute_program_args(client.clone(), args)
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))?;

            Self::portal_and_pipe_output(process.clone(), output.clone())
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))?;

            // TODO find out why this kill doesn't work
            let mut kill_request = client.kill_request();
            kill_request.get().set_process(process);
            kill_request.get().set_signal(15); // SIGTERM
            output.done_request().send().promise.await?;
            Ok(())
        })
    }
}
