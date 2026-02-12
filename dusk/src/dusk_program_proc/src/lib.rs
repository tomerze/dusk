extern crate proc_macro;

mod format;
mod parse;

use proc_macro::TokenStream;

use format::format_header;

use parse::Metadata;

#[proc_macro]
pub fn metadata(item: TokenStream) -> TokenStream {
    let parsed = syn::parse_macro_input!(item as Metadata);
    TokenStream::from(format_header(&parsed))
}

#[proc_macro_derive(Args)]
pub fn derive_args(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let struct_name = &input.ident;

    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        #(#cfg_attrs)*
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

#[proc_macro_attribute]
pub fn impl_args_rpc_server(attr: TokenStream, item: TokenStream) -> TokenStream {
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
    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        __impl_args_rpc_server!(#(#cfg_attrs)* #self_ty, {
            #(#items)*
        });
    };

    TokenStream::from(expanded)
}

#[proc_macro_derive(Launcher)]
pub fn derive_launcher(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let struct_name = &input.ident;

    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        __derive_launcher!(#(#cfg_attrs)* #struct_name);
    };

    TokenStream::from(expanded)
}

#[proc_macro_derive(Process)]
pub fn derive_process(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let struct_name = &input.ident;

    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        __derive_process!(#(#cfg_attrs)* #struct_name);
    };

    TokenStream::from(expanded)
}

#[proc_macro_attribute]
pub fn process_mixin(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(
            proc_macro2::TokenStream::from(attr),
            "process_mixin takes no arguments",
        )
        .to_compile_error()
        .into();
    }

    let input = syn::parse_macro_input!(item as syn::ItemImpl);
    let self_ty = &input.self_ty;
    let items = &input.items;
    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        __process_mixin_path!(#(#cfg_attrs)* #self_ty, {
            #(#items)*
        });
    };

    TokenStream::from(expanded)
}

#[proc_macro_derive(Portal)]
pub fn derive_portal(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let struct_name = &input.ident;

    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        #(#cfg_attrs)*
        impl dusk_program::dusk_capnp::dusk_capnp::portal::Server for #struct_name {
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

#[proc_macro_attribute]
pub fn impl_portal_rpc_server(attr: TokenStream, item: TokenStream) -> TokenStream {
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
    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        __impl_portal_rpc_server!(#(#cfg_attrs)* #self_ty, {
            #(#items)*
        });
    };

    TokenStream::from(expanded)
}
