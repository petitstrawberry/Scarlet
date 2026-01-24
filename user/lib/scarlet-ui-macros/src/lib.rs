//! ScarletUI Macros - Procedural macros for ScarletUI
//!
//! This crate provides derive macros for ScarletUI traits.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Data, DataStruct, Fields, TypePath};
use syn::punctuated::Punctuated;

/// Derive macro for View trait
///
/// # Example
///
/// ```ignore
/// #[derive(View, Clone)]
/// struct CounterApp {
///     count: State<i32>,
/// }
/// ```
#[proc_macro_derive(View)]
pub fn derive_view(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // Parse struct fields to find State<T> fields
    let state_fields = match &input.data {
        Data::Struct(DataStruct { fields, .. }) => {
            extract_state_fields(fields)
        }
        _ => {
            // For enums or other types, return empty
            Punctuated::new()
        }
    };

    // Generate code to collect State fields
    let collect_state_fields: std::vec::Vec<proc_macro2::TokenStream> = state_fields
        .iter()
        .map(|field_name| {
            let field_ident = quote::format_ident!("{}", field_name);
            quote! {
                vec.push(&self.#field_ident as &dyn ::scarlet_ui::state::Listenable);
            }
        })
        .collect();

    let expanded = quote! {
        impl ::scarlet_ui::view::View for #name {
            fn create_element(&self) -> alloc::boxed::Box<dyn ::scarlet_ui::element::Element> {
                // Create a ComponentElement to wrap this View
                alloc::boxed::Box::new(::scarlet_ui::element::ComponentElement::new(self.clone()))
            }

            fn listenables(&self) -> alloc::vec::Vec<&dyn ::scarlet_ui::state::Listenable> {
                let mut vec = alloc::vec::Vec::new();
                #(#collect_state_fields)*
                vec
            }

            fn as_any(&self) -> &dyn core::any::Any {
                self
            }
        }
    };

    proc_macro::TokenStream::from(expanded)
}

/// Extract field names that are of type State<T>
fn extract_state_fields(fields: &Fields) -> Punctuated<syn::Ident, syn::token::Comma> {
    let mut state_fields = Punctuated::new();

    if let Fields::Named(named_fields) = fields {
        for field in &named_fields.named {
            let field_name = field.ident.as_ref().unwrap();

            // Check if field type is State<T>
            if is_state_type(&field.ty) {
                state_fields.push(field_name.clone());
            }
        }
    }

    state_fields
}

/// Check if a type is State<T> (either scarlet_ui::state::State or just State)
fn is_state_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(TypePath { path, .. }) = ty {
        // Get the last segment of the path
        if let Some(last_segment) = path.segments.last() {
            // Check if it's "State"
            if last_segment.ident == "State" {
                // Optionally check if it's from scarlet_ui::state module
                // For simplicity, we accept any "State" identifier
                return true;
            }
        }
    }
    false
}
