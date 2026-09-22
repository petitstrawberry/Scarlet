extern crate proc_macro;

use proc_macro::{TokenStream, TokenTree};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct OnThreadExit(Arc<AtomicUsize>);

impl Drop for OnThreadExit {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

thread_local! {
    static EXIT: RefCell<Option<OnThreadExit>> = const { RefCell::new(None) };
}

#[proc_macro]
pub fn answer(input: TokenStream) -> TokenStream {
    assert!(input.is_empty());
    let exits = Arc::new(AtomicUsize::new(0));
    let child_exits = Arc::clone(&exits);
    std::thread::spawn(move || {
        EXIT.with(|slot| *slot.borrow_mut() = Some(OnThreadExit(child_exits)));
    })
    .join()
    .unwrap();
    assert_eq!(
        exits.load(Ordering::SeqCst),
        1,
        "proc macro thread TLS destructor"
    );
    "42u32".parse().unwrap()
}

#[proc_macro_attribute]
pub fn passthrough(attribute: TokenStream, item: TokenStream) -> TokenStream {
    assert!(attribute.is_empty());
    item
}

#[proc_macro_derive(Answer)]
pub fn derive_answer(item: TokenStream) -> TokenStream {
    let mut tokens = item.into_iter();
    assert!(matches!(tokens.next(), Some(TokenTree::Ident(kind)) if kind.to_string() == "struct"));
    let Some(TokenTree::Ident(name)) = tokens.next() else {
        panic!("expected a struct name");
    };
    format!("impl {name} {{ fn answer() -> u32 {{ 42 }} }}")
        .parse()
        .unwrap()
}
