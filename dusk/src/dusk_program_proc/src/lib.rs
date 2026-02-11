extern crate proc_macro;

mod format;
mod parse;

use proc_macro::TokenStream;

use format::{format_header, format_section};

use parse::Definition;

#[proc_macro]
pub fn definition(item: TokenStream) -> TokenStream {
    let parsed = syn::parse_macro_input!(item as Definition);
    let mut output = proc_macro2::TokenStream::new();

    // Check for duplicate section names
    let mut seen = std::collections::HashSet::new();
    for section in &parsed.sections {
        let name = section.name.to_string();
        if !seen.insert(name.clone()) {
            return syn::Error::new_spanned(
                &section.name,
                format!("duplicate section name: {}", name),
            )
            .to_compile_error()
            .into();
        }
    }

    // Output header and metadata
    output.extend(format_header(&parsed.metadata));

    for section in &parsed.sections {
        let metadata = &parsed.metadata;
        let section_code = match section.name.to_string().as_str() {
            "launcher" => format_section::launcher(section, metadata),
            "process" => format_section::process(section, metadata),
            _ => {
                return syn::Error::new_spanned(
                    &section.name,
                    format!("unknown section name: {}", section.name),
                )
                .to_compile_error()
                .into();
            }
        };
        output.extend(section_code);
    }
    TokenStream::from(output)
}

#[proc_macro_derive(Args)]
pub fn derive_args(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let struct_name = &input.ident;

    let expanded = quote::quote! {
        #[cfg(feature = "client")]
        impl dusk_program::dusk_capnp::dusk_capnp::program_args::Server for #struct_name {
            fn program_id(
                &mut self,
                _params: dusk_program::dusk_capnp::dusk_capnp::program_args::ProgramIdParams,
                mut results: dusk_program::dusk_capnp::dusk_capnp::program_args::ProgramIdResults,
            ) -> capnp::capability::Promise<(), capnp::Error> {
                results.get().set_program_id(PROGRAM_ID);
                capnp::capability::Promise::ok(())
            }
        }
    };

    TokenStream::from(expanded)
}

#[proc_macro_derive(Portal)]
pub fn derive_portal(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let struct_name = &input.ident;

    let expanded = quote::quote! {
        __derive_portal!(#struct_name);
    };

    TokenStream::from(expanded)
}

/// Attribute macro that rewrites `impl Type { ... }` into `impl {name}_capnp::{name}_args::Server for Type { ... }`.
///
/// The trait path is derived from the program name declared in `definition!`.
///
/// Usage:
/// ```ignore
/// #[dusk_program_proc::args_rpc_server]
/// impl Args {
///     fn get(&mut self, ...) -> ... { ... }
/// }
/// ```
#[proc_macro_attribute]
pub fn args_rpc_server(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(
            proc_macro2::TokenStream::from(attr),
            "args_rpc_server takes no arguments",
        )
        .to_compile_error()
        .into();
    }

    let input = syn::parse_macro_input!(item as syn::ItemImpl);
    let self_ty = &input.self_ty;
    let items = &input.items;

    let expanded = quote::quote! {
        __args_server_path!(#self_ty, {
            #(#items)*
        });
    };

    TokenStream::from(expanded)
}

/// Attribute macro that rewrites `impl Type { ... }` into `impl portal::Server for Type { ... }`.
///
/// The trait path is derived from the program name declared in `definition!`.
///
/// Usage:
/// ```ignore
/// #[dusk_program_proc::portal_rpc_server]
/// impl Portal {
///     fn input(&mut self, ...) -> ... { ... }
///     fn output(&mut self, ...) -> ... { ... }
/// }
/// ```
#[proc_macro_attribute]
pub fn portal_rpc_server(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(
            proc_macro2::TokenStream::from(attr),
            "portal_rpc_server takes no arguments",
        )
        .to_compile_error()
        .into();
    }

    let input = syn::parse_macro_input!(item as syn::ItemImpl);
    let self_ty = &input.self_ty;
    let items = &input.items;

    let expanded = quote::quote! {
        __portal_server_path!(#self_ty, {
            #(#items)*
        });
    };

    TokenStream::from(expanded)
}
