//! Procedural macros for declarative UI in ScarletUI
//!
//! This crate provides SwiftUI-style declarative macros for building UIs.
//!
//! # view! Macro
//!
//! The `view!` macro enables declarative, SwiftUI-style view construction:
//!
//! ```rust
//! use scarlet_ui::view;
//!
//! let view = view! {
//!     VStack(spacing: 16) {
//!         Text("Hello, World!")
//!             .set_font(FontConfig { size: 24, ..Default::default() })
//!             .set_color(Color::WHITE)
//!
//!         Button("Click me")
//!             .set_action(Arc::new(|| println!("Clicked!")))
//!             .set_colors(Color::BUTTON_NORMAL, Color::BUTTON_HOVER, Color::BUTTON_PRESSED)
//!     }
//! };
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, 
    DeriveInput, 
    Expr, 
    Ident, 
    Result, 
    Token,
    braced, 
    parenthesized,
    punctuated::Punctuated,
};

/// Derive macro for creating a stateful view
///
/// # Example
///
/// ```rust
/// use scarlet_ui_macros::View;
///
/// #[derive(View)]
/// struct CounterView {
///     #[state]
///     count: i32,
/// }
/// ```
#[proc_macro_derive(View, attributes(state, binding))]
pub fn derive_view(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    
    // Generate state field wrappers
    let expanded = quote! {
        impl Default for #name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
    
    TokenStream::from(expanded)
}

/// A view expression in the DSL
enum ViewExpr {
    /// Simple view: `Label("text")`
    Simple {
        name: Ident,
        args: Vec<Expr>,
    },
    /// Container view: `VStack { ... }`
    Container {
        name: Ident,
        args: Vec<(Ident, Expr)>, // named args like spacing: 16
        children: Vec<ViewExpr>,
    },
    /// Method chain: `Label("text").color(Color::WHITE)`
    Chained {
        base: Box<ViewExpr>,
        methods: Vec<MethodCall>,
    },
}

struct MethodCall {
    name: Ident,
    args: Vec<Expr>,
}

impl Parse for ViewExpr {
    fn parse(input: ParseStream) -> Result<Self> {
        // Parse the base view name
        let name: Ident = input.parse()?;
        
        // Parse constructor arguments if present
        let mut constructor_args = Vec::new();
        let mut named_args = Vec::new();
        
        if input.peek(syn::token::Paren) {
            let content;
            parenthesized!(content in input);
            
            // Parse comma-separated arguments
            while !content.is_empty() {
                // Check for named argument: `name: value`
                if content.peek(Ident) && content.peek2(Token![:]) {
                    let arg_name: Ident = content.parse()?;
                    let _: Token![:] = content.parse()?;
                    let arg_value: Expr = content.parse()?;
                    named_args.push((arg_name, arg_value));
                } else {
                    let arg: Expr = content.parse()?;
                    constructor_args.push(arg);
                }
                
                if content.peek(Token![,]) {
                    let _: Token![,] = content.parse()?;
                }
            }
        }
        
        // Check for children block
        let mut children = Vec::new();
        if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            
            while !content.is_empty() {
                children.push(content.parse()?);
            }
        }
        
        // Parse method chains
        let mut methods = Vec::new();
        while input.peek(Token![.]) {
            let _: Token![.] = input.parse()?;
            let method_name: Ident = input.parse()?;
            
            let mut method_args = Vec::new();
            if input.peek(syn::token::Paren) {
                let content;
                parenthesized!(content in input);
                let args: Punctuated<Expr, Token![,]> = 
                    Punctuated::parse_terminated(&content)?;
                method_args = args.into_iter().collect();
            }
            
            methods.push(MethodCall {
                name: method_name,
                args: method_args,
            });
        }
        
        // Build the appropriate ViewExpr
        let base = if !children.is_empty() || !named_args.is_empty() {
            ViewExpr::Container {
                name,
                args: named_args,
                children,
            }
        } else {
            ViewExpr::Simple {
                name,
                args: constructor_args,
            }
        };
        
        if methods.is_empty() {
            Ok(base)
        } else {
            Ok(ViewExpr::Chained {
                base: Box::new(base),
                methods,
            })
        }
    }
}

