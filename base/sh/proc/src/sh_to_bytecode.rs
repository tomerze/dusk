use dusk_program_sh_bytecode::bytecode;
use proc_macro2::{Literal, TokenStream};
use quote::quote;

pub(crate) fn expand(source: &str) -> Result<TokenStream, String> {
    let lowered = bytecode::lower_from_source(source).map_err(|error| error.to_string())?;
    let lowered = Literal::byte_string(&lowered);
    Ok(quote! { #lowered.to_vec() })
}
