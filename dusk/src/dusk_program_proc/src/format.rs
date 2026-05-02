use quote::{format_ident, quote};

use crate::parse::Metadata;

pub fn format_header(metadata: &Metadata) -> proc_macro2::TokenStream {
    let capnp_mod_name = format_ident!("{}_capnp", metadata.name);
    let capnp_mod_path = format!("/capnp/{}_capnp.rs", metadata.name);
    let name = &metadata.name;
    let version = &metadata.version;
    let program_id = &metadata.program_id;

    let args_server: syn::Path = syn::parse_str(&format!(
        "{}_capnp::{}_args::Server",
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

        #[allow(unused)]
        pub use #capnp_mod_name::PROGRAM_ID;

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

        macro_rules! __derive_launcher {
            ($(#[$meta:meta])* $user_ty:ty) => {
                $(#[$meta])*
                impl dusk_program::launcher::Launcher for $user_ty {
                    fn program_id(&self) -> u64 {
                        #program_id
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
