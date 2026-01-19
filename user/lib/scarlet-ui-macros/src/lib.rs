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
    Data,
    DataStruct,
    DeriveInput,
    Expr,
    Fields,
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

/// Attribute macro for creating observable types
///
/// This attribute implements the Observable trait for a struct.
/// Applied to a struct, it:
/// - Adds a `notifier: ObservableNotifier` field
/// - Generates setter methods for each `#[published]` field
/// - Implements the `Observable` trait
///
/// # Example
///
/// ```rust
/// use scarlet_ui::observable;
///
/// #[observable]
/// struct UserSettings {
///     #[published]
///     username: String,
///
///     #[published]
///     is_premium: bool,
///
///     count: u32,  // Not published, no automatic notification
/// }
/// ```
///
/// This expands to:
/// ```ignore
/// struct UserSettings {
///     notifier: ObservableNotifier,
///     username: String,
///     is_premium: bool,
///     count: u32,
/// }
///
/// impl UserSettings {
///     fn set_username(&mut self, username: String) {
///         self.username = username;
///         self.notifier.notify();
///     }
///
///     fn set_is_premium(&mut self, is_premium: bool) {
///         self.is_premium = is_premium;
///         self.notifier.notify();
///     }
/// }
///
/// impl Observable for UserSettings {
///     type SubscriptionId = usize;
///     fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> Self::SubscriptionId {
///         self.notifier.subscribe(observer)
///     }
///     fn unsubscribe(&self, id: Self::SubscriptionId) {
///         self.notifier.unsubscribe(id)
///     }
///     fn notify(&self) {
///         self.notifier.notify()
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn observable(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as DeriveInput);

    // Process the struct
    let struct_name = &input.ident;
    let vis = &input.vis;

    // Get the fields and extract published ones
    let fields = match &mut input.data {
        Data::Struct(DataStruct {
            fields: Fields::Named(fields),
            ..
        }) => &mut fields.named,
        _ => {
            return quote::quote! {
                compile_error!("#[observable] only supports structs with named fields");
            }
            .into();
        }
    };

    // Collect published field information
    let mut published_fields = Vec::new();

    for field in fields.iter() {
        let field_name = field.ident.as_ref().expect("named field");
        let field_type = &field.ty;
        let field_vis = &field.vis;

        // Check if this field has #[published] attribute
        let is_published = field
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("published"));

        if is_published {
            published_fields.push((field_name.clone(), field_type.clone(), field_vis.clone()));
        }
    }

    // Add data fields for published fields
    for (field_name, field_type, field_vis) in &published_fields {
        let data_field_name = Ident::new(
            &format!("_{}_data", field_name),
            proc_macro2::Span::call_site()
        );
        fields.push(syn::Field {
            attrs: vec![],
            vis: field_vis.clone(),
            ident: Some(data_field_name),
            colon_token: Some(syn::token::Colon::default()),
            ty: syn::Type::Verbatim(quote::quote! {
                alloc::sync::Arc<DataContext<#field_type>>
            }),
            mutability: syn::FieldMutability::None,
        });
    }

    // Make fields mutable again
    let _fields = match &mut input.data {
        Data::Struct(DataStruct {
            fields: Fields::Named(fields),
            ..
        }) => &mut fields.named,
        _ => unreachable!(),
    };

    // Generate getter and setter methods
    let mut getter_methods = Vec::new();
    let mut setter_methods = Vec::new();

    for (field_name, field_type, _field_vis) in &published_fields {
        let data_field_name = Ident::new(
            &format!("_{}_data", field_name),
            proc_macro2::Span::call_site()
        );
        let getter_name = Ident::new(
            &format!("_{}", field_name),
            proc_macro2::Span::call_site()
        );

        // Generate getter method (_field_name)
        getter_methods.push(quote::quote! {
            #vis fn #getter_name(&self) -> alloc::sync::Arc<DataContext<#field_type>> {
                alloc::sync::Arc::clone(&self.#data_field_name)
            }
        });

        // Generate setter method (field_name)
        setter_methods.push(quote::quote! {
            #vis fn #field_name(&mut self, value: #field_type) {
                self.#field_name = value.clone();
                self.#data_field_name.set(value);
                self.notifier.notify();
            }
        });
    }

    // Generate field initialization for data fields
    let data_field_inits: Vec<_> = published_fields.iter().map(|(field_name, _field_type, _vis)| {
        let data_field_name = Ident::new(
            &format!("_{}_data", field_name),
            proc_macro2::Span::call_site()
        );
        quote::quote! {
            #data_field_name: alloc::sync::Arc::new(DataContext::new(Default::default()))
        }
    }).collect();

    // Generate the expanded struct
    let expanded = quote::quote! {
        #input

        impl #struct_name {
            /// Create a new instance with default values
            #vis fn new() -> Self {
                Self {
                    notifier: ::scarlet_ui::ObservableNotifier::new(),
                    #(#data_field_inits,)*
                    ..Default::default()
                }
            }

            #(#getter_methods)*

            #(#setter_methods)*
        }

        impl ::scarlet_ui::Observable for #struct_name {
            type SubscriptionId = usize;

            fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> Self::SubscriptionId {
                self.notifier.subscribe(observer)
            }

            fn unsubscribe(&self, id: Self::SubscriptionId) {
                self.notifier.unsubscribe(id)
            }

            fn notify(&self) {
                self.notifier.notify()
            }
        }
    };

    TokenStream::from(expanded)
}

