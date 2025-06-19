// kernel/build.rs

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use quote::quote;
use syn::{
    File, FnArg, ImplItem, Item, ItemFn, ItemImpl, ItemMod, Pat, PatType, PathArguments,
    ReturnType, Signature, Type, TypePath, Visibility, visit::Visit,
};
// heckクレートをインポートし、スネークケースへの変換を利用する
use heck::ToSnakeCase;

/// A struct to hold information about a found public API item.
#[derive(Clone)]
struct ApiFunction {
    // The signature needs to be mutable to handle method-to-function conversion.
    signature: Signature,
    full_path: String,
}

/// A visitor that extracts public API items and submodule declarations from a single file.
#[derive(Default)]
struct FileVisitor {
    pub_functions: Vec<ApiFunction>,
    submodules: Vec<PathBuf>,
    current_dir: PathBuf,
    module_path: String,
}

/// Checks if a function signature is compatible with `extern "C"` ABI.
fn is_abi_safe(sig: &syn::Signature) -> bool {
    // 1. Check for any generics (type, lifetime, const) or a where clause.
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        return false;
    }
    // 2. Check for `async` functions.
    if sig.asyncness.is_some() {
        return false;
    }
    // 3. Check arguments and return type for FFI-unsafe patterns.
    for arg in &sig.inputs {
        if let syn::FnArg::Typed(pat_type) = arg {
            if !is_type_abi_safe(&pat_type.ty) {
                return false;
            }
        }
    }
    if let ReturnType::Type(_, ty) = &sig.output {
        if !is_type_abi_safe(ty) {
            return false;
        }
    }
    true
}

/// Checks if a type can be safely passed across an FFI boundary.
fn is_type_abi_safe(ty: &Type) -> bool {
    match ty {
        Type::ImplTrait(_) => false,
        Type::Path(type_path) => {
            if let Some(segment) = type_path.path.segments.last() {
                // Heuristic check for types that are not FFI-safe by default.
                if segment.ident == "Arguments" { return false; }
                if segment.ident == "Box" {
                    if let PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(Type::TraitObject(_))) = args.args.first() {
                            return false; // Box<dyn Trait> is not FFI-safe.
                        }
                    }
                }
            }
            true
        }
        _ => true,
    }
}

impl<'ast> Visit<'ast> for FileVisitor {
    fn visit_item_mod(&mut self, i: &'ast ItemMod) {
        for attr in &i.attrs {
            if attr.path().is_ident("cfg") {
                if let Ok(meta) = attr.meta.require_list() {
                    if meta.tokens.to_string() == "test" {
                        println!("cargo:warning=>>> Skipping test module and its contents: {}", i.ident);
                        return;
                    }
                }
            }
        }
        
        if i.content.is_none() {
            let mod_name = i.ident.to_string();
            let path_rs = self.current_dir.join(format!("{}.rs", mod_name));
            let path_mod_rs = self.current_dir.join(&mod_name).join("mod.rs");
            if path_rs.exists() { self.submodules.push(path_rs); } 
            else if path_mod_rs.exists() { self.submodules.push(path_mod_rs); }
        } else {
            let original_path = self.module_path.clone();
            if self.module_path.is_empty() { self.module_path = i.ident.to_string(); } 
            else { self.module_path = format!("{}::{}", self.module_path, i.ident); }
            syn::visit::visit_item_mod(self, i);
            self.module_path = original_path;
        }
    }

    fn visit_item_fn(&mut self, i: &'ast ItemFn) {
        if let Visibility::Public(_) = i.vis {
            let sig = &i.sig;
            if !is_abi_safe(sig) {
                let name = &sig.ident;
                println!("cargo:warning=Skipping FFI-unsafe function from CoreApiTable: {}::{}", self.module_path, name);
                return;
            }

            let name = sig.ident.to_string();
            let full_path = if self.module_path.is_empty() {
                format!("crate::{}", name)
            } else {
                format!("crate::{}::{}", self.module_path, name)
            };

            self.pub_functions.push(ApiFunction {
                signature: sig.clone(),
                full_path,
            });
        }
    }

