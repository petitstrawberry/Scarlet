use alloc::vec;

use super::*;
use super::mockchar::MockCharDevice;
use crate::device::DeviceType;

#[test_case]
fn test_generic_char_device_creation() {
    let read_fn = || Some(b'A');
    let write_fn = |_: u8| Ok(());
    let can_read_fn = || true;
    let can_write_fn = || true;

    let device = GenericCharDevice::new(
        1,
        "test_char",
        read_fn,
        write_fn,
        can_read_fn,
        can_write_fn,
    );

    assert_eq!(device.id(), 1);
    assert_eq!(device.name(), "test_char");
    assert_eq!(device.device_type(), DeviceType::Char);
    assert_eq!(device.name(), "test_char");
    assert_eq!(device.id(), 1);
}

#[test_case]
fn test_generic_char_device_read_write() {
    let read_fn = || Some(b'T');
    let write_fn = |_: u8| Ok(());
    let can_read_fn = || true;
    let can_write_fn = || true;

    let mut device = GenericCharDevice::new(
        2,
        "test_char_rw",
        read_fn,
        write_fn,
        can_read_fn,
        can_write_fn,
    );

    // Test single byte read
    assert_eq!(device.read_byte(), Some(b'T'));

    // Test single byte write
    assert!(device.write_byte(b'H').is_ok());

    // Test readiness checks
    assert!(device.can_read());
    assert!(device.can_write());
}

#[test_case]
fn test_mock_char_device() {
    let mut device = MockCharDevice::new(3, "mock_char");

    // Test device properties
    assert_eq!(device.id(), 3);
    assert_eq!(device.name(), "mock_char");
    assert_eq!(device.device_type(), DeviceType::Char);

    // Test read functionality
    device.set_read_data(vec![b'H', b'e', b'l', b'l', b'o']);
    assert_eq!(device.read_byte(), Some(b'H'));
    assert_eq!(device.read_byte(), Some(b'e'));
    assert_eq!(device.read_byte(), Some(b'l'));

    // Test write functionality
    assert!(device.write_byte(b'W').is_ok());
    assert!(device.write_byte(b'o').is_ok());
    assert!(device.write_byte(b'r').is_ok());
    assert!(device.write_byte(b'l').is_ok());
    assert!(device.write_byte(b'd').is_ok());

    let written_data = device.get_written_data();
    assert_eq!(written_data, &vec![b'W', b'o', b'r', b'l', b'd']);

    // Test can_read/can_write
    assert!(device.can_read()); // Still has data to read
    assert!(device.can_write()); // Mock device can always write
}

#[test_case]
fn test_char_device_buffer_operations() {
    let mut device = MockCharDevice::new(4, "buffer_test");

    // Set up read data
    let test_data = vec![b'T', b'e', b's', b't', b'!'];
    device.set_read_data(test_data.clone());

    // Test reading into buffer
    let mut read_buffer = [0u8; 10];
    let bytes_read = device.read(&mut read_buffer[..5]);
    assert_eq!(bytes_read, 5);
    assert_eq!(&read_buffer[..5], &test_data[..]);

    // Test writing from buffer
    let write_data = b"Hello";
    let result = device.write(write_data);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 5);

    let written = device.get_written_data();
    assert_eq!(written, &vec![b'H', b'e', b'l', b'l', b'o']);
}

#[test_case]
fn test_char_device_read_exhaustion() {
    let mut device = MockCharDevice::new(5, "exhaustion_test");

    // Set limited read data
    device.set_read_data(vec![b'A', b'B']);

    // Read all available data
    assert_eq!(device.read_byte(), Some(b'A'));
    assert_eq!(device.read_byte(), Some(b'B'));
    
    // Further reads should return None
    assert_eq!(device.read_byte(), None);
    assert!(!device.can_read());

    // Test partial buffer read
    device.reset_read_index();
    let mut buffer = [0u8; 5];
    let bytes_read = device.read(&mut buffer);
    assert_eq!(bytes_read, 2); // Only 2 bytes available
    assert_eq!(&buffer[..2], &[b'A', b'B']);
}