/// Field attribute for marking fields as published
///
/// This attribute marks a field in an `#[observable]` struct as published.
/// A setter method will be generated that automatically notifies observers
/// when the field changes.
///
/// # Example
///
/// ```rust
/// #[observable]
/// struct Settings {
///     #[published]
///     username: String,  // Will have set_username() method
///
///     count: u32,        // No setter, no auto-notify
/// }
/// ```
#[proc_macro_attribute]
pub fn published(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass through - processing is handled by #[observable]
    item
}

/// Attribute macro for generating bind methods for View controls
///
/// This attribute automatically adds data binding support to control structs.
/// It:
/// - Adds `{field}_data: Option<Arc<DataContext<T>>>` fields for each `#[bind]` field
/// - Generates `bind_{field}(data: &Arc<DataContext<T>>) -> Self` methods
/// - Automatically subscribes to the DataContext using `self.id()`
///
/// # Example
///
/// ```rust
/// use scarlet_ui::bindable;
///
/// #[bindable]
/// struct TextField {
///     #[bind]
///     text: String,
///     #[bind]
///     is_valid: bool,
///     placeholder: String,
/// }
/// ```
///
/// This expands to:
/// ```ignore
/// struct TextField {
///     text: String,
///     is_valid: bool,
///     placeholder: String,
///     // Auto-generated fields:
///     text_data: Option<Arc<DataContext<String>>>,
///     is_valid_data: Option<Arc<DataContext<bool>>>,
/// }
///
/// impl TextField {
///     pub fn bind_text(mut self, data: &Arc<DataContext<String>>) -> Self {
///         self.text = data.get();
///         data.subscribe(self.id());
///         self.text_data = Some(Arc::clone(data));
///         self
///     }
///
///     pub fn bind_is_valid(mut self, data: &Arc<DataContext<bool>>) -> Self {
///         self.is_valid = data.get();
///         data.subscribe(self.id());
///         self.is_valid_data = Some(Arc::clone(data));
///         self
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn bindable(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as DeriveInput);
    let struct_name = &input.ident;
    let vis = &input.vis;

    // Get the fields
    let fields = match &mut input.data {
        Data::Struct(DataStruct {
            fields: Fields::Named(fields),
            ..
        }) => &mut fields.named,
        _ => {
            return quote::quote! {
                compile_error!("#[bindable] only supports structs with named fields");
            }
            .into();
        }
    };

    // Collect #[bind] fields and original field info
    let mut bind_fields = Vec::new();
    let mut original_field_names = Vec::new();
    let mut original_field_types = Vec::new();

    for field in fields.iter() {
        let field_name = field.ident.as_ref().expect("named field");
        let field_type = &field.ty;
        original_field_names.push(field_name.clone());
        original_field_types.push(field_type.clone());

        // Check if this field has #[bind] attribute
        if field.attrs.iter().any(|attr| attr.path().is_ident("bind")) {
            bind_fields.push((field_name.clone(), field_type.clone()));
        }
    }

    // Remove #[bind] attributes from fields
    for field in fields.iter_mut() {
        field.attrs.retain(|attr| !attr.path().is_ident("bind"));
    }

    // Generate bind methods and collect data field info
    let mut bind_methods = Vec::new();
    let mut data_field_names = Vec::new();

    for (field_name, field_type) in &bind_fields {
        let data_field_name = Ident::new(
            &format!("{}_data", field_name),
            proc_macro2::Span::call_site()
        );
        let bind_method_name = Ident::new(
            &format!("bind_{}", field_name),
            proc_macro2::Span::call_site()
        );

        data_field_names.push(data_field_name.clone());

        // Add data field to struct
        fields.push(syn::Field {
            attrs: vec![],
            vis: vis.clone(),
            ident: Some(data_field_name.clone()),
            colon_token: Some(syn::token::Colon::default()),
            ty: syn::Type::Verbatim(quote::quote! {
                Option<alloc::sync::Arc<DataContext<#field_type>>>
            }),
            mutability: syn::FieldMutability::None,
        });

        // Generate bind method
        bind_methods.push(quote::quote! {
            #vis fn #bind_method_name(mut self, data: &alloc::sync::Arc<DataContext<#field_type>>) -> Self {
                self.#field_name = data.get();
                data.subscribe(self.id());
                self.#data_field_name = Some(alloc::sync::Arc::clone(data));
                self
            }
        });
    }

    // Generate field initialization for Default impl
    let field_defaults: Vec<TokenStream2> = original_field_names.iter()
        .zip(original_field_types.iter())
        .map(|(name, ty)| {
            quote::quote! {
                #name: <#ty as ::core::default::Default>::default()
            }
        })
        .collect();

    let data_field_defaults: Vec<TokenStream2> = data_field_names.iter()
        .map(|name| {
            quote::quote! {
                #name: ::core::default::Default::default()
            }
        })
        .collect();

    // Generate the expanded code with Default implementation
    let expanded = quote::quote! {
        #input

        impl #struct_name {
            #(#bind_methods)*
        }

        impl ::core::default::Default for #struct_name {
            fn default() -> Self {
                Self {
                    #(#field_defaults,)*
                    #(#data_field_defaults,)*
                }
            }
        }
    };

    TokenStream::from(expanded)
}