    fn visit_item_impl(&mut self, i: &'ast ItemImpl) {
        if i.trait_.is_some() { return; } // Skip trait impls

        // --- NEW: Generic Impl Block Exclusion ---
        // If the impl block itself has generics (e.g., `impl<'a> MyType<'a>`),
        // its methods are not FFI-safe. Skip the entire block.
        if !i.generics.params.is_empty() {
            let self_ty_str = quote!(#i.self_ty).to_string();
            println!(
                "cargo:warning=>>> Skipping all methods for generic impl: impl<{}> {}",
                quote!(#i.generics.params).to_string(),
                self_ty_str
            );
            return;
        }


        let self_ty = &i.self_ty;
        let self_ty_str = quote!(#self_ty).to_string();
        
        for item in &i.items {
            if let ImplItem::Fn(method) = item {
                if let Visibility::Public(_) = method.vis {
                    let sig = &method.sig;
                    let name = &sig.ident;
                    let full_path = if self.module_path.is_empty() {
                        format!("crate::{}::{}", self_ty_str, name)
                    } else {
                        format!("crate::{}::{}::{}", self.module_path, self_ty_str, name)
                    };

                    if !is_abi_safe(sig) {
                        println!("cargo:warning=Skipping FFI-unsafe method from CoreApiTable: {}", &full_path);
                        continue;
                    }
                    
                    // --- Receiver Transformation Logic ---
                    let mut transformed_sig = sig.clone();
                    if let Some(FnArg::Receiver(receiver)) = transformed_sig.inputs.first_mut() {
                        if receiver.reference.is_none() && receiver.mutability.is_none() {
                             println!("cargo:warning=Skipping method taking 'self' by value from CoreApiTable: {}", &full_path);
                             continue;
                        }

                        let receiver_ty = if receiver.mutability.is_some() {
                            quote! { *mut #self_ty }
                        } else {
                            quote! { *const #self_ty }
                        };
                        
                        let fn_arg = FnArg::Typed(PatType {
                            attrs: Vec::new(),
                            pat: Box::new(syn::parse_quote! { this }),
                            colon_token: Default::default(),
                            ty: Box::new(syn::parse_str(&receiver_ty.to_string()).unwrap()),
                        });
                        
                        transformed_sig.inputs[0] = fn_arg;
                    }

                    self.pub_functions.push(ApiFunction {
                        signature: transformed_sig,
                        full_path,
                    });
                }
            }
        }
        syn::visit::visit_item_impl(self, i);
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    
    let src_path = PathBuf::from("src");
    let crate_root = if src_path.join("lib.rs").exists() {
        src_path.join("lib.rs")
    } else if src_path.join("main.rs").exists() {
        src_path.join("main.rs")
    } else {
        panic!("Could not find crate root (src/lib.rs or src/main.rs)");
    };

    let mut files_to_process = vec![crate_root];
    let mut processed_files = HashSet::new();
    let mut all_pub_functions: HashMap<String, ApiFunction> = HashMap::new();

    while let Some(path) = files_to_process.pop() {
        if !processed_files.insert(path.clone()) { continue; }
        
        println!("cargo:rerun-if-changed={}", path.to_str().unwrap());
        let code = match fs::read_to_string(&path) { Ok(c) => c, Err(_) => continue };
        let ast = match syn::parse_file(&code) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Failed to parse file: {}. Error: {}", path.display(), e);
                continue;
            }
        };

        let path_from_src = path.strip_prefix(&src_path).unwrap();
        let mut module_path_str = path_from_src.with_extension("").to_string_lossy().to_string();
        if module_path_str.ends_with("/mod") || module_path_str.ends_with("\\mod") {
            module_path_str = PathBuf::from(&module_path_str).parent().unwrap().to_string_lossy().to_string();
        }
        if module_path_str == "lib" || module_path_str == "main" { module_path_str = String::new(); }
        let module_path = module_path_str.replace("/", "::").replace("\\", "::");

        let mut visitor = FileVisitor {
            current_dir: path.parent().unwrap().to_path_buf(),
            module_path, ..Default::default()
        };
        visitor.visit_file(&ast);

        for func in visitor.pub_functions {
            all_pub_functions.entry(func.full_path.clone()).or_insert(func);
        }
        files_to_process.extend(visitor.submodules);
    }

    // --- Code Generation Phase ---
    let mut table_fields = Vec::new();
    let mut table_inits = Vec::new();
    
    let mut sorted_functions: Vec<_> = all_pub_functions.values().collect();
    sorted_functions.sort_by_key(|f| &f.full_path);

    for func in sorted_functions {
        let field_name_str = func.full_path
            .strip_prefix("crate::")
            .unwrap_or(&func.full_path)
            .replace("::", "_")
            .to_snake_case();
        let field_name = syn::Ident::new(&field_name_str, proc_macro2::Span::call_site());
        
        let mut fn_sig = func.signature.clone();
        fn_sig.abi = Some(syn::parse_quote!(extern "C"));
        fn_sig.unsafety = Some(syn::parse_quote!(unsafe));

        let inputs = &fn_sig.inputs;
        let output = &fn_sig.output;
        let fn_pointer_type = quote! { fn(#inputs) #output };

        let full_path_tokens: proc_macro2::TokenStream = func.full_path.parse().unwrap();

        table_fields.push(quote! { pub #field_name: #fn_pointer_type });
        table_inits.push(quote! { #field_name: #full_path_tokens });
    }

    let generated_code = quote! {
        /// この構造体はビルド時に自動生成されます。
        /// カーネル内の全ての`pub fn`（C-ABI互換のもの）への関数ポインタを含みます。
        #[repr(C)]
        pub struct CoreApiTable {
            #(#table_fields,)*
        }

        /// APIテーブルの唯一の静的インスタンス。
        /// `api-only`ビルドではこの部分はコンパイルされません。
        #[cfg(not(feature = "api-only"))]
        static KERNEL_CORE_API: CoreApiTable = CoreApiTable {
            #(#table_inits,)*
        };
        
        /// コアモジュールがAPIテーブルへのポインタを取得するための、公開された唯一の関数。
        #[no_mangle]
        #[cfg(not(feature = "api-only"))]
        pub extern "C" fn get_core_api_table() -> *const CoreApiTable {
            &KERNEL_CORE_API
        }
    };

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("core_api_generated.rs");
    
    let generated_file = match syn::parse2::<syn::File>(generated_code.clone()) {
        Ok(file) => file,
        Err(e) => {
            panic!("Failed to parse generated code. This is a bug in build.rs. Error: {e}\n--- Generated Code ---\n{generated_code}");
        }
    };

    let formatted_code = prettyplease::unparse(&generated_file);
    fs::write(dest_path, formatted_code).unwrap();

    println!("cargo:warning=CoreApiTable generated successfully with proper formatting.");
}
