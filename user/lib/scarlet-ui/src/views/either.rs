//! Either - Conditional view for branching UI
//!
//! Similar to SwiftUI's `_ConditionalContent`, this allows `if`/`match`
//! expressions to return different view types from a single branch.
//!
//! # Example
//!
//! ```ignore
//! match show {
//!     true => Either::A(settings_view()),
//!     false => Either::B(clock_view()),
//! }
//! ```
//!
//! For 3+ branches, use `Either3`, `Either4`, etc.:
//!
//! ```ignore
//! match page {
//!     0 => Either3::A(home()),
//!     1 => Either3::B(settings()),
//!     2 => Either3::C(about()),
//! }
//! ```

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

use crate::element::Element;
use crate::view::View;

macro_rules! define_either {
    ($name:ident, ($($t:ident),+ $(,)?)) => {
        #[derive(Clone)]
        pub enum $name<$($t),+> {
            $($t($t)),+
        }

        impl<$($t: View + Clone + 'static),+> View for $name<$($t),+> {
            fn create_element(&self) -> Box<dyn Element> {
                match self {
                    $(Self::$t(v) => v.create_element(),)+
                }
            }

            fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
                match self {
                    $(Self::$t(v) => v.listenables(),)+
                }
            }

            fn as_any(&self) -> &dyn Any {
                self
            }

            fn type_id(&self) -> core::any::TypeId {
                match self {
                    $(Self::$t(v) => View::type_id(v),)+
                }
            }

            fn type_name(&self) -> &str {
                match self {
                    $(Self::$t(v) => View::type_name(v),)+
                }
            }
        }
    };
}

define_either!(Either, (A, B));
define_either!(Either3, (A, B, C));
define_either!(Either4, (A, B, C, D));
define_either!(Either5, (A, B, C, D, E));
define_either!(Either6, (A, B, C, D, E, F));
