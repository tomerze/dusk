use dusk_program_sh_compiler::compile;
use proc_macro2::{Literal, TokenStream};
use quote::quote;

pub(crate) fn expand(source: &str) -> Result<TokenStream, String> {
    let bytecode = compile::compile(source).map_err(|error| error.to_string())?;
    let bytecode = Literal::byte_string(&bytecode);
    Ok(quote! { #bytecode.to_vec() })
}
