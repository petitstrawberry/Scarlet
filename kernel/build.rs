// kernel/build.rs

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use syn::{File, Item, ItemMod, Visibility, visit::Visit};

/// A struct to hold information about a found public API item.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ApiItem {
    name: String,
    full_path: String,
}

/// A visitor that extracts public API items and submodule declarations from a single file.
#[derive(Default)]
struct FileVisitor {
    // API items found in the current file.
    pub_items: Vec<ApiItem>,
    // Submodules declared in the current file (`mod foo;`) that need to be visited next.
    submodules: Vec<PathBuf>,
    // The directory of the file being visited, for resolving relative module paths.
    current_dir: PathBuf,
    // The module path of the file being visited (e.g., "task::scheduler").
    module_path: String,
}

impl<'ast> Visit<'ast> for FileVisitor {
    /// This method is called for every `mod` item in the AST.
    fn visit_item_mod(&mut self, i: &'ast ItemMod) {
        // --- Test Module Exclusion Logic ---
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
        
        // --- Submodule Discovery Logic ---
        if i.content.is_none() {
            let mod_name = i.ident.to_string();
            let path_rs = self.current_dir.join(format!("{}.rs", mod_name));
            let path_mod_rs = self.current_dir.join(&mod_name).join("mod.rs");

            if path_rs.exists() {
                self.submodules.push(path_rs);
            } else if path_mod_rs.exists() {
                self.submodules.push(path_mod_rs);
            }
        } else {
            // For inline modules `mod foo { ... }`, we need to update the path and recurse.
            let original_path = self.module_path.clone();
            if self.module_path.is_empty() {
                self.module_path = i.ident.to_string();
            } else {
                self.module_path = format!("{}::{}", self.module_path, i.ident);
            }
            syn::visit::visit_item_mod(self, i);
            self.module_path = original_path; // Backtrack
        }
    }

    /// This method is called for any type of item (struct, enum, fn, etc.).
    fn visit_item(&mut self, i: &'ast Item) {
        let (is_public, name_opt) = match i {
            Item::Struct(s) => (matches!(s.vis, Visibility::Public(_)), Some(s.ident.to_string())),
            Item::Enum(e) => (matches!(e.vis, Visibility::Public(_)), Some(e.ident.to_string())),
            Item::Trait(t) => (matches!(t.vis, Visibility::Public(_)), Some(t.ident.to_string())),
            Item::Fn(f) => (matches!(f.vis, Visibility::Public(_)), Some(f.sig.ident.to_string())),
            _ => (false, None),
        };

        if is_public {
            if let Some(name) = name_opt {
                let full_path = if self.module_path.is_empty() {
                    format!("crate::{}", name)
                } else {
                    format!("crate::{}::{}", self.module_path, name)
                };
                self.pub_items.push(ApiItem { name, full_path });
            }
        }
        syn::visit::visit_item(self, i);
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
    let mut all_pub_items = HashSet::new(); // Use HashSet to handle duplicates automatically.

    while let Some(path) = files_to_process.pop() {
        if !processed_files.insert(path.clone()) {
            continue;
        }
        
        println!("cargo:rerun-if-changed={}", path.to_str().unwrap());
        let code = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let ast = match syn::parse_file(&code) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Failed to parse file: {}. Error: {}", path.display(), e);
                continue;
            }
        };

        // Calculate the module path from the file path relative to `src`.
        let path_from_src = path.strip_prefix(&src_path).unwrap();
        let mut module_path_str = path_from_src.with_extension("").to_string_lossy().to_string();
        if module_path_str.ends_with("/mod") || module_path_str.ends_with("\\mod") {
            module_path_str = PathBuf::from(&module_path_str).parent().unwrap().to_string_lossy().to_string();
        }
        if module_path_str == "lib" || module_path_str == "main" {
            module_path_str = String::new();
        }
        let module_path = module_path_str.replace("/", "::").replace("\\", "::");

        let mut visitor = FileVisitor {
            current_dir: path.parent().unwrap().to_path_buf(),
            module_path,
            ..Default::default()
        };
        
        visitor.visit_file(&ast);

        all_pub_items.extend(visitor.pub_items);
        files_to_process.extend(visitor.submodules);
    }

    // --- Reporting Phase ---
    println!("--- Detected Public APIs (excluding tests) ---");
    let mut sorted_items: Vec<_> = all_pub_items.into_iter().collect();
    sorted_items.sort();
    
    for item in sorted_items {
        println!("cargo:warning=Found pub item: {}", item.full_path);
    }
    println!("----------------------------");
}
