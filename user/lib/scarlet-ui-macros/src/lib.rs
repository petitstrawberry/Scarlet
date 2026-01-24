//! ScarletUI Macros - Procedural macros for ScarletUI
//!
//! This crate provides derive macros for ScarletUI traits.

#![no_std]

extern crate alloc;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

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

    let expanded = quote! {
        impl ::scarlet_ui::view::View for #name {
            fn create_element(&self) -> alloc::boxed::Box<dyn ::scarlet_ui::element::Element> {
                // Create a ComponentElement to wrap this View
                alloc::boxed::Box::new(::scarlet_ui::element::ComponentElement::new(self.clone()))
            }

            fn listenables(&self) -> alloc::vec::Vec<&dyn ::scarlet_ui::state::Listenable> {
                // Collect State fields (would need field analysis in full implementation)
                alloc::vec::Vec::new()
            }

            fn as_any(&self) -> &dyn core::any::Any {
                self
            }
        }
    };

    proc_macro::TokenStream::from(expanded)
}
