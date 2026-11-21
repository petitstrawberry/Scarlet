//! A frequently repeated native branch into fragmented text can keep the
//! return and the next call in one sealed allocation. All entries require
//! the same guest LR. Instructions that could change it exit at their PC.
use super::{loops, shadow::EXIT, signed};
use crate::{
    platform_register,
    worker_memory::{Mapping, NativeSteps},
};
use std::{collections::BTreeMap, io};

type Registry = BTreeMap<usize, Mapping>;

fn owner(maps: &Registry, pc: u64) -> Option<&Mapping> {
    let (_, m) = maps.range(..=pc as usize).next_back()?;
    m.contains(pc as usize).then_some(m)
}

// The key proves the incoming architectural LR. Carry guest x18 in the
// physical LR throughout this closed graph; every external edge ends at an
// attributed stop. The signal handler restores both registers at any PC.
enum Op {
    Exit,
    Native(u32),
    Jump(u64),
    Conditional { word: u32, bits: u32, target: u64 },
}

impl Op {
    fn decode(word: u32, pc: u64, lr: u64) -> Self {
        if word == 0xd65f_03c0 {
            return Self::Jump(lr);
        }
        if word & 0x7c00_0000 == 0x1400_0000 {
            if word >> 31 != 0 && pc + 4 != lr {
                return Self::Exit;
            }
            return Self::Jump(pc.wrapping_add_signed(super::branch_offset(word)));
        }
        let mut native = word;
        let bits = if word & 0xff00_0010 == 0x5400_0000 {
            19
        } else if word & 0x7c00_0000 == 0x3400_0000 {
            match word & 31 {
                18 => native = (word & !31) | 30,
                30 => return Self::Exit,
                _ => {}
            }
            if word & (1 << 25) == 0 { 19 } else { 14 }
        } else {
            return if loops::relocatable(word) {
                platform_register::linked_word(word).map_or(Self::Exit, Self::Native)
            } else {
                Self::Exit
            };
        };
        Self::Conditional {
            word: native,
            bits,
            target: pc.wrapping_add_signed(signed(word >> 5, bits) * 4),
        }
    }
}

struct Pair {
    key: (u64, u64),
    publications: [(u64, u64); 2],
    pcs: Box<[u64]>,
    entry: usize,
    code: NativeSteps,
}

impl Pair {
    fn build(maps: &Registry, key: (u64, u64), hint: u64) -> io::Result<Option<Self>> {
        let (target, lr) = key;
        let Some(fetched) = owner(maps, target) else {
            return Ok(None);
        };
        let native = fetched.fetch_word(target as usize).is_none();
        if lr % 4 != 0 {
            return Ok(None);
        }
        for anchor in [lr, hint] {
            let Some(caller) = owner(maps, anchor) else {
                continue;
            };
            if caller.instruction_at(anchor as usize).is_none() {
                continue;
            }
            if native {
                // A canonical x18 trap can occur anywhere in the callee. The
                // architectural link must name a direct call into its mapping.
                let Some(call) = lr.checked_sub(4) else { continue };
                if anchor != lr || !caller.native_text_word(call as usize).is_some_and(|w| {
                    w >> 26 == 0b100101
                        && fetched.contains(call.wrapping_add_signed(super::branch_offset(w)) as usize)
                }) {
                    continue;
                }
            } else {
                let start = anchor.saturating_sub(128).max(caller.address() as u64);
                // Look for an incoming direct edge. This is only the discovery
                // radius; outgoing edges follow actual control flow in both maps.
                if !(0..65).any(|i| {
                    let pc = start + i * 4;
                    caller.instruction_at(pc as usize).is_some_and(|w| {
                        w & 0x7c00_0000 == 0x1400_0000
                            && pc.wrapping_add_signed(super::branch_offset(w)) == target
                    })
                }) {
                    continue;
                }
            }
            let mut graph = BTreeMap::new();
            let mut pending = std::collections::VecDeque::from([target]);
            while let Some(pc) = pending.pop_front() {
                if graph.contains_key(&pc) {
                    continue;
                }
                let op = if graph.len() < 2048 {
                    let word = if native {
                        fetched.native_text_word(pc as usize)
                            .or_else(|| caller.native_text_word(pc as usize))
                    } else {
                        fetched.fetch_word(pc as usize)
                            .or_else(|| caller.published_word(pc as usize))
                    };
                    word.map_or(Op::Exit, |w| Op::decode(w, pc, lr))
                } else {
                    Op::Exit
                };
                match op {
                    Op::Native(_) => pending.push_back(pc + 4),
                    Op::Jump(to) => pending.push_back(to),
                    Op::Conditional { target, .. } => pending.extend([pc + 4, target]),
                    Op::Exit => {}
                }
                graph.insert(pc, op);
            }
            // A canonical loop already has a one-entry native block. Only
            // replace it if the linked run can reach a subsequent call.
            if matches!(graph[&target], Op::Exit)
                || (native && !matches!(graph.get(&(lr - 4)), Some(Op::Jump(_))))
            {
                return Ok(None);
            }
            let pcs: Box<[_]> = graph.keys().copied().collect();
            let slot = |pc| {
                pcs.binary_search(&pc)
                    .expect("reachable linked instruction")
            };
            let entry = slot(target);
            // Sorted aligned PCs keep every admitted fallthrough adjacent.
            // The bounded frontier fits even the 14-bit test-and-branch form.
            let words: Vec<_> = graph
                .into_values()
                .enumerate()
                .map(|(i, op)| {
                    let delta = |to| (slot(to) as u32).wrapping_sub(i as u32);
                    match op {
                        Op::Exit => EXIT,
                        Op::Native(w) => w,
                        Op::Jump(pc) => 0x1400_0000 | (delta(pc) & 0x03ff_ffff),
                        Op::Conditional { word, bits, target } => {
                            let mask = ((1 << bits) - 1) << 5;
                            (word & !mask) | (delta(target) << 5 & mask)
                        }
                    }
                })
                .collect();
            let code = NativeSteps::seal(&words)?.expect("nonempty linked text");
            return Ok(Some(Self {
                key,
                publications: [
                    (target, fetched.publication()),
                    (anchor, caller.publication()),
                ],
                pcs,
                entry,
                code,
            }));
        }
        Ok(None)
    }

