use std::collections::HashSet;
use std::{collections::HashMap, fs, io::Write, path::Path};
use std::error::Error;
use syn::{visit::Visit, Attribute, File, Item, ItemFn, Meta, MetaNameValue};
use walkdir::WalkDir;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s == "gen-kernel-api").unwrap_or(false) {
        if let Err(e) = generate_kernel_api() {
            eprintln!("Error generating kernel_api crate: {}", e);
            std::process::exit(1);
        }
    } else {
        eprintln!("Usage: cargo xtask [gen-kernel-api]");
    }
}


// kernel_apiクレートを生成する関数
fn generate_kernel_api() -> Result<(), Box<dyn Error>> {
    println!("Generating kernel_api crate...");
    
    // カーネルソースディレクトリのパス
    let kernel_src_dir = Path::new("../kernel/src");
    
    // #[export]属性付きの関数と構造体を収集
    let exported_functions = collect_exported_functions(kernel_src_dir)?;
    let exported_types = collect_exported_types(kernel_src_dir)?;
    
    if exported_functions.is_empty() && exported_types.is_empty() {
        println!("⚠️ No exported functions or types found");
        return Ok(());
    }
    
    println!("✅ Found {} exported functions and {} exported types", 
             exported_functions.len(), exported_types.len());
    
    // モジュール構造を分析してマッピングを作成
    let module_map = analyze_module_structure(kernel_src_dir, &exported_functions, &exported_types)?;
    
    // kernel_apiクレートのディレクトリを作成
    let api_crate_dir = Path::new("../kernel_api/src");
    fs::create_dir_all(api_crate_dir)?;
    
    // lib.rs を生成
    generate_api_lib_rs(api_crate_dir, &module_map)?;
    
    // 各モジュールのファイルを生成
    for (module_path, items) in &module_map {
        generate_module_file(api_crate_dir, module_path, items, &module_map)?;
    }
    
    println!("✅ kernel_api crate generated successfully.");
    Ok(())
}


fn collect_exported_functions(src_dir: &Path) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let mut exported_functions = Vec::new();

    // カーネルソースツリーを再帰的に走査
    for entry in WalkDir::new(src_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        println!("Processing file: {:?}", path);

        // Rustソースファイルのみを処理
        if path.extension().map_or(false, |ext| ext == "rs") {
            let content = fs::read_to_string(path)?;
            
            // synでソースコードをパース
            if let Ok(file) = syn::parse_file(&content) {
                let mut visitor = ExportVisitor::default();
                visitor.visit_file(&file);
                
                // #[export]属性付き関数をリストに追加
                for (name, sig) in visitor.exports {
                    exported_functions.push((name, sig));
                }
            }
        }
    }
    
    Ok(exported_functions)
}

#[derive(Default)]
struct ExportVisitor {
    exports: Vec<(String, String)>,
}

