extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

#[proc_macro_attribute]
pub fn sh_entry(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(TokenStream2::from(attr), "sh_entry takes no arguments")
            .to_compile_error()
            .into();
    }

    let body = syn::parse_macro_input!(item as syn::ItemFn);
    quote! {
        #[::linkme::distributed_slice(::dusk_program_sh::entry::SH_ENTRIES)]
        #body
    }
    .into()
}
