//! Literal boundary values exercise both models on the same host. These are
//! byte contracts, not a round trip through the implementation's own encoder.

use scarlet_abi::data_model::{AbiDataError, AbiDataModel, ByteOrder, WordWidth};
use scarlet_abi::environment::{EnvironmentExec, EnvironmentExecError};

const LE32: AbiDataModel = AbiDataModel::new(WordWidth::Bits32, ByteOrder::Little);
const LE64: AbiDataModel = AbiDataModel::new(WordWidth::Bits64, ByteOrder::Little);
const BE32: AbiDataModel = AbiDataModel::new(WordWidth::Bits32, ByteOrder::Big);
const BE64: AbiDataModel = AbiDataModel::new(WordWidth::Bits64, ByteOrder::Big);

#[test]
fn register_sign_extension_and_all_ones_sentinel() {
    for (bits, signed) in [
        (0, 0),
        (0x7fff_ffff, 2_147_483_647),
        (0x8000_0000, -2_147_483_648),
        (0xffff_ffff, -1),
    ] {
        assert_eq!(LE32.register(bits).unwrap().unsigned(), bits);
        assert_eq!(LE32.register(bits).unwrap().signed(), signed);
        assert_eq!(LE32.signed_register(signed).unwrap().unsigned(), bits);
    }
    assert_eq!(LE64.register(0xffff_ffff).unwrap().signed(), 4_294_967_295);
    assert_eq!(LE64.register(u64::MAX).unwrap().signed(), -1);
    assert_eq!(
        LE64.signed_register(i64::MIN).unwrap().unsigned(),
        0x8000_0000_0000_0000
    );
    assert_eq!(
        LE64.register(0x7fff_ffff_ffff_ffff).unwrap().signed(),
        i64::MAX
    );
    assert_eq!(
        LE32.register(0x1_0000_0000),
        Err(AbiDataError::ValueOutOfRange)
    );
    assert_eq!(
        LE32.signed_register(2_147_483_648),
        Err(AbiDataError::ValueOutOfRange)
    );
    assert_eq!(
        LE32.signed_register(-2_147_483_649),
        Err(AbiDataError::ValueOutOfRange)
    );
}

#[test]
fn pointer_slots_reject_high_bits_instead_of_narrowing() {
    for address in [0, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff] {
        assert_eq!(LE32.user_address(address).unwrap().value(), address);
    }
    assert_eq!(
        LE32.user_address(0x1_0000_0000),
        Err(AbiDataError::ValueOutOfRange)
    );
    assert_eq!(
        LE32.user_address(u64::MAX),
        Err(AbiDataError::ValueOutOfRange)
    );
    assert_eq!(
        LE64.user_address(0x1_0000_0000).unwrap().value(),
        0x1_0000_0000
    );
    // This fixed-width u64 pointer slot is legal only in the 64-bit model.
    let slot = [0, 0, 0, 0, 1, 0, 0, 0];
    let address = LE32.read_u64(&slot, 0).unwrap();
    assert_eq!(
        LE32.user_address(address),
        Err(AbiDataError::ValueOutOfRange)
    );
    assert!(LE64.user_address(address).is_ok());
}

#[test]
fn user_ranges_check_the_last_byte_and_array_multiplication() {
    for model in [LE32, LE64] {
        let top = model.user_address(model.word_width.max_unsigned()).unwrap();
        assert_eq!(top.check_range(0), Ok(()));
        assert_eq!(top.check_range(1), Ok(()));
        assert_eq!(top.check_range(2), Err(AbiDataError::AddressOverflow));
        assert_eq!(top.checked_add(1), Err(AbiDataError::AddressOverflow));
        let zero = model.user_address(0).unwrap();
        assert!(zero.is_null());
        assert_eq!(zero.check_range(0), Ok(()));
        assert_eq!(
            zero.element(u64::MAX, 8),
            Err(AbiDataError::AddressOverflow)
        );
    }
    let base = LE32.user_address(0xffff_f000).unwrap();
    assert_eq!(base.check_range(4096), Ok(()));
    assert_eq!(base.check_range(4097), Err(AbiDataError::AddressOverflow));
    assert_eq!(base.element(1023, 4).unwrap().value(), 0xffff_fffc);
    assert_eq!(base.element(1023, 4).unwrap().check_range(4), Ok(()));
    assert_eq!(base.element(1024, 4), Err(AbiDataError::AddressOverflow));
    assert_eq!(
        LE32.user_address(0).unwrap().check_range(0x1_0000_0000),
        Ok(())
    );
    assert_eq!(
        LE32.user_address(1).unwrap().check_range(0x1_0000_0000),
        Err(AbiDataError::AddressOverflow)
    );
    // Page boundaries are a VM concern, not a rejection by the scalar codec.
    assert_eq!(LE32.user_address(0x1ffe).unwrap().check_range(4), Ok(()));
}

