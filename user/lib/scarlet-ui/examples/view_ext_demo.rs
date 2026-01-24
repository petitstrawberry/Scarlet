//! ViewExt trait demonstration
//!
//! This example demonstrates the SwiftUI-style method chaining enabled by ViewExt.

#![no_std]

extern crate alloc;

use scarlet_ui::prelude::*;
use scarlet_ui::{Text, Color, EdgeInsets};

/// Demonstrates ViewExt usage
pub fn demo_view_ext() {
    // Example 1: Simple modifier chain
    let _view = Text::new("Hello, World!")
        .padding(10.0)
        .background(Color::BLUE)
        .frame(200.0, 50.0);

    // Example 2: Using EdgeInsets for custom padding
    let _view2 = Text::new("Custom Padding")
        .padding_insets(EdgeInsets {
            top: 5.0,
            left: 10.0,
            bottom: 15.0,
            right: 20.0,
        })
        .background(Color::RED);

    // Example 3: Frame methods
    let _view3 = Text::new("Fixed Width").frame_width(300.0);
    let _view4 = Text::new("Fixed Height").frame_height(100.0);

    // Example 4: Size constraints
    let _view5 = Text::new("Constrained")
        .size_constraints(100.0, 50.0, 500.0, 200.0);

    // Example 5: Alignment
    let _view6 = Text::new("Centered").alignment(Alignment::Center);

    // Example 6: Complex chain
    let _complex_view = Text::new("Complex Layout")
        .padding(20.0)
        .background(Color::rgb(0.2, 0.3, 0.4))
        .frame(400.0, 100.0)
        .alignment(Alignment::Center);

    // ViewExt trait is working correctly!
}
