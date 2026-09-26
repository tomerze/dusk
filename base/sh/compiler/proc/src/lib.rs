#![feature(proc_macro_tracked_env)]

extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::Literal;
use quote::quote;

#[proc_macro]
pub fn compile_sh(item: TokenStream) -> TokenStream {
    let source = syn::parse_macro_input!(item with parse_source);
    match compile(&source.value()) {
        Ok(compiled) => {
            let compiled = Literal::byte_string(&compiled);
            quote! { #compiled.to_vec() }.into()
        }
        Err(message) => syn::Error::new(source.span(), message)
            .to_compile_error()
            .into(),
    }
}

fn parse_source(input: syn::parse::ParseStream) -> syn::Result<syn::LitStr> {
    if input.peek(syn::LitStr) {
        return input.parse();
    }
    let span = input.span();
    let invocation = match input.parse::<syn::Macro>() {
        Ok(invocation) if invocation.path.is_ident("env") => invocation,
        _ => {
            return Err(syn::Error::new(
                span,
                "expected a string literal or `env!(\"NAME\")`",
            ));
        }
    };
    let variable: syn::LitStr = invocation.parse_body()?;
    let value = proc_macro::tracked::env_var(variable.value()).map_err(|error| {
        syn::Error::new(variable.span(), format!("`{}`: {error}", variable.value()))
    })?;
    Ok(syn::LitStr::new(&value, variable.span()))
}

fn compile(source: &str) -> Result<Vec<u8>, String> {
    dusk_base::link_anchors();
    let disconnected: dusk_capnp::dusk_capnp::dusk::Client =
        dusk_capnp::capnp_rpc::new_future_client(async {
            Err(dusk_capnp::capnp::Error::disconnected(
                "a command compiled at build time has no node to talk to".to_string(),
            ))
        });
    futures::executor::block_on(dusk_program_sh::compile_to_words(disconnected, source))
        .map_err(|error| format!("{error:#}"))
}
