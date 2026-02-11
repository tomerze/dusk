use convert_case::{Case, Casing};
use quote::{format_ident, quote};

use crate::parse::Metadata;

pub fn format_header(metadata: &Metadata) -> proc_macro2::TokenStream {
    let capnp_mod_name = format_ident!("{}_capnp", metadata.name);
    let capnp_mod_path = format!("/capnp/{}_capnp.rs", metadata.name);
    let name = &metadata.name;
    let version = &metadata.version;
    let program_id = &metadata.program_id;

    let pascal_name = metadata.name.to_case(Case::Pascal);
    let process_struct_name = format_ident!("{}Process", pascal_name);
    let launcher_type_alias = format_ident!("{}Launcher", pascal_name);
    let state_type_alias = format_ident!("{}ProcessState", pascal_name);

    let args_server: syn::Path = syn::parse_str(&format!(
        "{}_capnp::{}_args::Server",
        metadata.name, metadata.name
    ))
    .unwrap();
    let args_client: syn::Path = syn::parse_str(&format!(
        "{}_capnp::{}_args::Client",
        metadata.name, metadata.name
    ))
    .unwrap();
    let portal_server: syn::Path =
        syn::parse_str("dusk_program::dusk_capnp::dusk_capnp::portal::Server").unwrap();
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

        #[allow(clippy::all)]
        pub mod #capnp_mod_name {
            include!(concat!(env!("OUT_DIR"), #capnp_mod_path));
        }

        /// The program name, as declared in `definition!` metadata.
        #[allow(unused)]
        pub const PROGRAM_NAME: &str = #name;

        #[allow(unused)]
        pub use #capnp_mod_name::PROGRAM_ID;

        #[allow(unused)]
        pub use dusk_program::dusk_capnp::dusk_capnp::portal;
        #[allow(unused)]
        pub use dusk_program::value::{Record, Value};

        /// Helper macro encoding the args RPC server trait path for this program.
        macro_rules! __args_server_path {
            ($(#[$meta:meta])* $self_ty:ty, { $($body:tt)* }) => {
                $(#[$meta])*
                impl #args_server for $self_ty {
                    $($body)*
                }
            };
        }

        /// Helper macro encoding the portal RPC server trait path for this program.
        macro_rules! __portal_server_path {
            ($(#[$meta:meta])* $self_ty:ty, { $($body:tt)* }) => {
                $(#[$meta])*
                impl #portal_server for $self_ty {
                    $($body)*
                }
            };
        }

        /// Helper macro used by #[derive(Portal)] to emit the type alias and blanket impl.
        macro_rules! __derive_portal {
            ($(#[$meta:meta])* $user_ty:ty) => {
                $(#[$meta])*
                impl #program_portal_server for $user_ty {}
            };
        }

        /// Helper macro used by #[derive(Process)] to emit the Process struct and impls.
        macro_rules! __derive_process {
            ($(#[$meta:meta])* $state_ty:ty) => {
                $(#[$meta])*
                type #state_type_alias = $state_ty;

                $(#[$meta])*
                #[derive(Clone)]
                pub struct #process_struct_name {
                    pub pid: u64,
                    pub namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
                    pub program_args: #args_client,
                    pub state: $state_ty,
                }

                $(#[$meta])*
                impl #process_struct_name {
                    pub fn new(
                        pid: u64,
                        namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
                        program_args: #args_client,
                        state: $state_ty,
                    ) -> Self {
                        #process_struct_name {
                            pid,
                            namespace,
                            program_args,
                            state,
                        }
                    }
                }

                $(#[$meta])*
                #[async_trait::async_trait(?Send)]
                impl dusk_program::process::Process for #process_struct_name {
                    fn pid(&self) -> u64 {
                        self.pid
                    }
                    fn program_id(&self) -> u64 {
                        #program_id
                    }
                    fn name(&self) -> alloc::string::String {
                        alloc::string::String::from(#name)
                    }
                    fn version(&self) -> alloc::string::String {
                        alloc::string::String::from(format!("{}", #version))
                    }
                    fn namespace(&self) -> alloc::rc::Rc<dusk_program::namespace::Namespace> {
                        self.namespace.clone()
                    }
                    fn clone_box(&self) -> Box<dyn dusk_program::process::Process> {
                        Box::new(#process_struct_name {
                            pid: self.pid,
                            namespace: self.namespace.clone(),
                            program_args: self.program_args.clone(),
                            state: self.state.clone(),
                        })
                    }
                }
            };
        }

        /// Helper macro used by #[process_mixin] to emit the ProcessMixin impl.
        macro_rules! __process_mixin_path {
            ($(#[$meta:meta])* $state_ty:ty, { $($body:tt)* }) => {
                $(#[$meta])*
                #[async_trait::async_trait(?Send)]
                impl dusk_program::process::ProcessMixin for #process_struct_name {
                    $($body)*
                }
            };
        }

        /// Helper macro used by #[derive(Launcher)] to emit the Launcher type alias and impl.
        macro_rules! __derive_launcher {
            ($(#[$meta:meta])* $user_ty:ty) => {
                $(#[$meta])*
                type #launcher_type_alias = $user_ty;

                $(#[$meta])*
                impl dusk_program::launcher::Launcher for $user_ty {
                    fn program_id(&self) -> u64 {
                        #program_id
                    }
                }
            };
        }

        /// Helper macro used by #[launcher_mixin] to emit the LauncherMixin impl.
        macro_rules! __launcher_mixin_path {
            ($(#[$meta:meta])* $user_ty:ty, { $($body:tt)* }) => {
                $(#[$meta])*
                impl dusk_program::launcher::LauncherMixin for #launcher_type_alias {
                    $($body)*
                }
            };
        }
    }
}
