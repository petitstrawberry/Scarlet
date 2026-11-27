use super::{Descriptor, Input, Kind, valid_descriptor};

// Report IDs 3 and 4 carry independent sets of relative axes and buttons.
const MOUSE: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x03, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7f,
    0x75, 0x08, 0x95, 0x02, 0x81, 0x06, 0x85, 0x04, 0x05, 0x09, 0x09, 0x01, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x01, 0x81, 0x02, 0x75, 0x07, 0x81, 0x01, 0xc0,
];

#[cfg_attr(target_os = "none", test_case)]
#[cfg_attr(not(target_os = "none"), test)]
fn numbered_mouse_reports_keep_button_state_across_motion_packets() {
    let mut descriptor = Descriptor::parse(MOUSE).unwrap();
    assert!(descriptor.numbered);
    assert_eq!(descriptor.input_bits[3], 16);
    assert_eq!(descriptor.input_bits[4], 8);
    let app = descriptor.apps.pop().unwrap();
    assert_eq!(app.kind, Kind::Mouse);
    let mut input = Input::new(app);
    assert_eq!(input.report(4, Some(&[1])), [(1, 0x110, 1), (0, 0, 0)]);
    assert_eq!(
        input.report(3, Some(&[0xf5, 7])),
        [(2, 0, -11), (2, 1, 7), (0, 0, 0)]
    );
    assert!(input.report(4, Some(&[1])).is_empty());
    assert_eq!(
        input.report(3, None),
        [(0, 3, 0), (0, 0, 0), (1, 0x110, 0), (0, 0, 0)]
    );
}

#[cfg_attr(target_os = "none", test_case)]
#[cfg_attr(not(target_os = "none"), test)]
fn descriptor_validation_handles_vendor_pages_and_rejects_truncated_items() {
    let vendor = [
        0x06, 0x00, 0xff, 0x09, 0x02, 0xa1, 0x01, 0x75, 8, 0x95, 1, 0x81, 2, 0xc0,
    ];
    assert!(valid_descriptor(&vendor));
    assert!(Descriptor::parse(&vendor).is_none());
    assert!(!valid_descriptor(&MOUSE[..MOUSE.len() - 1]));
    assert!(!valid_descriptor(&[0xfe, 0x04, 0x55, 0x01]));
}