#[test]
fn unaligned_word_bytes_and_byte_order() {
    let bytes = [0xa5, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x5a];
    for (model, expected) in [
        (LE32, 0x6745_2301),
        (BE32, 0x0123_4567),
        (LE64, 0xefcd_ab89_6745_2301),
        (BE64, 0x0123_4567_89ab_cdef),
    ] {
        assert_eq!(model.read_word(&bytes, 1).unwrap().unsigned(), expected);
        let mut written = [0xa5; 10];
        model.write_word(&mut written, 1, expected).unwrap();
        let end = 1 + model.word_width.bytes();
        assert_eq!(&written[1..end], &bytes[1..end]);
        assert_eq!(written[0], 0xa5);
        assert!(written[end..].iter().all(|byte| *byte == 0xa5));
    }
}

#[test]
fn wide_time_and_file_values_do_not_become_words() {
    let six_seconds_ns = [0x00, 0xbc, 0xa0, 0x65, 0x01, 0x00, 0x00, 0x00];
    let negative_offset = [0xff, 0xff, 0xff, 0xff, 0xfe, 0xff, 0xff, 0xff];
    for model in [LE32, LE64] {
        assert_eq!(model.read_u64(&six_seconds_ns, 0).unwrap(), 6_000_000_000);
        assert_eq!(model.read_i64(&negative_offset, 0).unwrap(), -4_294_967_297);
        let mut bytes = [0; 8];
        model.write_u64(&mut bytes, 0, 6_000_000_000).unwrap();
        assert_eq!(bytes, six_seconds_ns);
        model.write_i64(&mut bytes, 0, -4_294_967_297).unwrap();
        assert_eq!(bytes, negative_offset);
    }
    let positive_offset_be = [0, 0, 0, 1, 0, 0, 0, 1];
    assert_eq!(
        BE32.read_i64(&positive_offset_be, 0).unwrap(),
        4_294_967_297
    );
    let mut bytes = [0; 8];
    BE32.write_i64(&mut bytes, 0, 4_294_967_297).unwrap();
    assert_eq!(bytes, positive_offset_be);
}

#[test]
fn short_buffers_and_overflowing_offsets_never_panic_or_partially_write() {
    for model in [LE32, LE64, BE32, BE64] {
        for len in 0..model.word_width.bytes() {
            let mut bytes = [0xa5; 8];
            assert_eq!(
                model.read_word(&bytes[..len], 0),
                Err(AbiDataError::BufferTooShort)
            );
            assert_eq!(
                model.write_word(&mut bytes[..len], 0, 7),
                Err(AbiDataError::BufferTooShort)
            );
            assert_eq!(bytes, [0xa5; 8]);
        }
        let mut bytes = [0xa5; 8];
        assert_eq!(
            model.read_word(&bytes, usize::MAX),
            Err(AbiDataError::BufferTooShort)
        );
        assert_eq!(
            model.write_word(&mut bytes, usize::MAX, 7),
            Err(AbiDataError::BufferTooShort)
        );
        assert_eq!(
            model.write_u64(&mut bytes, 1, 7),
            Err(AbiDataError::BufferTooShort)
        );
        assert_eq!(bytes, [0xa5; 8]);
    }
    let mut bytes = [0xa5; 8];
    assert_eq!(
        LE32.write_word(&mut bytes, 0, 0x1_0000_0000),
        Err(AbiDataError::ValueOutOfRange)
    );
    assert_eq!(bytes, [0xa5; 8]);
}

