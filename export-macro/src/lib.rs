extern crate proc_macro;
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, FnArg, Pat, PatIdent};

/// #[export]属性マクロ
/// 
/// この実装では、関数を出力し、加えてAPI_SYMBOLSを初期化する関数を生成します。
/// 
/// カーネル側（feature = "kernel_api"がない場合）：
/// - 元の関数を維持
/// - API_SYMBOLS構造体に関数ポインタを格納する初期化関数を生成
/// - early_initcall!でその初期化関数を登録
///
/// モジュール側（feature = "kernel_api"がある場合）：
/// - 元の関数をビルド対象から除外
/// - API_SYMBOLS経由で関数ポインタを取得するラッパー関数を生成
#[proc_macro_attribute]
pub fn export(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);
    let fn_name = &input.sig.ident;
    let fn_inputs = &input.sig.inputs;
    let fn_output = &input.sig.output;
    let init_fn_name = syn::Ident::new(&format!("__init_api_symbol_{}", fn_name), fn_name.span());
    
    // 引数名だけを抽出（ラッパー関数呼び出し用）
    let arg_names = fn_inputs.iter().filter_map(|arg| {
        if let FnArg::Typed(pat_type) = arg {
            if let Pat::Ident(PatIdent { ident, .. }) = &*pat_type.pat {
                return Some(ident);
            }
        }
        None
    }).collect::<Vec<_>>();
    
    println!("Exporting function: {}", fn_name);
    
    // 元の関数と初期化関数を生成
    let r#gen = quote! {
        #input

        #[allow(non_snake_case)]
        fn #init_fn_name() {
            unsafe {
                crate::symbol::API_SYMBOLS.#fn_name = #fn_name as *const ();
            }
        }

        paste::paste! {
            crate::early_initcall!(#init_fn_name);
        }
    };
    r#gen.into()
}