    fn offset(&self, pc: u64) -> Option<usize> {
        let bytes = pc.checked_sub(self.code.address())?;
        (bytes % 4 == 0 && bytes / 4 < self.pcs.len() as u64).then_some((bytes / 4) as usize)
    }
}

#[derive(Default)]
pub(crate) struct Crossings {
    // Per execution thread, with a small bound on sealed allocations.
    pairs: std::collections::VecDeque<Pair>,
    previous: Option<(u64, u64)>,
    tried: bool,
    request: bool,
    // Count repeated native patch entries without allocating in the signal
    // handler. A failed compilation stays cold until either publication changes.
    native_sites: [Option<NativeSite>; 8],
}
#[derive(Clone, Copy)]
struct NativeSite {
    key: (u64, u64),
    publications: (u64, u64),
    visits: u8,
}

impl Crossings {
    pub(crate) fn begin(&mut self, maps: &Registry) {
        self.pairs.retain(|p| {
            p.publications
                .iter()
                .all(|&(pc, id)| owner(maps, pc).map(Mapping::publication) == Some(id))
        });
        self.previous = None;
        self.tried = false;
        self.request = false;
    }

    // Called with guest signals masked in the signal handler. No allocation
    // or locking. The registry read guard spans the entire native transition.
    pub(crate) fn observe(&mut self, pc: u64, lr: u64) -> bool {
        let key = (pc, lr);
        if self.previous != Some(key) {
            self.previous = Some(key);
            self.tried = false;
        } else if !self.tried {
            self.tried = true;
            self.request = true;
            return true;
        }
        false
    }

    pub(crate) fn requested_native_link(&self) -> bool {
        self.request
    }

    pub(crate) fn observe_native(&mut self, pc: u64, lr: u64, callee: &Mapping, maps: &Registry) -> bool {
        let Some(caller) = owner(maps, lr) else { return false };
        let key = (pc, lr);
        let publications = (callee.publication(), caller.publication());
        let index = ((pc >> 2) ^ (lr >> 2)) as usize % self.native_sites.len();
        let slot = &mut self.native_sites[index];
        let site = slot.get_or_insert(NativeSite { key, publications, visits: 0 });
        if site.key != key || site.publications != publications {
            *site = NativeSite { key, publications, visits: 0 };
        }
        site.visits = site.visits.saturating_add(1);
        if site.visits == 8 {
            self.request = true;
            return true;
        }
        false
    }

    pub(crate) fn prepare(
        &mut self,
        pc: u64,
        lr: u64,
        hint: u64,
        maps: &Registry,
    ) -> io::Result<()> {
        if std::mem::take(&mut self.request)
            && let Some(pair) = Pair::build(maps, (pc, lr), hint)?
        {
            if self.pairs.len() == 4 {
                self.pairs.pop_front();
            }
            self.pairs.push_back(pair);
        }
        Ok(())
    }

    pub(crate) fn entry(&self, pc: u64, lr: u64) -> Option<u64> {
        self.pairs
            .iter()
            .find(|p| p.key == (pc, lr))
            .map(|p| p.code.address() + p.entry as u64 * 4)
    }

    pub(crate) fn original_pc(&self, pc: u64) -> Option<u64> {
        self.pairs.iter().find_map(|p| Some(p.pcs[p.offset(pc)?]))
    }

    pub(crate) fn architectural_lr(&self, pc: u64) -> Option<u64> {
        self.pairs
            .iter()
            .find_map(|p| p.offset(pc).map(|_| p.key.1))
    }
}
