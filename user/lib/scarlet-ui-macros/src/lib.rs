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
///
/// This macro generates:
/// - `impl View for CounterApp` - creates ComponentElement, collects listenables
/// - `impl Default for CounterApp` - auto-initializes State fields with auto-generated StateId
///
/// Users can implement their own `new()` method and use `Default::default()`:
/// ```ignore
/// impl CounterApp {
///     pub fn new(custom_value: i32) -> Self {
///         Self {
///             count: State::new(StateId::new(0), custom_value),
///         }
/// }
/// }
/// ```
#[proc_macro_derive(View)]
pub fn derive_view(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // Parse struct fields to find State<T> fields
    let (state_fields, state_indices) = match &input.data {
        Data::Struct(DataStruct { fields, .. }) => {
            extract_state_fields_with_indices(fields)
        }
        _ => {
            // For enums or other types, return empty
            (Punctuated::new(), Vec::new())
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

    // Generate Default implementation that initializes State fields with auto-generated StateId
    let default_init: std::vec::Vec<proc_macro2::TokenStream> = state_indices
        .iter()
        .zip(state_fields.iter())
        .map(|(idx, field_name)| {
            let field_ident = quote::format_ident!("{}", field_name);
            // Use State::initial for types with Default (State<T> inner type)
            quote! {
                #field_ident: ::scarlet_ui::state::State::initial(
                    ::scarlet_ui::state::StateId::new(#idx)
                ),
            }
        })
        .collect();

    let has_state_fields = !state_fields.is_empty();

    let default_impl = if has_state_fields {
        quote! {
            impl core::default::Default for #name {
                fn default() -> Self {
                    Self {
                        #(#default_init)*
                    }
                }
            }
        }
    } else {
        quote! {}
    };

    let expanded = quote! {
        #default_impl

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

/// Extract field names that are of type State<T> (without types)
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

/// Extract field names and indices for State<T> fields (with auto-incrementing IDs)
fn extract_state_fields_with_indices(fields: &Fields) -> (Punctuated<syn::Ident, syn::token::Comma>, Vec<usize>) {
    let mut state_fields = Punctuated::new();
    let mut state_indices = Vec::new();
    let mut counter = 0usize;

    if let Fields::Named(named_fields) = fields {
        for field in &named_fields.named {
            let field_name = field.ident.as_ref().unwrap();

            // Check if field type is State<T>
            if is_state_type(&field.ty) {
                state_fields.push(field_name.clone());
                state_indices.push(counter);
                counter += 1;
            }
        }
    }

    (state_fields, state_indices)
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
