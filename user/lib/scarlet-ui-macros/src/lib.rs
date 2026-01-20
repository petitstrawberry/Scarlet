use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

/// Derive macro for View trait
/// Automatically implements View for structs
#[proc_macro_derive(View)]
pub fn derive_view(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let expanded = quote! {
        impl ::scarlet_ui::View for #name {
            fn type_id(&self) -> ::scarlet_ui::std::any::TypeId {
                ::scarlet_ui::std::any::TypeId::of::<Self>()
            }

            fn type_name(&self) -> &'static str {
                ::scarlet_ui::std::any::type_name::<Self>()
            }

            fn build(&self) -> ::scarlet_ui::std::boxed::Box<dyn ::scarlet_ui::RenderNode> {
                // Users should implement this method
                ::scarlet_ui::std::panic!("View::build() not implemented for {}. Use #[view] attribute macro instead.", ::scarlet_ui::std::any::type_name::<Self>());
            }

            fn as_any(&self) -> &dyn ::scarlet_ui::std::any::Any {
                self
            }
        }
    };

    TokenStream::from(expanded)
}

/// Attribute macro for View structs
/// Example:
/// ```rust
/// #[view]
/// struct MyView {
///     #[state] count: State<i32>,
///     title: String,
/// }
/// ```
#[proc_macro_attribute]
pub fn view(_attrs: TokenStream, input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let expanded = quote! {
        #[derive(::scarlet_ui::View, Clone)]
        #input
    };

    TokenStream::from(expanded)
}

/// Macro for VStack - simplified version
/// Example:
/// ```rust
/// let stack = scarlet_ui::vstack! {
///     ::scarlet_ui::VStack::new()
/// };
/// ```
#[proc_macro]
pub fn vstack(_input: TokenStream) -> TokenStream {
    let expanded = quote! {
        ::scarlet_ui::VStack::new()
    };

    TokenStream::from(expanded)
}

/// Macro for HStack - placeholder
#[proc_macro]
pub fn hstack(_input: TokenStream) -> TokenStream {
    let expanded = quote! {
        ::scarlet_ui::std::compile_error!("HStack not yet implemented")
    };

    TokenStream::from(expanded)
}

/// Macro for ZStack - placeholder
#[proc_macro]
pub fn zstack(_input: TokenStream) -> TokenStream {
    let expanded = quote! {
        ::scarlet_ui::std::compile_error!("ZStack not yet implemented")
    };

    TokenStream::from(expanded)
}
