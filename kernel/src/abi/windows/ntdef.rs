//! Windows NT native structure definitions for 64-bit (AArch64 / x64).
//! All structures use `#[repr(C)]` to match the Windows ABI layout.

use core::mem::size_of;

/// NtQuerySystemInformation class 0x00
#[repr(C)]
#[derive(Default)]
pub struct SystemBasicInformation {
    pub reserved: u32,
    pub timer_resolution: u32,
    pub page_size: u32,
    pub number_of_physical_pages: u32,
    pub lowest_physical_page_number: u32,
    pub highest_physical_page_number: u32,
    pub allocation_granularity: u32,
    pub minimum_user_mode_address: u64,
    pub maximum_user_mode_address: u64,
    pub active_processors_affinity_mask: u64,
    pub number_of_processors: u8,
}

/// NtQuerySystemInformation class 0x03
#[repr(C)]
#[derive(Default)]
pub struct SystemTimeOfDayInformation {
    pub boot_time: u64,
    pub current_time: u64,
    pub time_zone_bias: u64,
    pub time_zone_id: u32,
    pub reserved: u32,
    pub boot_time_bias: u64,
    pub sleep_time_bias: u64,
}

/// NtQuerySystemInformation class 0x07
#[repr(C)]
#[derive(Default)]
pub struct SystemProcessorFeaturesInformation {
    pub processor_feature_bits: u64,
    pub reserved: [u64; 3],
}

/// NtQuerySystemInformation class 0x3E
#[repr(C)]
#[derive(Default)]
pub struct SystemCodeIntegrityInformation {
    pub code_integrity_options: u32,
}

const _: fn() = || {
    assert!(size_of::<SystemBasicInformation>() == 0x40);
    assert!(size_of::<SystemTimeOfDayInformation>() == 0x30);
    assert!(size_of::<SystemProcessorFeaturesInformation>() == 0x20);
    assert!(size_of::<SystemCodeIntegrityInformation>() == 0x04);
};
