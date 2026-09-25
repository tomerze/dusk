use quote::{format_ident, quote};

use crate::parse::Metadata;

pub fn format_header(metadata: &Metadata) -> proc_macro2::TokenStream {
    let capnp_mod_name = format_ident!("{}_capnp", metadata.name);
    let capnp_mod_path = format!("/capnp/{}_capnp.rs", metadata.name);
    let name = &metadata.name;
    let version = &metadata.version;
    let program_id = &metadata.program_id;

    let args_server: syn::Path = syn::parse_str(&format!(
        "{}_capnp::{}_args::server::Server",
        metadata.name, metadata.name
    ))
    .unwrap();

    let server_client: syn::Path = syn::parse_str(&format!(
        "{}_capnp::{}_args::server::Client",
        metadata.name, metadata.name
    ))
    .unwrap();

    let data_owned: syn::Path = syn::parse_str(&format!(
        "{}_capnp::{}_args::data::Owned",
        metadata.name, metadata.name
    ))
    .unwrap();

    let program_portal_server: syn::Path = syn::parse_str(&format!(
        "{}_capnp::{}_portal::Server",
        metadata.name, metadata.name
    ))
    .unwrap();

    quote! {
        use alloc::format;
        use alloc::string::String;
        use alloc::vec;
        use alloc::vec::Vec;

        #[allow(unused)]
        #[prelude_import]
        use dusk_program::dusk_capnp::prelude::*;

        use dusk_program::prelude::*;

        #[allow(clippy::all, unreachable_patterns)]
        pub mod #capnp_mod_name {
            include!(concat!(env!("OUT_DIR"), #capnp_mod_path));
        }

        #[allow(unused)]
        pub const PROGRAM_NAME: &str = #name;

        /// The git revision of the workspace at this crate's build time.
        #[allow(unused)]
        pub const GIT_REV: &str = env!("GIT_REV");

        #[allow(unused)]
        pub use #capnp_mod_name::PROGRAM_ID;

        /// Owned, in-memory builder for this program's `args.data` capnp
        /// struct.
        #[allow(unused)]
        pub type ArgsDataBuilder = dusk_program::dusk_capnp::capnp::message::TypedBuilder<#data_owned>;

        #[allow(unused)]
        pub use dusk_program::dusk_capnp::dusk_capnp::portal;
        #[allow(unused)]
        pub use dusk_program::value::{Record, Value};

        macro_rules! __impl_args_rpc_server {
            (
                $(#[$meta:meta])*
                [$($impl_generics:tt)*] [$($self_ty:tt)*] [$($where_clause:tt)*]
                { $($body:tt)* }
            ) => {
                $(#[$meta])*
                impl $($impl_generics)* #args_server for $($self_ty)* $($where_clause)* {
                    $($body)*
                }
            };
        }

        macro_rules! __derive_args {
            (
                $(#[$meta:meta])*
                [$($impl_generics:tt)*] [$($self_ty:tt)*] [$($where_clause:tt)*]
                [$data_field:ident]
                [$($created_field:ident)?]
            ) => {
                $(#[$meta])*
                const _: () = {
                    fn __derive_args_assert<__T: ?::core::marker::Sized + #args_server>() {}
                    #[allow(dead_code)]
                    fn __derive_args_check $($impl_generics)* () $($where_clause)* {
                        __derive_args_assert::<$($self_ty)*>();
                    }
                };

                $(#[$meta])*
                impl $($impl_generics)* $($self_ty)* $($where_clause)* {
                    #[allow(unused_qualifications)]
                    pub fn as_program_args(
                        mut self,
                    ) -> dusk_program::dusk_capnp::capnp::Result<
                        alloc::rc::Rc<dusk_program::program_args::ProgramArgs>,
                    > {
                        let owned = dusk_program::program_args::ProgramArgs::new();
                        // Phase 1: write program_id and args.data by copying
                        // the typed reader out of the `#[data]` field
                        // (`capnp::message::TypedBuilder<#data_owned>`) into
                        // the args.data slot. Borrows `&self.$data_field`;
                        // the borrow ends with the closure.
                        owned.with_root_builder(|mut root| {
                            root.set_program_id(PROGRAM_ID);
                            let mut data_dest = root.init_args().init_data();
                            let data_builder: <#data_owned as dusk_program::dusk_capnp::capnp::traits::Owned>::Builder<'_> =
                                self.$data_field.get_root()?;
                            data_dest.set_as::<#data_owned>(data_builder.into_reader())
                        })?;
                        $(
                            if let Some(created) = self.$created_field.clone() {
                                owned.set_created(created)?;
                            }
                        )?
                        // Phase 2: consume `self` into a server cap and stash
                        // it in args.server. Use `get_args()` (not
                        // `init_args()`) - `init_args()` clears both pointer
                        // slots and would wipe the data we just wrote.
                        let server: #server_client =
                            dusk_program::dusk_capnp::capnp_rpc::new_client(self);
                        owned.with_root_builder(|root| {
                            root.get_args().init_server().set_as_capability(
                                <_ as dusk_program::dusk_capnp::capnp::capability::FromClientHook>::into_client_hook(server),
                            );
                            Ok(())
                        })?;
                        Ok(alloc::rc::Rc::new(owned))
                    }
                }
            };
        }

        macro_rules! __derive_launcher {
            ($(#[$meta:meta])* $user_ty:ty) => {
                $(#[$meta])*
                impl dusk_program::launcher::Launcher for $user_ty {
                    fn program_id(&self) -> u64 {
                        #program_id
                    }
                    fn version(&self) -> alloc::string::String {
                        alloc::string::String::from(format!("{}", #version))
                    }
                    fn git_rev(&self) -> alloc::string::String {
                        alloc::string::String::from(GIT_REV)
                    }
                }
            };
        }

        macro_rules! __derive_process {
            ($(#[$meta:meta])* $user_ty:ty, $ctx_field:ident) => {
                $(#[$meta])*
                #[async_trait::async_trait(?Send)]
                impl dusk_program::process::Process for $user_ty {
                    fn program_id(&self) -> u64 {
                        #program_id
                    }
                    fn name(&self) -> alloc::string::String {
                        let name = self.$ctx_field.name.lock(|n| n.borrow().clone());
                        if let Some(name) = name {
                            name
                        } else {
                            alloc::string::String::from(#name)
                        }
                    }
                    fn version(&self) -> alloc::string::String {
                        alloc::string::String::from(format!("{}", #version))
                    }
                    fn git_rev(&self) -> alloc::string::String {
                        alloc::string::String::from(GIT_REV)
                    }
                    fn clone_box(&self) -> alloc::boxed::Box<dyn dusk_program::process::Process> {
                        alloc::boxed::Box::new(self.clone())
                    }
                    fn namespace(&self) -> alloc::rc::Rc<dusk_program::namespace::Namespace> {
                        self.$ctx_field.namespace.clone()
                    }
                    fn pid(&self) -> u64 {
                        self.$ctx_field.pid
                    }
                }
            };
        }

        macro_rules! __impl_portal_rpc_server {
            (
                $(#[$meta:meta])*
                [$($impl_generics:tt)*] [$($self_ty:tt)*] [$($where_clause:tt)*]
                { $($body:tt)* }
            ) => {
                $(#[$meta])*
                impl $($impl_generics)* #program_portal_server for $($self_ty)* $($where_clause)* {
                    $($body)*
                }
            };
        }

    }
}
