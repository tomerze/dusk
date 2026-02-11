use convert_case::{Case, Casing};
use quote::{format_ident, quote};

use crate::parse::Metadata;

use std::collections::HashSet;
use syn::{Block, Item, ItemFn, Stmt};

pub fn merge_mixin(
    mixin: Option<&Block>,
    defaults: Vec<proc_macro2::TokenStream>,
) -> proc_macro2::TokenStream {
    let mut user_methods = HashSet::new();
    let mut mixin_tokens = quote! {};

    // 1. Identify what the user wrote in their mixin
    if let Some(block) = mixin {
        for stmt in &block.stmts {
            if let Stmt::Item(Item::Fn(f)) = stmt {
                user_methods.insert(f.sig.ident.to_string());
            }
        }
        let stmts = &block.stmts;
        mixin_tokens = quote! { #(#stmts)* };
    }

    // 2. Filter defaults: Only keep those NOT in user_methods
    let filtered_defaults = defaults.into_iter().filter(|tokens| {
        // Parse the token stream to find the function name
        if let Ok(item_fn) = syn::parse2::<ItemFn>(tokens.clone()) {
            return !user_methods.contains(&item_fn.sig.ident.to_string());
        }
        true // If it's not a function (e.g. a type), keep it
    });

    quote! {
        #(#filtered_defaults)*
        #mixin_tokens
    }
}

pub fn format_header(metadata: &Metadata) -> proc_macro2::TokenStream {
    let capnp_mod_name = format_ident!("{}_capnp", metadata.name);
    let capnp_mod_path = format!("/capnp/{}_capnp.rs", metadata.name);
    let name = &metadata.name;

    let args_server: syn::Path = syn::parse_str(&format!(
        "{}_capnp::{}_args::Server",
        metadata.name, metadata.name
    ))
    .unwrap();
    let portal_server: syn::Path = syn::parse_str(&format!(
        "dusk_program::dusk_capnp::dusk_capnp::portal::Server"
    ))
    .unwrap();
    let portal_type_alias = format_ident!("{}Portal", metadata.name.to_case(Case::Pascal));
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
            ($self_ty:ty, { $($body:tt)* }) => {
                impl #args_server for $self_ty {
                    $($body)*
                }
            };
        }

        /// Helper macro encoding the portal RPC server trait path for this program.
        macro_rules! __portal_server_path {
            ($self_ty:ty, { $($body:tt)* }) => {
                impl #portal_server for $self_ty {
                    $($body)*
                }
            };
        }

        /// Helper macro used by #[derive(Portal)] to emit the type alias and blanket impl.
        macro_rules! __derive_portal {
            ($user_ty:ty) => {
                pub type #portal_type_alias = $user_ty;
                impl #program_portal_server for $user_ty {}
            };
        }
    }
}

//pub fn format_args_section(section: &Section) -> proc_macro2::TokenStream {

pub mod format_section {
    use quote::format_ident;

    use super::*;
    use crate::parse::{ItemContent, Metadata, Section};

