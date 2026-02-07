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
            "args" => format_section::args(section, metadata),
            "launcher" => format_section::launcher(section, metadata),
            "process" => format_section::process(section, metadata),
            "portal" => format_section::portal(section, metadata),
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
