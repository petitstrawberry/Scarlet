pub mod uart;

extern crate alloc;
use alloc::vec::Vec;

use crate::println;
use crate::print;

pub static mut DRIVER_TABLE: Vec<Driver> = Vec::new();

pub struct Driver {
    name: &'static str,
    match_fn: fn(&str) -> bool,
    probe_fn: fn(&str) -> Result<(), &'static str>,
}

#[allow(static_mut_refs)]
pub fn driver_register(driver: Driver) {
    println!("Driver registered: {}", driver.name);
    unsafe {
        DRIVER_TABLE.push(driver);
    }
}

#[cfg(test)]
mod tests {

    #[test_case]
    #[allow(static_mut_refs)]
    fn test_driver_register() {
        use super::*;
        let len = unsafe { DRIVER_TABLE.len() };
        let driver = Driver {
            name: "test",
            match_fn: |name| name == "test",
            probe_fn: |_name| Ok(()),
        };
        driver_register(driver);
        assert_eq!(unsafe { DRIVER_TABLE.len() }, len + 1);
    }
}