    pub fn launcher(section: &Section, metadata: &Metadata) -> proc_macro2::TokenStream {
        let pascal_name = metadata.name.to_case(Case::Pascal);
        let struct_name = format_ident!("{}Launcher", pascal_name);
        let program_id = &metadata.program_id;
        let process_type = format_ident!("{}Process", pascal_name);
        let args_client: syn::Path = syn::parse_str(
            format!("{}_capnp::{}_args::Client", metadata.name, metadata.name).as_str(),
        )
        .unwrap();

        let mut public_type: Option<&syn::Path> = None;
        let mut mixin: Option<&syn::Block> = None;
        for item in section.items.iter() {
            match item.key.to_string().as_str() {
                "public_type" => {
                    if let ItemContent::Path(path) = &item.content {
                        public_type = Some(path);
                    } else {
                        return syn::Error::new_spanned(
                            &item.key,
                            "expected a type path for public_type",
                        )
                        .to_compile_error();
                    }
                }
                "mixin" => {
                    if let ItemContent::Block(block) = &item.content {
                        mixin = Some(block);
                    } else {
                        return syn::Error::new_spanned(&item.key, "expected a block for mixin")
                            .to_compile_error();
                    }
                }
                _ => {
                    return syn::Error::new_spanned(
                        &item.key,
                        format!("unknown item key in section: {}", item.key),
                    )
                    .to_compile_error();
                }
            }
        }

        let type_definition = if let Some(public_type) = public_type {
            quote! {
                pub type #struct_name = #public_type;
            }
        } else {
            quote! {
                pub struct #struct_name;
            }
        };
        let state_type_name = format_ident!("{}ProcessState", pascal_name);

        let default_mixin_methods = vec![
            quote! {
                fn program_id(&self) -> u64 { #program_id }
            },
            quote! {
                fn launch(
                    &mut self,
                    pid: u64,
                    namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
                    program_args: dusk_capnp::dusk_capnp::program_args::Client,
                ) -> anyhow::Result<Box<dyn dusk_program::process::Process>> {
                    let state: #state_type_name = Default::default();
                    let cast_program_args = capnp::capability::FromClientHook::cast_to::<#args_client>(program_args);
                    Ok(Box::new(<#process_type>::new(pid, namespace, cast_program_args, state)))
                }
            },
        ];

        let mixin_body = merge_mixin(mixin, default_mixin_methods);

        quote! {
            #type_definition

            impl dusk_program::launcher::Launcher for #struct_name {
                #mixin_body
            }
        }
    }

    pub fn process(section: &Section, metadata: &Metadata) -> proc_macro2::TokenStream {
        let name = metadata.name.as_str();
        let version = &metadata.version;
        let pascal_name = metadata.name.to_case(Case::Pascal);
        let struct_name = format_ident!("{}Process", pascal_name);
        let program_id = &metadata.program_id;
        let args_client: syn::Path = syn::parse_str(
            format!("{}_capnp::{}_args::Client", metadata.name, metadata.name).as_str(),
        )
        .unwrap();
        let portal_client: syn::Path = syn::parse_str(
            format!("{}_capnp::{}_portal::Client", metadata.name, metadata.name).as_str(),
        )
        .unwrap();
        let portal_type = format_ident!("{}Portal", pascal_name);

        let mut state_type: Option<&syn::Path> = None;
        let mut mixin: Option<&syn::Block> = None;
        for item in section.items.iter() {
            match item.key.to_string().as_str() {
                "state_type" => {
                    if let ItemContent::Path(path) = &item.content {
                        state_type = Some(path);
                    } else {
                        return syn::Error::new_spanned(
                            &item.key,
                            "expected a type path for state_type",
                        )
                        .to_compile_error();
                    }
                }
                "mixin" => {
                    if let ItemContent::Block(block) = &item.content {
                        mixin = Some(block);
                    } else {
                        return syn::Error::new_spanned(&item.key, "expected a block for mixin")
                            .to_compile_error();
                    }
                }
                _ => {
                    return syn::Error::new_spanned(
                        &item.key,
                        format!("unknown item key in section: {}", item.key),
                    )
                    .to_compile_error();
                }
            }
        }

        let state_type_name = format_ident!("{}ProcessState", pascal_name);

        let state_type_definition = if let Some(state_type) = state_type {
            quote! { pub type #state_type_name = #state_type; }
        } else {
            quote! { pub type #state_type_name = (); }
        };
        let default_mixin_methods = vec![
            quote! {
                fn pid(&self) -> u64 {
                    self.pid
                }
            },
            quote! {
                fn program_id(&self) -> u64 {
                    #program_id
                }
            },
            quote! {
                fn name(&self) -> alloc::string::String {
                    alloc::string::String::from(#name)
                }
            },
            quote! {
                fn version(&self) -> alloc::string::String {
                    alloc::string::String::from(format!("{}", #version))
                }
            },
            quote! {
                fn namespace(&self) -> alloc::rc::Rc<dusk_program::namespace::Namespace> {
                    self.namespace.clone()
                }
            },
            quote! {
                fn clone_box(&self) -> Box<dyn dusk_program::process::Process> {
                    Box::new(#struct_name {
                        pid: self.pid,
                        namespace: self.namespace.clone(),
                        program_args: self.program_args.clone(),
                        state: self.state.clone(),
                    })
                }
            },
            quote! {
                fn portal(&self) -> dusk_capnp::dusk_capnp::portal::Client {
                    let client: #portal_client =
                        capnp_rpc::new_client(<#portal_type>::new(self.clone()));
                    client.cast_to::<dusk_capnp::dusk_capnp::portal::Client>()
                }
            },
        ];

        let mixin_body = merge_mixin(mixin, default_mixin_methods);

        quote! {
            #state_type_definition

            #[derive(Clone)]
            pub struct #struct_name{
                pub pid: u64,
                pub namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
                pub program_args: #args_client,
                pub state: #state_type_name,
            }

            impl #struct_name {
                pub fn new(
                    pid: u64,
                    namespace: alloc::rc::Rc<dusk_program::namespace::Namespace>,
                    program_args: #args_client,
                    state: #state_type_name,
                ) -> Self {
                    #struct_name {
                        pid,
                        namespace,
                        program_args,
                        state
                    }
                }
            }

            #[async_trait::async_trait(?Send)]
            impl dusk_program::process::Process for #struct_name {
                #mixin_body
            }
        }
    }
}
