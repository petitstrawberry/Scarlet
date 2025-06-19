// kernel/build.rs

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use quote::quote;
use serde::Serialize;
use serde_json;
use prettyplease;
use syn::{
    FnArg, ImplItem, ItemFn, ItemImpl, ItemMod, PathArguments,
    ReturnType, Signature, Type, Visibility, visit::Visit,
};
use heck::ToSnakeCase;

// Add serde::Serialize
#[derive(Clone)]
struct ApiFunction {
    signature: Signature,
    full_path: String,
    // The type this method belongs to, if any.
    self_type: Option<String>,
}

#[derive(Serialize)]
struct SerializableApiFunction {
    signature_str: String,
    full_path: String,
    self_type: Option<String>,
}

#[derive(Default)]
struct FileVisitor {
    pub_functions: Vec<ApiFunction>,
    submodules: Vec<PathBuf>,
    current_dir: PathBuf,
    module_path: String,
}

// is_abi_safe and is_type_abi_safe functions remain the same...
fn is_abi_safe(sig: &syn::Signature) -> bool {
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() { return false; }
    if sig.asyncness.is_some() { return false; }
    for arg in &sig.inputs {
        if let syn::FnArg::Typed(pat_type) = arg {
            if !is_type_abi_safe(&pat_type.ty) { return false; }
        }
    }
    if let ReturnType::Type(_, ty) = &sig.output {
        if !is_type_abi_safe(ty) { return false; }
    }
    true
}

fn is_type_abi_safe(ty: &Type) -> bool {
    match ty {
        Type::ImplTrait(_) => false,
        Type::Path(type_path) => {
            if let Some(segment) = type_path.path.segments.last() {
                if segment.ident == "Arguments" { return false; }
                if segment.ident == "Box" {
                    if let PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(Type::TraitObject(_))) = args.args.first() {
                            return false;
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
                self_type: None, // This is a free-standing function
            });
        }
    }

    fn visit_item_impl(&mut self, i: &'ast ItemImpl) {
        if i.trait_.is_some() { return; } 
        if !i.generics.params.is_empty() {
            let self_ty_str = quote!(#i.self_ty).to_string();
            println!("cargo:warning=>>> Skipping all methods for generic impl: impl<{}> {}", quote!(#i.generics.params), self_ty_str);
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
                    
                    let mut transformed_sig = sig.clone();
                    let mut self_type_for_func = None;
                    if let Some(FnArg::Receiver(receiver)) = transformed_sig.inputs.first_mut() {
                        if receiver.reference.is_none() && receiver.mutability.is_none() {
                             println!("cargo:warning=Skipping method taking 'self' by value: {}", &full_path);
                             continue;
                        }
                        self_type_for_func = Some(self_ty_str.clone());
                    }

                    self.pub_functions.push(ApiFunction {
                        signature: sig.clone(), // Store original signature
                        full_path,
                        self_type: self_type_for_func,
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
        // ... (rest of the file processing logic is the same) ...
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
    let mut sorted_functions: Vec<_> = all_pub_functions.values().collect();
    sorted_functions.sort_by_key(|f| &f.full_path);

    // --- Generate CoreApiTable for kernel ---
    generate_api_table_for_kernel(&sorted_functions);
    
    // --- Generate JSON for macros ---
    generate_json_for_macros(&sorted_functions);
}

fn generate_api_table_for_kernel(functions: &[&ApiFunction]) {
    let mut table_fields = Vec::new();
    let mut table_inits = Vec::new();

    for func in functions {
        let field_name_str = func.full_path
            .strip_prefix("crate::").unwrap_or(&func.full_path)
            .replace("::", "_").to_snake_case();
        let field_name = syn::Ident::new(&field_name_str, proc_macro2::Span::call_site());
        
        let mut fn_sig = func.signature.clone();
        
        // Transform `&self` etc. into an explicit pointer for the function pointer type
        if let Some(FnArg::Receiver(receiver)) = fn_sig.inputs.first_mut() {
             if receiver.reference.is_none() && receiver.mutability.is_none() { continue; }
             let self_ty: Type = syn::parse_str(&func.self_type.as_ref().unwrap()).unwrap();
             let receiver_ty = if receiver.mutability.is_some() {
                 quote! { *mut #self_ty }
             } else {
                 quote! { *const #self_ty }
             };
             fn_sig.inputs[0] = syn::parse_quote! { this: #receiver_ty };
        }
        
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
        #[repr(C)]
        pub struct CoreApiTable { #(#table_fields,)* }
        
        #[cfg(not(feature = "api-only"))]
        static KERNEL_CORE_API: CoreApiTable = CoreApiTable { #(#table_inits,)* };
        
        #[no_mangle]
        #[cfg(not(feature = "api-only"))]
        pub extern "C" fn get_core_api_table() -> *const CoreApiTable { &KERNEL_CORE_API }
    };
    
    // Parse the generated code into a syntax tree and format it
    let parsed_file = syn::parse_file(&generated_code.to_string()).unwrap();
    let formatted_code = prettyplease::unparse(&parsed_file);
    
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("core_api_table.rs");
    fs::write(&dest_path, formatted_code).unwrap();
    println!("cargo:rustc-env=CORE_API_TABLE_PATH={}", dest_path.display());
}

fn generate_json_for_macros(functions: &[&ApiFunction]) {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("core_api.json");
    // Convert to serializable format
    let serializable_functions: Vec<SerializableApiFunction> = functions.iter().map(|f| {
        let sig = &f.signature;
        let sig_tokens = quote!(#sig);
        let sig_str = sig_tokens.to_string();
        
        SerializableApiFunction {
            signature_str: sig_str,
            full_path: f.full_path.clone(),
            self_type: f.self_type.clone(),
        }
    }).collect();
    // Serialize the collected function data to JSON
    let json_data = serde_json::to_string_pretty(&serializable_functions).unwrap();
    fs::write(dest_path, json_data).unwrap();
    // Set an environment variable so the macro crate can find this file
    println!("cargo:rustc-env=SCARLET_API_JSON_PATH={}", Path::new(&out_dir).join("core_api.json").display());
}

