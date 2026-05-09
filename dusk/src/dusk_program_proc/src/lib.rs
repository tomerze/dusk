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

#[proc_macro_derive(Args, attributes(data))]
pub fn derive_args(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let fields = match &input.data {
        syn::Data::Struct(data) => match &data.fields {
            syn::Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input.ident,
                    "Args can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input.ident,
                "Args can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let data_fields: Vec<_> = fields
        .iter()
        .filter(|f| f.attrs.iter().any(|a| a.path().is_ident("data")))
        .collect();

    if data_fields.len() != 1 {
        return syn::Error::new_spanned(
            &input.ident,
            "Args derive requires exactly one field annotated with #[data]",
        )
        .to_compile_error()
        .into();
    }

    let data_field_ident = data_fields[0].ident.as_ref().unwrap();

    let expanded = quote::quote! {
        __derive_args!(
            #(#cfg_attrs)*
            [#impl_generics] [#struct_name #ty_generics] [#where_clause] [#data_field_ident]
        );
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
    let (impl_generics, _, where_clause) = input.generics.split_for_impl();
    let self_ty = &input.self_ty;
    let items = &input.items;
    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        __impl_args_rpc_server!(
            #(#cfg_attrs)*
            [#impl_generics] [#self_ty] [#where_clause]
            { #(#items)* }
        );
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

#[proc_macro_derive(Process, attributes(process_context))]
pub fn derive_process(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let struct_name = &input.ident;

    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let fields = match &input.data {
        syn::Data::Struct(data) => match &data.fields {
            syn::Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input.ident,
                    "Process can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input.ident,
                "Process can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let context_fields: Vec<_> = fields
        .iter()
        .filter(|f| f.attrs.iter().any(|a| a.path().is_ident("process_context")))
        .collect();

    if context_fields.len() != 1 {
        return syn::Error::new_spanned(
            &input.ident,
            "Process derive requires exactly one field annotated with #[process_context]",
        )
        .to_compile_error()
        .into();
    }

    let context_field = context_fields[0];
    let field_name = context_field.ident.as_ref().unwrap();

    // Verify the field type is ProcessContext
    let ty = &context_field.ty;
    let type_str = quote::quote!(#ty).to_string();
    if !type_str.contains("ProcessContext") {
        return syn::Error::new_spanned(
            ty,
            "field annotated with #[process_context] must be of type ProcessContext",
        )
        .to_compile_error()
        .into();
    }

    let expanded = quote::quote! {
        __derive_process!(#(#cfg_attrs)* #struct_name, #field_name);
    };

    TokenStream::from(expanded)
}

#[proc_macro_derive(Portal)]
pub fn derive_portal(item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as syn::DeriveInput);
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        #(#cfg_attrs)*
        impl #impl_generics dusk_program::dusk_capnp::dusk_capnp::portal::Server
            for #struct_name #ty_generics #where_clause
        {
            fn program_id(
                &mut self,
                _params: dusk_program::dusk_capnp::dusk_capnp::portal::ProgramIdParams,
                mut results: dusk_program::dusk_capnp::dusk_capnp::portal::ProgramIdResults,
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
    let (impl_generics, _, where_clause) = input.generics.split_for_impl();
    let self_ty = &input.self_ty;
    let items = &input.items;
    let cfg_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("cfg"))
        .collect();

    let expanded = quote::quote! {
        __impl_portal_rpc_server!(
            #(#cfg_attrs)*
            [#impl_generics] [#self_ty] [#where_clause]
            { #(#items)* }
        );
    };

    TokenStream::from(expanded)
}