impl<'ast> Visit<'ast> for ExportVisitor {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        // 関数に#[export]属性があるか確認
        if node.attrs.iter().any(|attr| is_export_attr(attr)) {
            let name = node.sig.ident.to_string();
            
            // 関数シグネチャの詳細情報を取得
            let mut params = Vec::new();
            for input in &node.sig.inputs {
                match input {
                    syn::FnArg::Typed(pat_type) => {
                        // 引数の型と名前を取得
                        let ty = &pat_type.ty;
                        let ty_str = quote::quote!(#ty).to_string();
                        
                        // 引数名を取得
                        let param_name = match &*pat_type.pat {
                            syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                            _ => "_".to_string(),
                        };
                        
                        params.push((param_name, ty_str));
                    },
                    _ => {} // レシーバー引数（self）は無視
                }
            }
            
            // 戻り値の型を取得
            let return_type = match &node.sig.output {
                syn::ReturnType::Default => "()".to_string(),
                syn::ReturnType::Type(_, ty) => quote::quote!(#ty).to_string(),
            };
            
            // 関数の修飾子（async, unsafeなど）
            let is_async = node.sig.asyncness.is_some();
            let is_unsafe = node.sig.unsafety.is_some();
            
            // シグネチャ情報をJSONライクな形式で保存
            let sig_info = format!(
                "{{\"params\":{:?},\"return\":\"{}\",\"is_async\":{},\"is_unsafe\":{}}}",
                params, return_type, is_async, is_unsafe
            );
            
            self.exports.push((name, sig_info));
        }
        
        // デフォルトの走査を続行
        syn::visit::visit_item_fn(self, node);
    }
}

fn is_export_attr(attr: &Attribute) -> bool {
    // 'export'という名前の属性を検索
    if attr.path().is_ident("export") {
        return true;
    }
    
    // // また、#[unsafe(export_name = "...")] のようなものも検出する
    // if attr.path().is_ident("unsafe") {
    //     // 属性の中のexport_nameを検索
    //     if let Ok(meta) = attr.meta.require_list() {
    //         if let Ok(nested) = meta.parse_args_with(syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated) {
    //             for nested_meta in nested {
    //                 if let syn::Meta::NameValue(name_value) = nested_meta {
    //                     if name_value.path.is_ident("export_name") {
    //                         return true;
    //                     }
    //                 }
    //             }
    //         }
    //     }
    // }
    
    false
}

// モジュール構造を分析する関数
fn analyze_module_structure(
    src_dir: &Path, 
    exported_functions: &[(String, String)], 
    exported_types: &[(String, String, String)]
) -> Result<HashMap<String, Vec<ExportedItem>>, Box<dyn Error>> {
    let mut module_map: HashMap<String, Vec<ExportedItem>> = HashMap::new();
    
    // .rsファイルを再帰的に走査してモジュール構造を分析
    for entry in WalkDir::new(src_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        
        // Rustソースファイルのみを処理
        if path.extension().map_or(false, |ext| ext == "rs") {
            let rel_path = path.strip_prefix(src_dir)?.to_str().unwrap_or("").to_string();
            let module_path = get_module_path(&rel_path);
            
            // ファイル内容を読み込む
            let content = fs::read_to_string(path)?;
            
            // ファイルから宣言を解析
            if let Ok(file) = syn::parse_file(&content) {
                // このファイル内のエクスポート項目を収集
                for (name, ty) in exported_functions {
                    if is_item_in_file(&file, name) {
                        // JSONからシグネチャ情報を解析
                        let sig_info: serde_json::Value = if ty.starts_with("{") {
                            match serde_json::from_str(ty) {
                                Ok(v) => v,
                                Err(_) => {
                                    // 解析エラーの場合はデフォルト値を使用
                                    let item = ExportedItem::Function {
                                        name: name.clone(),
                                        signature: ty.clone(),
                                        params: Vec::new(),
                                        return_type: "()".to_string(),
                                        is_async: false,
                                        is_unsafe: false,
                                    };
                                    module_map.entry(module_path.clone()).or_default().push(item);
                                    continue;
                                }
                            }
                        } else {
                            // 古い形式の場合はデフォルト値を使用
                            let item = ExportedItem::Function {
                                name: name.clone(),
                                signature: ty.clone(),
                                params: Vec::new(),
                                return_type: "()".to_string(),
                                is_async: false,
                                is_unsafe: false,
                            };
                            module_map.entry(module_path.clone()).or_default().push(item);
                            continue;
                        };

                        // JSONからパラメータ情報を抽出
                        let mut params = Vec::new();
                        if let Some(params_value) = sig_info.get("params") {
                            if let Some(params_array) = params_value.as_array() {
                                for param in params_array {
                                    if let (Some(param_name), Some(param_type)) = (
                                        param.get(0).and_then(|v| v.as_str()),
                                        param.get(1).and_then(|v| v.as_str())
                                    ) {
                                        params.push((param_name.to_string(), param_type.to_string()));
                                    }
                                }
                            }
                        }

                        // その他の情報を抽出
                        let return_type = sig_info.get("return").and_then(|v| v.as_str()).unwrap_or("()").to_string();
                        let is_async = sig_info.get("is_async").and_then(|v| v.as_bool()).unwrap_or(false);
                        let is_unsafe = sig_info.get("is_unsafe").and_then(|v| v.as_bool()).unwrap_or(false);

                        let item = ExportedItem::Function {
                            name: name.clone(),
                            signature: ty.clone(),
                            params,
                            return_type,
                            is_async,
                            is_unsafe,
                        };
                        module_map.entry(module_path.clone()).or_default().push(item);
                    }
                }
                
                for (name, kind, _) in exported_types {
                    if is_item_in_file(&file, name) {
                        let item = ExportedItem::Type {
                            name: name.clone(),
                            kind: kind.clone(),
                        };
                        module_map.entry(module_path.clone()).or_default().push(item);
                    }
                }
            }
        }
    }
    
    Ok(module_map)
}

// ファイルパスからモジュールパスを取得
fn get_module_path(rel_path: &str) -> String {
    let path = Path::new(rel_path);
    let mut components = Vec::new();
    
    let mut current = path;
    while let Some(file_name) = current.file_name() {
        if let Some(name) = file_name.to_str() {
            if name == "mod.rs" {
                // モジュールディレクトリの場合
                if let Some(parent) = current.parent() {
                    if let Some(parent_name) = parent.file_name() {
                        components.push(parent_name.to_str().unwrap_or("").to_string());
                    }
                }
            } else {
                // 通常のファイルの場合
                let module_name = name.strip_suffix(".rs").unwrap_or(name);
                if module_name != "lib" && module_name != "main" {
                    components.push(module_name.to_string());
                }
            }
        }
        
        current = if let Some(parent) = current.parent() {
            parent
        } else {
            break;
        };
    }
    
    components.reverse();
    components.join("::")
}

// 指定された項目がファイル内に存在するか確認
fn is_item_in_file(file: &syn::File, item_name: &str) -> bool {
    for item in &file.items {
        match item {
            syn::Item::Fn(f) => {
                if f.sig.ident == item_name {
                    return true;
                }
            },
            syn::Item::Struct(s) => {
                if s.ident == item_name {
                    return true;
                }
            },
            syn::Item::Enum(e) => {
                if e.ident == item_name {
                    return true;
                }
            },
            syn::Item::Type(t) => {
                if t.ident == item_name {
                    return true;
                }
            },
            _ => {}
        }
    }
    false
}

// エクスポートされた項目を表す列挙型
#[derive(Debug, Clone)]
enum ExportedItem {
    Function {
        name: String,
        signature: String,
        params: Vec<(String, String)>,  // (引数名, 型)
        return_type: String,
        is_async: bool,
        is_unsafe: bool,
    },
    Type {
        name: String,
        kind: String,
    },
}

// kernel_api クレートの lib.rs を生成
fn generate_api_lib_rs(
    api_crate_dir: &Path, 
    module_map: &HashMap<String, Vec<ExportedItem>>
) -> Result<(), Box<dyn Error>> {
    let mut content = String::from(
        "// Auto-generated by xtask\n// DO NOT EDIT MANUALLY\n\n"
    );
    
    content.push_str("#![no_std]\n\n");
    content.push_str("//! Scarlet Kernel API\n");
    content.push_str("//! \n");
    content.push_str("//! This crate provides access to Scarlet kernel functions and types.\n");
    content.push_str("//! It is automatically generated from the kernel source code.\n\n");
    
    // シンボルテーブルのインポート
    content.push_str("mod symbol;\n");
    content.push_str("pub use symbol::API_SYMBOLS;\n\n");
    
    // モジュール宣言（最上位モジュールのリストを作成）
    let mut root_modules = HashSet::new();
    for module_path in module_map.keys() {
        let root = module_path.split("::").next().unwrap_or("");
        if !root.is_empty() {
            root_modules.insert(root.to_string());
        }
    }
    
    // 最上位モジュールの宣言
    for module in &root_modules {
        content.push_str(&format!("pub mod {};\n", module));
    }
    content.push_str("\n");
    
    // ここから最上位モジュールのエクスポート関数・型を直接lib.rsに含める
    let mut lib_content = content.clone();
    
    // 空のモジュールパスの項目を処理する（直接lib.rsに入れる）
    if let Some(root_items) = module_map.get("") {
        
        // 関数と型のラッパーを生成
        for item in root_items {
            match item {
                ExportedItem::Function { 
                    name, 
                    signature: _, 
                    params, 
                    return_type, 
                    is_async, 
                    is_unsafe 
                } => {
                    // 関数シグネチャを生成
                    let async_kw = if *is_async { "async " } else { "" };
                    let unsafe_kw = if *is_unsafe { "unsafe " } else { "" };
                    
                    // パラメータリストを生成
                    let param_list = params.iter()
                        .map(|(name, ty)| format!("{}: {}", name, ty))
                        .collect::<Vec<_>>()
                        .join(", ");
                    
                    // 引数名リストを生成
                    let arg_list = params.iter()
                        .map(|(name, _)| name.clone())
                        .collect::<Vec<_>>()
                        .join(", ");
                    
                    // パラメータ型リストを生成
                    let param_type_list = params.iter()
                        .map(|(_, ty)| ty.clone())
                        .collect::<Vec<_>>()
                        .join(", ");
                    
                    lib_content.push_str(&format!(
                        "/// Wrapper for the `{}` kernel function\n", name
                    ));
                    lib_content.push_str(&format!(
                        "pub {}{}fn {}({}) -> {} {{\n", async_kw, unsafe_kw, name, param_list, return_type
                    ));
                    lib_content.push_str(&format!(
                        "    let func_ptr = unsafe {{ crate::API_SYMBOLS.{} }};\n", name
                    ));
                    lib_content.push_str(
                        "    if func_ptr.is_null() {\n"
                    );
                    lib_content.push_str(&format!(
                        "        panic!(\"API function {} not initialized\");\n", name
                    ));
                    lib_content.push_str(
                        "    }\n"
                    );
                    lib_content.push_str(&format!(
                        "    let f: {}{}fn({}) -> {} = unsafe {{ core::mem::transmute(func_ptr) }};\n",
                        async_kw, unsafe_kw, param_type_list, return_type
                    ));
                    lib_content.push_str(&format!(
                        "    f({})\n", arg_list
                    ));
                    lib_content.push_str("}\n\n");
                },
                ExportedItem::Type { name, kind } => {
                    lib_content.push_str(&format!(
                        "/// Exported {} `{}`\n", kind, name
                    ));
                    lib_content.push_str(&format!(
                        "// pub {0} {1} {{ /* フィールド */ }}\n\n", kind, name
                    ));
                },
            }
        }
        
        // lib.rsに書き込む
        let lib_rs_path = api_crate_dir.join("lib.rs");
        fs::write(lib_rs_path, lib_content)?;
    } else {
        // 最上位モジュールの処理（`.rs`ファイルの代わりにlib.rsに内容を追加）
        let mut lib_content = content.clone();
        
        // 最上位モジュールの関数と型をlib.rsに直接含める
        for (module_path, items) in module_map.iter() {
            if module_path.split("::").count() == 1 {  // 最上位モジュールのみを処理
                for item in items {
                    if let ExportedItem::Function { 
                        name, 
                        signature: _, 
                        params, 
                        return_type, 
                        is_async, 
                        is_unsafe 
                    } = item {
                        // 関数シグネチャを生成
                        let async_kw = if *is_async { "async " } else { "" };
                        let unsafe_kw = if *is_unsafe { "unsafe " } else { "" };
                        
                        // パラメータリストを生成
                        let param_list = params.iter()
                            .map(|(name, ty)| format!("{}: {}", name, ty))
                            .collect::<Vec<_>>()
                            .join(", ");
                        
                        // 引数名リストを生成
                        let arg_list = params.iter()
                            .map(|(name, _)| name.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        
                        // パラメータ型リストを生成
                        let param_type_list = params.iter()
                            .map(|(_, ty)| ty.clone())
                            .collect::<Vec<_>>()
                            .join(", ");
                        
                        lib_content.push_str(&format!(
                            "/// Wrapper for the `{}` kernel function\n", name
                        ));
                        lib_content.push_str(&format!(
                            "pub {}{}fn {}({}) -> {} {{\n", async_kw, unsafe_kw, name, param_list, return_type
                        ));
                        lib_content.push_str(&format!(
                            "    let func_ptr = unsafe {{ crate::API_SYMBOLS.{} }};\n", name
                        ));
                        lib_content.push_str(
                            "    if func_ptr.is_null() {\n"
                        );
                        lib_content.push_str(&format!(
                            "        panic!(\"API function {} not initialized\");\n", name
                        ));
                        lib_content.push_str(
                            "    }\n"
                        );
                        lib_content.push_str(&format!(
                            "    let f: {}{}fn({}) -> {} = unsafe {{ core::mem::transmute(func_ptr) }};\n",
                            async_kw, unsafe_kw, param_type_list, return_type
                        ));
                        lib_content.push_str(&format!(
                            "    f({})\n", arg_list
                        ));
                        lib_content.push_str("}\n\n");
                    }
                }
            }
        }
        
        // lib.rsに書き込む
        let lib_rs_path = api_crate_dir.join("lib.rs");
        fs::write(lib_rs_path, lib_content)?;
    }
    
    // symbol.rs ファイルも生成
    let symbol_rs_path = api_crate_dir.join("symbol.rs");
    fs::write(symbol_rs_path, fs::read_to_string("../kernel/src/symbol.rs")?)?;
    
    Ok(())
}

// モジュールファイルを生成
fn generate_module_file(
    api_crate_dir: &Path, 
    module_path: &str, 
    items: &[ExportedItem],
    module_map: &HashMap<String, Vec<ExportedItem>>
) -> Result<(), Box<dyn Error>> {
    let path_components: Vec<&str> = module_path.split("::").collect();
    let mut current_dir = api_crate_dir.to_path_buf();
    
    // 最後のコンポーネント以外のディレクトリを作成
    for (i, component) in path_components.iter().enumerate() {
        if i < path_components.len() - 1 {
            current_dir = current_dir.join(component);
            fs::create_dir_all(&current_dir)?;
        }
    }
    
    // 最上位モジュールかどうかを確認
    let is_top_level = path_components.len() == 1;

    // 最上位モジュールの場合は処理をスキップ（lib.rsで既に処理済み）
    if is_top_level {
        // この最上位モジュールに含まれる関数や型はlib.rsで既に宣言されているのでスキップ
        return Ok(());
    }

    // ファイル名を決定
    let file_name = if path_components.is_empty() {
        // 空の場合はmod.rsにする
        "mod.rs".to_string()
    } else {
        // 通常のモジュール名.rsとして扱う
        format!("{}.rs", path_components.last().unwrap())
    };
    
    let file_path = current_dir.join(file_name);
    
    // ファイル内容を生成
    let mut content = String::from(
        "// Auto-generated by xtask\n// DO NOT EDIT MANUALLY\n\n"
    );
    
    // サブモジュールのre-export
    let prefix = if module_path.is_empty() {
        "".to_string()
    } else {
        module_path.to_string() + "::"
    };
    
    let sub_modules: HashSet<String> = module_map.keys()
        .filter(|k| {
            if prefix.is_empty() {
                // ルートモジュールの場合は、すべての最上位モジュールを含める
                !k.is_empty() && !k.contains("::")
            } else {
                // それ以外の場合は、プレフィックスで始まるが同じではないものを含める
                k.starts_with(&prefix) && k != &module_path
            }
        })
        .map(|k| {
            if prefix.is_empty() {
                k.clone()
            } else {
                k.strip_prefix(&prefix)
                    .unwrap_or(k)
                    .split("::")
                    .next()
                    .unwrap_or("")
                    .to_string()
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    
    for sub_module in &sub_modules {
        content.push_str(&format!("pub mod {};\n", sub_module));
    }
    
    if !sub_modules.is_empty() {
        content.push_str("\n");
    }
    
    // 関数と型のラッパーを生成
    for item in items {
        match item {
            ExportedItem::Function { 
                name, 
                signature: _, 
                params, 
                return_type, 
                is_async, 
                is_unsafe 
            } => {
                // 関数シグネチャを生成
                let async_kw = if *is_async { "async " } else { "" };
                let unsafe_kw = if *is_unsafe { "unsafe " } else { "" };
                
                // パラメータリストを生成
                let param_list = params.iter()
                    .map(|(name, ty)| format!("{}: {}", name, ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                
                // 引数名リストを生成
                let arg_list = params.iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                
                // パラメータ型リストを生成
                let param_type_list = params.iter()
                    .map(|(_, ty)| ty.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                
                content.push_str(&format!(
                    "/// Wrapper for the `{}` kernel function\n", name
                ));
                content.push_str(&format!(
                    "pub {}{}fn {}({}) -> {} {{\n", async_kw, unsafe_kw, name, param_list, return_type
                ));
                content.push_str(&format!(
                    "    let func_ptr = unsafe {{ crate::API_SYMBOLS.{} }};\n", name
                ));
                content.push_str(
                    "    if func_ptr.is_null() {\n"
                );
                content.push_str(&format!(
                    "        panic!(\"API function {} not initialized\");\n", name
                ));
                content.push_str(
                    "    }\n"
                );
                content.push_str(&format!(
                    "    let f: {}{}fn({}) -> {} = unsafe {{ core::mem::transmute(func_ptr) }};\n",
                    async_kw, unsafe_kw, param_type_list, return_type
                ));
                content.push_str(&format!(
                    "    f({})\n", arg_list
                ));
                content.push_str("}\n\n");
            },
            ExportedItem::Type { name, kind } => {
                content.push_str(&format!(
                    "/// Exported {} `{}`\n", kind, name
                ));
                content.push_str(&format!(
                    "// pub {0} {1} {{ /* フィールド */ }}\n\n", kind, name
                ));
            },
        }
    }
    
    // ファイルに書き込む
    fs::write(file_path, content)?;
    
    Ok(())
}

// エクスポートされた型を収集する関数
fn collect_exported_types(src_dir: &Path) -> Result<Vec<(String, String, String)>, Box<dyn Error>> {
    let mut exported_types = Vec::new();
    
    // カーネルソースツリーを再帰的に走査
    for entry in WalkDir::new(src_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        
        // Rustソースファイルのみを処理
        if path.extension().map_or(false, |ext| ext == "rs") {
            let content = fs::read_to_string(path)?;
            
            // synでソースコードをパース
            if let Ok(file) = syn::parse_file(&content) {
                let mut visitor = ExportTypeVisitor::default();
                visitor.visit_file(&file);
                
                // #[export]属性付き型をリストに追加
                for (name, kind, doc) in visitor.exports {
                    exported_types.push((name, kind, doc));
                }
            }
        }
    }
    
    Ok(exported_types)
}

// エクスポートされた型を訪問するための訪問者パターン
#[derive(Default)]
struct ExportTypeVisitor {
    exports: Vec<(String, String, String)>, // (名前, 種類, ドキュメント)
}

impl<'ast> Visit<'ast> for ExportTypeVisitor {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        // 構造体に#[export]属性があるか確認
        if node.attrs.iter().any(|attr| is_export_attr(attr)) {
            let name = node.ident.to_string();
            let doc = extract_doc_comment(&node.attrs);
            self.exports.push((name, "struct".to_string(), doc));
        }
        
        // デフォルトの走査を続行
        syn::visit::visit_item_struct(self, node);
    }
    
    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        // 列挙型に#[export]属性があるか確認
        if node.attrs.iter().any(|attr| is_export_attr(attr)) {
            let name = node.ident.to_string();
            let doc = extract_doc_comment(&node.attrs);
            self.exports.push((name, "enum".to_string(), doc));
        }
        
        // デフォルトの走査を続行
        syn::visit::visit_item_enum(self, node);
    }
    
    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        // 型エイリアスに#[export]属性があるか確認
        if node.attrs.iter().any(|attr| is_export_attr(attr)) {
            let name = node.ident.to_string();
            let doc = extract_doc_comment(&node.attrs);
            self.exports.push((name, "type".to_string(), doc));
        }
        
        // デフォルトの走査を続行
        syn::visit::visit_item_type(self, node);
    }
}

// ドキュメントコメントを抽出する関数
fn extract_doc_comment(attrs: &[syn::Attribute]) -> String {
    let mut doc = String::new();
    
    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(name_value) = &attr.meta {
                if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(lit_str), .. }) = &name_value.value {
                    let comment = lit_str.value();
                    doc.push_str(&comment);
                    doc.push('\n');
                }
            }
        }
    }
    
    doc
}