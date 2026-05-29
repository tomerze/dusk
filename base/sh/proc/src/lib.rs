extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;

#[proc_macro_attribute]
pub fn sh_entry(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(TokenStream2::from(attr), "sh_entry takes no arguments")
            .to_compile_error()
            .into();
    }

    let input = syn::parse_macro_input!(item as syn::ItemFn);
    let fields = match extract_entry_info_literals(&input) {
        Ok(fields) => fields,
        Err(error) => return error.to_compile_error().into(),
    };

    let crate_name = std::env::var("CARGO_PKG_NAME")
        .expect("CARGO_PKG_NAME must be set during proc-macro expansion");
    let fn_name = input.sig.ident.to_string();
    write_entries_info(&crate_name, &fn_name, &fields);

    let body = &input;
    quote! {
        #[::linkme::distributed_slice(::dusk_program_sh::entry::SH_ENTRIES)]
        #body
    }
    .into()
}

struct EntryInfoLiterals {
    name: String,
    short_description: String,
    long_description: String,
}

fn extract_entry_info_literals(function: &syn::ItemFn) -> syn::Result<EntryInfoLiterals> {
    // Function body's tail expression must be `ShEntry { … }`.
    let last_stmt = function.block.stmts.last().ok_or_else(|| {
        syn::Error::new(
            function.block.span(),
            "sh_entry: function body must end in a ShEntry { … } literal",
        )
    })?;
    let tail_expr = match last_stmt {
        syn::Stmt::Expr(expr, None) => expr,
        other => {
            return Err(syn::Error::new(
                other.span(),
                "sh_entry: function body must end in a ShEntry { … } literal",
            ));
        }
    };
    let sh_entry_struct = match tail_expr {
        syn::Expr::Struct(struct_expr) => struct_expr,
        other => {
            return Err(syn::Error::new(
                other.span(),
                "sh_entry: function body must end in a ShEntry { … } literal",
            ));
        }
    };

    // Find the `info: …` field initialiser.
    let info_field = sh_entry_struct
        .fields
        .iter()
        .find(|field| matches!(&field.member, syn::Member::Named(name) if name == "info"))
        .ok_or_else(|| {
            syn::Error::new(
                sh_entry_struct.span(),
                "sh_entry: ShEntry literal is missing the `info` field",
            )
        })?;

    // The initialiser must itself be `EntryInfo { … }`.
    let entry_info_struct = match &info_field.expr {
        syn::Expr::Struct(struct_expr) => struct_expr,
        other => {
            return Err(syn::Error::new(
                other.span(),
                "sh_entry: `info` initialiser must be an EntryInfo { … } literal",
            ));
        }
    };

    // Extract each of the three string-literal fields.
    let name = take_string_literal(entry_info_struct, "name")?;
    let short_description = take_string_literal(entry_info_struct, "short_description")?;
    let long_description = take_string_literal(entry_info_struct, "long_description")?;

    Ok(EntryInfoLiterals {
        name,
        short_description,
        long_description,
    })
}

fn take_string_literal(struct_expr: &syn::ExprStruct, field_name: &str) -> syn::Result<String> {
    let field = struct_expr
        .fields
        .iter()
        .find(|field| matches!(&field.member, syn::Member::Named(name) if name == field_name))
        .ok_or_else(|| {
            syn::Error::new(
                struct_expr.span(),
                format!("sh_entry: EntryInfo literal is missing `{field_name}`"),
            )
        })?;
    let lit = match &field.expr {
        syn::Expr::Lit(lit_expr) => lit_expr,
        other => {
            return Err(syn::Error::new(
                other.span(),
                format!("sh_entry: `{field_name}` must be a string literal"),
            ));
        }
    };
    let lit_str = match &lit.lit {
        syn::Lit::Str(lit_str) => lit_str,
        other => {
            return Err(syn::Error::new(
                other.span(),
                format!("sh_entry: `{field_name}` must be a string literal"),
            ));
        }
    };
    Ok(lit_str.value())
}

use std::path::{Path, PathBuf};

fn resolve_entries_info_dir() -> PathBuf {
    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        let target = Path::new(&out_dir)
            .ancestors()
            .nth(4)
            .expect("OUT_DIR has at least four ancestors");
        return target.join(".dusk_sh_entries");
    }
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        return Path::new(&target_dir).join(".dusk_sh_entries");
    }
    panic!(
        "sh_entry: neither OUT_DIR nor CARGO_TARGET_DIR is set; cannot \
         locate the cargo target directory"
    );
}

fn write_entries_info(crate_name: &str, fn_name: &str, fields: &EntryInfoLiterals) {
    let entries_info_dir = resolve_entries_info_dir();
    std::fs::create_dir_all(&entries_info_dir)
        .unwrap_or_else(|error| panic!("sh_entry: create {}: {error}", entries_info_dir.display()));

    let payload = serde_json::json!({
        "name": fields.name,
        "short_description": fields.short_description,
        "long_description": fields.long_description,
    });
    let body = serde_json::to_vec_pretty(&payload)
        .expect("sh_entry: serde_json cannot fail on a Value built from owned strings");

    let final_path = entries_info_dir.join(format!("{crate_name}__{fn_name}.json"));
    // Skip the write when the on-disk content is already identical. A
    // rename always bumps mtime, and dusk_llm/build.rs has a
    // rerun-if-changed on these files; rewriting unchanged JSON would
    // retrigger its (expensive) warm-up snapshot regeneration on every
    // recompile of any program crate.
    if std::fs::read(&final_path).is_ok_and(|existing| existing == body) {
        return;
    }
    let temp_path = entries_info_dir.join(format!("{crate_name}__{fn_name}.json.tmp"));
    std::fs::write(&temp_path, &body)
        .unwrap_or_else(|error| panic!("sh_entry: write {}: {error}", temp_path.display()));
    std::fs::rename(&temp_path, &final_path).unwrap_or_else(|error| {
        panic!(
            "sh_entry: rename {} -> {}: {error}",
            temp_path.display(),
            final_path.display(),
        )
    });
}
