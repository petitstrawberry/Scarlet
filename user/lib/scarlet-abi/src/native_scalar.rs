//! Scarlet Native syscall scalar convention (not the C or Linux syscall ABI).
//!
//! A u64/i64 occupies one argument word on a 64-bit target, or two consecutive
//! words, low then high, on a 32-bit target. There is no register-pair padding.
//! Following arguments move by `U64_WORDS`. Wide results use the first result
//! word and, on RV32, a1 for the high half. Signed offsets keep their two's
//! complement bits; pointers and lengths of user buffers remain native words.
//! Existing 64-bit syscall numbers and register layouts are unchanged.

pub const U64_WORDS: usize = 64 / usize::BITS as usize;

pub const fn u64_to_words(value: u64) -> [usize; U64_WORDS] {
    #[cfg(target_pointer_width = "64")]
    {
        [value as usize]
    }
    #[cfg(target_pointer_width = "32")]
    {
        [value as u32 as usize, (value >> 32) as usize]
    }
}

pub const fn u64_from_words(words: [usize; U64_WORDS]) -> u64 {
    #[cfg(target_pointer_width = "64")]
    {
        words[0] as u64
    }
    #[cfg(target_pointer_width = "32")]
    {
        words[0] as u64 | ((words[1] as u64) << 32)
    }
}

const _: () = {
    assert!(u64_from_words(u64_to_words(0xfedc_ba98_7654_3210)) == 0xfedc_ba98_7654_3210);
    assert!(u64_from_words(u64_to_words(u64::MAX)) == u64::MAX);
    assert!(u64_from_words(u64_to_words((-7i64) as u64)) as i64 == -7);
};
