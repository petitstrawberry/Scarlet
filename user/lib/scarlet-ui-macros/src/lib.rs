use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, parse::Parse, Token};
use syn::parse::{ParseStream, Result};

/// Derive macro for View trait
/// Automatically implements View for structs with body() method
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
                self.body().build()
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

struct VStackInput {
    children: Vec<syn::Expr>,
}

impl Parse for VStackInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut children = Vec::new();

        while !input.is_empty() {
            let child = input.parse::<syn::Expr>()?;
            children.push(child);

            // Try to parse comma, but don't require it at the end
            if input.is_empty() {
                break;
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;

                // Check if this was a trailing comma
                if input.is_empty() {
                    break;
                }
            }
        }

        Ok(VStackInput { children })
    }
}

/// Macro for VStack
/// Example:
/// ```rust
/// let stack = scarlet_ui::vstack! {
///     Text::new("Hello"),
///     Button::new("Click")
/// };
/// ```
#[proc_macro]
pub fn vstack(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as VStackInput);

    let children: Vec<proc_macro2::TokenStream> = parsed.children
        .into_iter()
        .map(|expr| quote::quote!(#expr))
        .collect();

    let expanded = match children.len() {
        0 => return quote! {
            ::scarlet_ui::std::compile_error!("vstack! requires at least 2 children")
        }.into(),
        _ => quote! {
            ::scarlet_ui::VStack::new((#(#children),*))
        },
    };

    TokenStream::from(expanded)
}

/// Macro for HStack
#[proc_macro]
pub fn hstack(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as VStackInput);

    let children: Vec<proc_macro2::TokenStream> = parsed.children
        .into_iter()
        .map(|expr| quote::quote!(#expr))
        .collect();

    let expanded = match children.len() {
        0 => return quote! {
            ::scarlet_ui::std::compile_error!("hstack! requires at least 2 children")
        }.into(),
        _ => quote! {
            ::scarlet_ui::HStack::new((#(#children),*))
        },
    };

    TokenStream::from(expanded)
}

/// Macro for ZStack
#[proc_macro]
pub fn zstack(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as VStackInput);

    let children: Vec<proc_macro2::TokenStream> = parsed.children
        .into_iter()
        .map(|expr| quote::quote!(#expr))
        .collect();

    let expanded = match children.len() {
        0 => return quote! {
            ::scarlet_ui::std::compile_error!("zstack! requires at least 1 child")
        }.into(),
        _ => quote! {
            ::scarlet_ui::ZStack::new((#(#children),*))
        },
    };

    TokenStream::from(expanded)
}
