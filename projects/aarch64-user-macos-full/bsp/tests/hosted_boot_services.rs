use scarlet::{device::manager::DeviceManager, random::RandomManager};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn hosted_boot_provides_system_time_and_registered_entropy() {
    let _platform = scarlet::hosted::prepare(4, 16 * 1024 * 1024).unwrap();

    let before = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let guest = Duration::from_nanos(scarlet::time::system_time_ns().unwrap());
    let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let tolerance = Duration::from_millis(1);
    assert!(guest >= before.saturating_sub(tolerance));
    assert!(guest <= after + tolerance);

    let mut entropy = [0; 512];
    // Read enough to require multiple pool refills. This API has no fallback.
    assert_eq!(RandomManager::get_random_bytes(&mut entropy), entropy.len());
    assert_ne!(entropy, [0; 512]);

    let device = DeviceManager::get_manager()
        .get_device_by_name("random")
        .expect("boot should register the random device");
    let mut bytes = [0; 32];
    assert_eq!(device.as_char_device().unwrap().read(&mut bytes), bytes.len());
    assert_ne!(bytes, [0; 32]);
}
