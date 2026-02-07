use syn::{
    Attribute, Ident, Token,
    parse::{Parse, ParseStream, Result},
};

pub struct Definition {
    pub metadata: Metadata,
    pub sections: Vec<Section>,
}

// Metadata header
pub struct Metadata {
    pub name: std::string::String,
    pub version: syn::Expr,
    pub program_id: syn::Path,
}

// Section AST
pub struct Section {
    pub _attrs: Vec<Attribute>,
    pub name: Ident, // section name from [name]
    pub items: Vec<ItemEntry>,
}

// Individual item inside a section. Content can be either a block (e.g. `rpc_server: {}`)
// or a path/identifier/type (e.g. `type_name: Args`).
pub enum ItemContent {
    Block(syn::Block),
    Path(syn::Path),
}

pub struct ItemEntry {
    pub key: Ident, // e.g., type_name, rpc_server, mixin
    pub content: ItemContent,
}

impl Parse for Definition {
    fn parse(input: ParseStream) -> Result<Self> {
        let metadata: Metadata = input.parse()?;
        let mut sections = Vec::new();

        while !input.is_empty() {
            sections.push(input.parse()?);
        }

        Ok(Definition { metadata, sections })
    }
}

impl Parse for Metadata {
    fn parse(input: ParseStream) -> Result<Self> {
        let ident: Ident = input.parse()?; // should be `metadata`
        if ident != "metadata" {
            return Err(syn::Error::new(ident.span(), "expected `metadata`"));
        }

        let content;
        syn::parenthesized!(content in input);

        let name_lit: syn::LitStr = content.parse()?;
        content.parse::<Token![,]>()?;
        // Accept either a string literal or an identifier/expression for version
        let version: syn::Expr = content.parse()?;
        content.parse::<Token![,]>()?;
        let program_id: syn::Path = content.parse()?;
        Ok(Metadata {
            name: name_lit.value(),
            version,
            program_id,
        })
    }
}

impl Parse for Section {
    fn parse(input: ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;

        // parse bracketed section name: [args]
        let content;
        syn::bracketed!(content in input);
        let name: Ident = content.parse()?;

        let mut items = Vec::new();
        while !input.is_empty() {
            // Stop at next bracket or end of input
            if input.peek(syn::token::Bracket) {
                break;
            }
            // Skip doc comments and attributes
            let _ = input.call(Attribute::parse_outer)?;
            // After skipping, check again for a bracket (section end)
            if input.peek(syn::token::Bracket) {
                break;
            } else if input.peek(Ident) {
                items.push(input.parse()?);
            } else if input.peek(Token![;]) {
                input.parse::<Token![;]>().ok(); // skip extra semicolons
            } else {
                // If we can't parse anything, return an error to avoid infinite loop
                return Err(input.error("expected identifier, section, or ';' in section body"));
            }
        }

        Ok(Section {
            _attrs: attrs,
            name,
            items,
        })
    }
}

impl Parse for ItemEntry {
    fn parse(input: ParseStream) -> Result<Self> {
        let key: Ident = input.parse()?;
        input.parse::<Token![:]>()?;

        // If the next token is a brace, parse a block; otherwise parse a path (identifier or qualified).
        if input.peek(syn::token::Brace) {
            let content: syn::Block = input.parse()?;
            Ok(ItemEntry {
                key,
                content: ItemContent::Block(content),
            })
        } else {
            let path: syn::Path = input.parse()?;
            Ok(ItemEntry {
                key,
                content: ItemContent::Path(path),
            })
        }
    }
}
