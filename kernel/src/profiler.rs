//! Simple profiler for measuring function execution time.
//!
//! This module provides a simple profiling utility to measure the execution time,
//! call count and other statistics for functions and code blocks. It uses a
//! global registry to store profiling data and a RAII guard (`ProfileGuard`)
//! to automatically record timings.
//!
//! # Usage
//!
//! To profile a scope, use the `profile_scope!` macro at the beginning of the
//! function or block you want to measure.
//!
//! ```
//! use crate::profiler::profile_scope;
//!
//! fn my_function() {
//!     profile_scope!("my_function");
//!     // ... function logic ...
//! }
//! ```
//!
//! At the end of your program or at a convenient checkpoint, call
//! `print_profiling_results()` to display the collected statistics.

#[macro_export]
macro_rules! profile_scope {
    ($name:expr) => {
        #[cfg(feature = "profiler")]
        let _guard = $crate::profiler::ProfileGuard::new($name);
    };
}

#[cfg(feature = "profiler")]
pub use self::profiler_impl::*;

#[cfg(feature = "profiler")]
mod profiler_impl {
    use alloc::collections::BTreeMap;
    use alloc::string::{String, ToString};
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use spin::Mutex;
    use lazy_static::lazy_static;
    use crate::early_println;
    use crate::timer::get_time_ns;

    /// Tree node for hierarchical profiling
    #[derive(Clone)]
    pub struct ProfileNode {
        pub name: String,
        pub call_count: u64,
        pub total_time_ns: u64,
        pub self_time_ns: u64,  // Time excluding children
        pub min_time_ns: u64,
        pub max_time_ns: u64,
        pub children: BTreeMap<String, ProfileNode>,
    }

    impl ProfileNode {
        pub fn new(name: String) -> Self {
            ProfileNode {
                name,
                call_count: 0,
                total_time_ns: 0,
                self_time_ns: 0,
                min_time_ns: u64::MAX,
                max_time_ns: 0,
                children: BTreeMap::new(),
            }
        }

        pub fn add_call(&mut self, elapsed_time: u64) {
            self.call_count += 1;
            self.total_time_ns += elapsed_time;
            self.self_time_ns += elapsed_time;  // Will be adjusted when children are subtracted
            self.min_time_ns = self.min_time_ns.min(elapsed_time);
            self.max_time_ns = self.max_time_ns.max(elapsed_time);
        }
    }

    /// Call stack entry for tracking current execution path
    pub struct CallStackEntry {
        pub name: String,
        pub start_time: u64,
    }

    lazy_static! {
        /// Global profiling tree root
        pub static ref PROFILER_ROOT: Arc<Mutex<ProfileNode>> =
            Arc::new(Mutex::new(ProfileNode::new("ROOT".to_string())));
        
        /// Call stack for tracking current execution path
        pub static ref CALL_STACK: Arc<Mutex<Vec<CallStackEntry>>> =
            Arc::new(Mutex::new(Vec::new()));
    }

    /// A RAII guard that records the execution time of its scope.
    ///
    /// When created, it records the start time. When it goes out of scope (and is dropped),
    /// it calculates the elapsed time and updates the global profiler registry.
    pub struct ProfileGuard {
        name: &'static str,
        start_time: u64,
    }

    impl ProfileGuard {
        /// Creates a new `ProfileGuard`.
        pub fn new(name: &'static str) -> Self {
            let start_time = get_time_ns();
            
            // Push to call stack
            let mut call_stack = CALL_STACK.lock();
            call_stack.push(CallStackEntry {
                name: name.to_string(),
                start_time,
            });
            
            ProfileGuard {
                name,
                start_time,
            }
        }
    }

    impl Drop for ProfileGuard {
        fn drop(&mut self) {
            let end_time = get_time_ns();
            let elapsed = end_time.saturating_sub(self.start_time);

            let mut call_stack = CALL_STACK.lock();
            let mut root = PROFILER_ROOT.lock();
            
            // Pop current function from call stack
            if let Some(current_entry) = call_stack.pop() {
                assert_eq!(current_entry.name, self.name);
            }
            
            // Navigate to the correct position in the tree
            let mut current_node = &mut *root;
            
            // Follow the call stack path to find/create the parent node
            for entry in call_stack.iter() {
                current_node = current_node.children
                    .entry(entry.name.clone())
                    .or_insert_with(|| ProfileNode::new(entry.name.clone()));
            }
            
            // Add this function as a child of the current node
            let child_node = current_node.children
                .entry(self.name.to_string())
                .or_insert_with(|| ProfileNode::new(self.name.to_string()));
            
            child_node.add_call(elapsed);
        }
    }

    /// Prints the collected profiling results in a tree structure
    pub fn print_profiling_results() {
        let root = PROFILER_ROOT.lock();
        
        early_println!("--- Tree-based Profiling Results ---");
        early_println!("{:<50} | {:>10} | {:>15} | {:>15} | {:>15} | {:>12}",
            "Function", "Count", "Total (ms)", "Self (ms)", "Average (μs)", "% of Parent");
        early_println!("{}", "-".repeat(140));
        
        print_node(&root, 0, root.total_time_ns);
        early_println!("{}", "-".repeat(25));
    }
    
    fn print_node(node: &ProfileNode, depth: usize, parent_total_time: u64) {
        if node.name == "ROOT" {
            // Don't print the root node, just its children
            for (_, child) in &node.children {
                print_node(child, 0, child.total_time_ns);
            }
            return;
        }
        
        let indent = "  ".repeat(depth);
        let total_ms = node.total_time_ns as f64 / 1_000_000.0;
        let self_ms = node.self_time_ns as f64 / 1_000_000.0;
        let avg_us = if node.call_count > 0 {
            (node.total_time_ns / node.call_count) as f64 / 1_000.0
        } else {
            0.0
        };
        let percentage = if parent_total_time > 0 {
            (node.total_time_ns as f64 / parent_total_time as f64) * 100.0
        } else {
            0.0
        };
        
        // Calculate available width for function name: base width minus indent
        let base_width: usize = 50;
        let available_width = base_width.saturating_sub(indent.len());
        
        early_println!("{}{:<width$} | {:>10} | {:>15.3} | {:>15.3} | {:>15.3} | {:>11.2}%",
            indent, node.name, node.call_count, total_ms, self_ms, avg_us, percentage,
            width = available_width);
        
        // Print children sorted by total time (descending)
        let mut children: alloc::vec::Vec<_> = node.children.values().collect();
        children.sort_by(|a, b| b.total_time_ns.cmp(&a.total_time_ns));
        
        for child in children {
            print_node(child, depth + 1, node.total_time_ns);
        }
    }
}
