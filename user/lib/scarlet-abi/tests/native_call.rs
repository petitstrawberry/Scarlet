use scarlet_abi::{NATIVE_CALL_BASE, Syscall, decode_native_call, syscall_transport_number};

#[test]
fn native_object_calls_cannot_collide_with_linux_syscalls() {
    for number in [56, 63, 64, 90, 100, 110, 200, 202, 222, 400, 900] {
        assert_eq!(decode_native_call(number), None);
    }
    for operation in [
        Syscall::VfsOpen,
        Syscall::HandleControl,
        Syscall::SocketCreate,
    ] {
        let encoded = NATIVE_CALL_BASE | operation as usize;
        assert_eq!(decode_native_call(encoded), Some(operation as usize));
        assert_eq!(
            syscall_transport_number(operation),
            if cfg!(target_os = "linux") {
                encoded
            } else {
                operation as usize
            }
        );
    }
}

#[test]
fn other_namespaces_and_high_bits_are_not_native_calls() {
    assert_eq!(decode_native_call(0x5342_0190), None);
    assert_eq!(decode_native_call(0x5344_0190), None);
    #[cfg(target_pointer_width = "64")]
    assert_eq!(
        decode_native_call((1usize << 32) | NATIVE_CALL_BASE | 400),
        None
    );
}
