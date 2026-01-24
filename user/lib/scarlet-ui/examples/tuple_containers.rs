//! Example demonstrating tuple-based container views

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::string::ToString;
use scarlet_ui::prelude::*;
use scarlet_ui::{vstack, hstack, zstack};

/// Example app demonstrating tuple-based containers
struct TupleContainerExample {
    // Add state here if needed
}

impl TupleContainerExample {
    fn new() -> Self {
        Self {}
    }

    fn build_ui(&self) -> VStack<(Text, Text)> {
        // Direct tuple usage
        VStack::new((
            Text::new("Tuple Container Example"),
            Text::new("VStack with 2-tuple"),
        ))
        .spacing(10.0)
    }

    fn build_ui_with_macro(&self) -> VStack<(Text, Text, Text)> {
        // Macro usage - should expand to the same thing
        vstack! {
            Text::new("Hello"),
            Text::new("from"),
            Text::new("macro!"),
        }
        .spacing(5.0)
    }
}

#[no_mangle]
pub extern "C" fn tuple_container_entry() {
    let example = TupleContainerExample::new();

    // Test direct tuple construction
    let _ui1 = example.build_ui();

    // Test macro construction
    let _ui2 = example.build_ui_with_macro();

    // Test HStack
    let _hstack = HStack::new((
        Text::new("Left"),
        Text::new("Right"),
    ));

    // Test ZStack
    let _zstack = ZStack::new((
        Text::new("Background"),
        Text::new("Foreground"),
    ));
}
