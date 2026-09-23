extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::Literal;
use quote::quote;

#[proc_macro]
pub fn compile_command(item: TokenStream) -> TokenStream {
    let source = syn::parse_macro_input!(item as syn::LitStr);
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

fn compile(source: &str) -> Result<Vec<u8>, String> {
    dusk_base::link_anchors();
    let disconnected: dusk_capnp::dusk_capnp::dusk::Client =
        dusk_capnp::capnp_rpc::new_future_client(async {
            Err(dusk_capnp::capnp::Error::disconnected(
                "a command compiled at build time has no node to talk to".to_string(),
            ))
        });
    futures::executor::block_on(dusk_program_sh::compile(
        disconnected,
        dusk_program_sh::entry::StaticShEntriesBuilder::default(),
        source,
        &[],
    ))
    .map_err(|error| format!("{error:#}"))
}
