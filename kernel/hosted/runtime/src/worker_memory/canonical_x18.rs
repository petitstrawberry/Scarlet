//! Promote existing platform-register traps inside short, closed native loops.
//! Every canonical instruction remains a valid independent entry point.
use super::{Patch, PatchedInstruction};
use crate::{fetched_instruction::taken_loop, platform_register::Action};
use std::ops::Range;

pub(super) fn plan(
    bytes: &[u8],
    ranges: &[Range<usize>],
    patches: &mut [Patch],
    steps: &mut Vec<u32>,
) {
    for first in 0..patches.len() {
        let patch = &patches[first];
        if !matches!(
            patch.instruction,
            PatchedInstruction::PlatformRegister(Action::Native {
                paired: false,
                literal: None,
                ..
            })
        ) {
            continue;
        }
        let pc = patch.offset;
        let range = &ranges[ranges.partition_point(|r| r.end <= pc)];
        // The branch and the complete body must belong to one declared text range.
        for (i, raw) in bytes[pc..range.end].chunks_exact(4).take(32).enumerate() {
            let branch = u32::from_le_bytes(raw.try_into().unwrap());
            if branch & 0xff00_0010 != 0x5400_0000 {
                continue;
            }
            let displacement = ((branch << 8) as i32 >> 13) as isize * 4;
            let end = pc + 4 * i;
            let Some(start) = end.checked_add_signed(displacement) else {
                break;
            };
            if start < range.start || start > pc || end - start >= 32 * 4 {
                break;
            }
            let original: Vec<u32> = bytes[start..end + 4]
                .chunks_exact(4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .collect();
            let Some(block) = taken_loop(branch, end as u64, start as u64, |_, _| Some(&original))
            else {
                break;
            };
            // Darwin exposes x0..x28 in __x. FP/LR are separate fields.
            let Some(scratch) = block.scratch.filter(|&r| r < 29) else {
                break;
            };
            let slot = steps.len();
            steps.extend(block.words);
            for p in patches[first..].iter_mut().take_while(|p| p.offset <= end) {
                p.instruction = PatchedInstruction::PlatformRegister(Action::NativeLoop {
                    slot,
                    entry: ((p.offset - start) / 4) as u8,
                    exit: original.len() as u8,
                    scratch: scratch as u8,
                });
            }
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_memory::patch_instructions;

    #[test]
    fn canonical_loop_requires_one_code_range_and_a_closed_body() {
        let words = [0x9100_0652u32, 0xf100_056b, 0x54ff_ffc1]; // add x18; subs; b.ne -8
        let promoted = |body: &[u32], ranges: &[Range<usize>]| {
            let mut bytes: Vec<_> = body.iter().flat_map(|w| w.to_le_bytes()).collect();
            let patches = patch_instructions(&mut bytes, ranges, &mut Vec::new()).unwrap();
            patches.iter().any(|p| {
                matches!(
                    p.instruction,
                    PatchedInstruction::PlatformRegister(Action::NativeLoop { .. })
                )
            })
        };
        assert!(promoted(&words, &[0..12]));
        assert!(!promoted(&words, &[0..8, 8..12]));
        // An interior exit, literal load, syscall, BRK, or exclusive load must keep its original behavior.
        for middle in [
            0x1400_0002,
            0x5800_0020,
            0xd400_0001,
            0xd420_0000,
            0xc85f_7c20,
        ] {
            assert!(!promoted(&[words[0], middle, words[2]], &[0..12]));
        }
    }
}