// The 64-bit fixture freezes the existing Environment syscall input bytes.
const ENV64: [u8; 48] = [
    48, 0, 0, 0, 0, 0, 0, 0, 0x78, 0x56, 0x34, 0x12, 1, 0, 0, 0, 0, 0x20, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0x30, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0,
];
const ENV32: [u8; 28] = [
    28, 0, 0, 0, 0, 0, 0, 0, 0x78, 0x56, 0x34, 0x92, 0, 0x20, 0, 0, 0, 0, 0, 0, 0, 0x30, 0, 0, 2,
    0, 0, 0,
];

#[test]
fn environment_has_explicit_32_and_64_bit_record_layouts() {
    for (model, fixture, argv) in [
        (LE32, ENV32.as_slice(), 0x9234_5678),
        (LE64, ENV64.as_slice(), 0x1_1234_5678),
    ] {
        assert_eq!(EnvironmentExec::encoded_size(model), fixture.len());
        // Decode from an unaligned slice, with trailing bytes outside the record.
        let mut bytes = vec![0xa5];
        bytes.extend_from_slice(fixture);
        bytes.push(0x5a);
        let options = EnvironmentExec::decode(model, &bytes[1..]).unwrap();
        assert_eq!(options.argv.value(), argv);
        assert_eq!(options.envp.value(), 0x2000);
        assert!(options.cwd.is_null());
        assert_eq!(options.handles.value(), 0x3000);
        assert_eq!(options.handle_count, 2);
    }
    let big_endian = [
        0, 0, 0, 28, 0, 0, 0, 0, 0x92, 0x34, 0x56, 0x78, 0, 0, 0x20, 0, 0, 0, 0, 0, 0, 0, 0x30, 0,
        0, 0, 0, 2,
    ];
    assert_eq!(
        EnvironmentExec::decode(BE32, &big_endian)
            .unwrap()
            .argv
            .value(),
        0x9234_5678
    );
}

#[test]
fn environment_rejects_truncated_wrong_width_and_reserved_records() {
    for (model, fixture) in [(LE32, ENV32.as_slice()), (LE64, ENV64.as_slice())] {
        for len in 0..fixture.len() {
            assert_eq!(
                EnvironmentExec::decode(model, &fixture[..len]),
                Err(EnvironmentExecError::Encoding(AbiDataError::BufferTooShort))
            );
        }
        let mut bytes = fixture.to_vec();
        bytes[4] = 1;
        assert_eq!(
            EnvironmentExec::decode(model, &bytes),
            Err(EnvironmentExecError::UnsupportedFlags)
        );
        bytes[0] = 0;
        assert_eq!(
            EnvironmentExec::decode(model, &bytes),
            Err(EnvironmentExecError::UnsupportedSize)
        );
    }
    assert_eq!(
        EnvironmentExec::decode(LE32, &ENV64),
        Err(EnvironmentExecError::UnsupportedSize)
    );
    assert_eq!(
        EnvironmentExec::decode(LE64, &ENV32),
        Err(EnvironmentExecError::UnsupportedSize)
    );
}

#[test]
fn native_environment_record_matches_the_existing_literal_bytes() {
    #[cfg(target_pointer_width = "64")]
    let (fixture, argv) = (ENV64.as_slice(), 0x1_1234_5678);
    #[cfg(target_pointer_width = "32")]
    let (fixture, argv) = (ENV32.as_slice(), 0x9234_5678);
    let record = scarlet_abi::RawEnvironmentExec {
        size: fixture.len() as u32,
        flags: 0,
        argv,
        envp: 0x2000,
        cwd: 0,
        handles: 0x3000,
        handle_count: 2,
    };
    // No padding in either layout: two u32s followed by five native words.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &record as *const _ as *const u8,
            core::mem::size_of_val(&record),
        )
    };
    if cfg!(target_endian = "little") {
        assert_eq!(bytes, fixture);
    }
    assert_eq!(
        EnvironmentExec::decode(AbiDataModel::NATIVE, bytes)
            .unwrap()
            .argv
            .value(),
        argv as u64
    );
}
