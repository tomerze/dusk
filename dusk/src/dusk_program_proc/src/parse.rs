use syn::{
    Token,
    parse::{Parse, ParseStream, Result},
};

// Metadata header
pub struct Metadata {
    pub name: std::string::String,
    pub version: syn::Expr,
    pub program_id: syn::Path,
}

impl Parse for Metadata {
    fn parse(input: ParseStream) -> Result<Self> {
        let name_lit: syn::LitStr = input.parse()?;
        input.parse::<Token![,]>()?;
        let version: syn::Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let program_id: syn::Path = input.parse()?;
        Ok(Metadata {
            name: name_lit.value(),
            version,
            program_id,
        })
    }
}
