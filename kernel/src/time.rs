//! Time utilities for the kernel
//! 
//! This module provides time-related functionality for the kernel,
//! including current time access for filesystem operations.

use crate::timer::get_kernel_timer;

/// Get the current time in microseconds
/// 
/// This function returns the current system time in microseconds since boot.
/// For filesystem operations, this provides a monotonic timestamp.
pub fn current_time() -> u64 {
    // For now, use CPU 0's timer. In a multi-core system, this might need
    // to be more sophisticated to get a consistent global timestamp.
    get_kernel_timer().get_time_us(0)
}

/// Get the current time in milliseconds
pub fn current_time_ms() -> u64 {
    current_time() / 1000
}

/// Get the current time in seconds
pub fn current_time_s() -> u64 {
    current_time() / 1_000_000
}

/// Convert microseconds to a human-readable format (for debugging)
pub fn format_time_us(time_us: u64) -> (u64, u64, u64) {
    let seconds = time_us / 1_000_000;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    
    (hours, minutes % 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_format_time() {
        let (hours, minutes, seconds) = format_time_us(3_661_000_000); // 1 hour, 1 minute, 1 second
        assert_eq!(hours, 1);
        assert_eq!(minutes, 1);
        assert_eq!(seconds, 1);
        
        let (hours, minutes, seconds) = format_time_us(123_000_000); // 2 minutes, 3 seconds
        assert_eq!(hours, 0);
        assert_eq!(minutes, 2);
        assert_eq!(seconds, 3);
    }
}