impl ViewExpr {
    fn to_tokens(&self) -> TokenStream2 {
        match self {
            ViewExpr::Simple { name, args } => {
                if args.is_empty() {
                    quote! { #name::new() }
                } else {
                    quote! { #name::new(#(#args),*) }
                }
            }
            ViewExpr::Container { name, args, children } => {
                let child_tokens: Vec<_> = children.iter()
                    .map(|c| {
                        let child_code = c.to_tokens();
                        quote! { .child(#child_code) }
                    })
                    .collect();

                let named_arg_tokens: Vec<_> = args.iter()
                    .map(|(n, v)| quote! { .#n(#v) })
                    .collect();

                quote! {
                    #name::new()
                        #(#named_arg_tokens)*
                        #(#child_tokens)*
                }
            }
            ViewExpr::Chained { base, methods } => {
                let base_tokens = base.to_tokens();
                let method_tokens: Vec<_> = methods.iter()
                    .map(|m| {
                        let name = &m.name;
                        let args = &m.args;
                        if args.is_empty() {
                            quote! { .#name() }
                        } else {
                            quote! { .#name(#(#args),*) }
                        }
                    })
                    .collect();

                quote! {
                    #base_tokens #(#method_tokens)*
                }
            }
        }
    }
}

/// Parse a full view builder input (may contain multiple top-level views)
struct ViewBuilderInput {
    views: Vec<ViewExpr>,
}

impl Parse for ViewBuilderInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut views = Vec::new();
        while !input.is_empty() {
            views.push(input.parse()?);
        }
        Ok(ViewBuilderInput { views })
    }
}

/// Macro for building view hierarchies declaratively
///
/// # Example
///
/// ```rust
/// use scarlet_ui::view_builder;
///
/// let view = view_builder! {
///     VStack(spacing: 16) {
///         Label("Hello")
///             .color(Color::BLACK)
///             .font_size(24)
///         
///         HStack(spacing: 8) {
///             Button("OK", || println!("OK"))
///             Button("Cancel", || println!("Cancel"))
///         }
///     }
/// };
/// ```
///
/// # Supported Syntax
///
/// - `ViewName(args...)` - Create a view with constructor arguments
/// - `ViewName(name: value, ...)` - Named arguments become method calls
/// - `ViewName { children... }` - Container views with children
/// - `.method(args...)` - Method chaining for modifiers
#[proc_macro]
pub fn view_builder(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ViewBuilderInput);
    
    if input.views.is_empty() {
        return quote! { compile_error!("view_builder! requires at least one view") }.into();
    }
    
    if input.views.len() == 1 {
        let tokens = input.views[0].to_tokens();
        return tokens.into();
    }
    
    // Multiple top-level views - wrap in a VStack
    let child_tokens: Vec<_> = input.views.iter()
        .map(|v| {
            let code = v.to_tokens();
            quote! { .child(#code) }
        })
        .collect();
    
    quote! {
        VStack::new() #(#child_tokens)*
    }.into()
}

/// Macro for building view hierarchies declaratively (alias for view_builder!)
///
/// # Example
///
/// ```rust
/// use scarlet_ui::view;
///
/// let view = view! {
///     VStack(spacing: 16) {
///         Text("Hello")
///             .set_color(Color::BLACK)
///
///         HStack(spacing: 8) {
///             Button("OK")
///             Button("Cancel")
///         }
///     }
/// };
/// ```
#[proc_macro]
pub fn view(input: TokenStream) -> TokenStream {
    view_builder(input)
}

/// Attribute macro for reactive state properties
///
/// This attribute can be applied to struct fields to wrap them with State<T>.
///
/// # Example
///
/// ```rust
/// struct MyView {
///     #[state]
///     counter: i32,  // becomes State<i32>
/// }
/// ```
#[proc_macro_attribute]
pub fn state(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // For now, pass through unchanged
    // A full implementation would transform the field type
    item
}

/// Attribute macro for binding properties
///
/// This attribute can be applied to struct fields to mark them as bindings.
///
/// # Example
///
/// ```rust
/// struct MyView {
///     #[binding]
///     text: String,  // expects to receive Binding<String>
/// }
/// ```
#[proc_macro_attribute]
pub fn binding(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // For now, pass through unchanged
    item
}
