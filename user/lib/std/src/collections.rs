//! Collection types for Scarlet
//!
//! This module provides:
//! - HashMap and HashSet from hashbrown for general use
//! - BTreeMap and BTreeSet from alloc for static contexts and ordered access

extern crate alloc;

pub use hashbrown::HashMap;
pub use hashbrown::HashSet;

// Re-export alloc collections for convenience
pub use alloc::collections::BTreeMap;
pub use alloc::collections::BTreeSet;

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_vec_creation() {
        let v: alloc::vec::Vec<i32> = alloc::vec::Vec::new();
        assert_eq!(v.len(), 0);
        assert_eq!(v.capacity(), 0);
    }

    #[test_case]
    fn test_vec_push() {
        use alloc::vec::Vec;
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        v.push(3);

        assert_eq!(v.len(), 3);
        assert_eq!(v[0], 1);
        assert_eq!(v[1], 2);
        assert_eq!(v[2], 3);
    }

    #[test_case]
    fn test_vec_pop() {
        use alloc::vec::Vec;
        let mut v = Vec::new();
        v.push(10);
        v.push(20);

        assert_eq!(v.pop(), Some(20));
        assert_eq!(v.pop(), Some(10));
        assert_eq!(v.pop(), None);
    }

    #[test_case]
    fn test_string_creation() {
        use alloc::string::String;
        let s = String::from("Hello");
        assert_eq!(s.len(), 5);
        assert_eq!(s.as_str(), "Hello");
    }

    #[test_case]
    fn test_string_push_str() {
        use alloc::string::String;
        let mut s = String::from("Hello");
        s.push_str(", World!");

        assert_eq!(s.as_str(), "Hello, World!");
    }

    #[test_case]
    fn test_hashmap_insert_get() {
        let mut map = HashMap::new();
        map.insert("key1", 100);
        map.insert("key2", 200);

        assert_eq!(map.get(&"key1"), Some(&100));
        assert_eq!(map.get(&"key2"), Some(&200));
        assert_eq!(map.get(&"key3"), None);
    }

    #[test_case]
    fn test_hashmap_len() {
        let mut map = HashMap::new();
        assert_eq!(map.len(), 0);

        map.insert(1, "one");
        map.insert(2, "two");

        assert_eq!(map.len(), 2);
    }

    #[test_case]
    fn test_btreemap_insert_get() {
        let mut map = BTreeMap::new();
        map.insert("b", 2);
        map.insert("a", 1);
        map.insert("c", 3);

        assert_eq!(map.get(&"a"), Some(&1));
        assert_eq!(map.get(&"b"), Some(&2));
        assert_eq!(map.get(&"c"), Some(&3));
    }
}
