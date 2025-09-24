#![allow(internal_features)]
#![feature(prelude_import)]
#![cfg_attr(not(feature = "client"), no_std)]

extern crate alloc;

#[allow(unused)]
#[prelude_import]
use dusk_capnp::prelude::*;

use anyhow::Result;
use capnp::capability::FromClientHook;
use capnp::capability::Promise;
use dusk_capnp::dusk_capnp::portal;
use dusk_capnp::pry;
use dusk_program::portal::Portal;
use dusk_program::{basic_launcher, basic_process};

#[cfg(feature = "client")]
pub mod args;

#[cfg(feature = "client")]
pub mod program_args_builder;

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
        program_args_builder: sh_capnp::program_args_builder::Client,
        command: &str,
    ) -> Result<dusk_capnp::dusk_capnp::program_args::Client> {
        let mut build_from_string_request = program_args_builder.build_from_string_request();
        build_from_string_request.get().set_string(command);
        let build_from_string_reply = build_from_string_request.send().promise.await?;
        let result = build_from_string_reply.get()?.get_result()?;
        Ok(result)
    }

    async fn execute_program_args(
        client: dusk_capnp::dusk_capnp::dusk::Client,
        program_args: dusk_capnp::dusk_capnp::program_args::Client,
    ) -> Result<()> {
        let mut process_request = client.process_request();
        process_request.get().set_program_args(program_args);
        let process_reply = process_request.send().promise.await?;
        let process = process_reply.get()?.get_result()?;
        let _result = process.run_request().send().promise.await?;
        // TODO portal and stream!
        Ok(())
    }
}

impl Portal for ShPortal {}

impl portal::Server for ShPortal {}

impl sh_capnp::sh_portal::Server for ShPortal {
    fn sh(
        &mut self,
        params: sh_capnp::sh_portal::ShParams,
        _results: sh_capnp::sh_portal::ShResults,
    ) -> capnp::capability::Promise<(), capnp::Error> {
        let command = pry!(pry!(pry!(params.get()).get_command()).to_string());
        log::info!("Executing shell command: {}", &command);

        let program_args = self.process.program_args.clone();

        Promise::from_future(async move {
            let get_request_result = program_args.get_request().send().promise.await?;
            let program_args_builder = get_request_result.get()?.get_program_args_builder()?;
            let client = get_request_result.get()?.get_client()?;
            let args = Self::command_string_to_program_args(program_args_builder, &command)
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))?;
            Self::execute_program_args(client, args)
                .await
                .map_err(|e| capnp::Error::failed(e.to_string()))?;
            Ok(())
        })
    }
}
