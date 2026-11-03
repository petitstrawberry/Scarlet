//! Compact equivalents of one app-manager file delegation batch. These sets
//! are snapshots, not growing global sets: later apps must not retroactively
//! expand an earlier batch's source/target combinations.
use alloc::{collections::BTreeSet, vec::Vec};
use scarlet_abi::shield::{
    RawLabelSelector as Selector, RawLabelSet, RawSetFileRule, RawSetMember,
};

pub struct FileBatch {
    pub sets: Vec<RawLabelSet>,
    pub members: Vec<RawSetMember>,
    pub rules: Vec<RawSetFileRule>,
    pub next_set: u32,
}
impl FileBatch {
    pub fn new(
        previous: &BTreeSet<u32>,
        current: &BTreeSet<u32>,
        base: &BTreeSet<u32>,
        first_set: u32,
        installer: u32,
        zygote: u32,
    ) -> Result<Self, &'static str> {
        if first_set == 0 {
            return Err("zero label set");
        }
        let next_set = first_set
            .checked_add(3)
            .ok_or("label set identity exhaustion")?;
        let old = first_set;
        let new = first_set + 1;
        let cleanup = first_set + 2;
        let mut members = Vec::new();
        let mut origins = previous.clone();
        origins.extend(current);
        origins.extend(base);
        members.extend(
            origins
                .iter()
                .map(|&label| RawSetMember { set: old, label }),
        );
        members.extend(
            current
                .iter()
                .map(|&label| RawSetMember { set: new, label }),
        );
        members.extend(base.difference(current).map(|&label| RawSetMember {
            set: cleanup,
            label,
        }));
        let mut rules = Vec::new();
        let mut push = |source, target, kind, action, flags| {
            rules.push(RawSetFileRule {
                source: Selector::label(source),
                origin: Selector::set(old),
                target: Selector::set(target),
                kind,
                action,
                flags,
                reserved: 0,
            })
        };
        for kind in [1, 2] {
            push(installer, new, kind, 1, 0);
            for source in [installer, zygote] {
                push(source, new, kind, 2, 1);
            }
        }
        push(zygote, new, 3, 2, 1);
        for kind in [1, 2, 3] {
            push(zygote, cleanup, kind, 2, 1);
        }
        Ok(Self {
            sets: (first_set..next_set)
                .map(|id| RawLabelSet { id, reserved: 0 })
                .collect(),
            members,
            rules,
            next_set,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn later_apps_do_not_expand_permissions_of_previous_batches() {
        let first_labels = [20].into_iter().collect();
        let second_labels = [21].into_iter().collect();
        let base = [2].into_iter().collect();
        let first = FileBatch::new(&BTreeSet::new(), &first_labels, &base, 1, 3, 4).unwrap();
        let second =
            FileBatch::new(&first_labels, &second_labels, &base, first.next_set, 3, 4).unwrap();
        let permitted = |origin, target| {
            [&first, &second].iter().any(|batch| {
                batch.rules.iter().any(|rule| {
                    rule.action == 2
                        && rule.source.id == 3
                        && rule.kind == 1
                        && origin != target
                        && batch
                            .members
                            .iter()
                            .any(|m| m.set == rule.origin.id && m.label == origin)
                        && batch
                            .members
                            .iter()
                            .any(|m| m.set == rule.target.id && m.label == target)
                })
            })
        };
        assert!(permitted(20, 21));
        assert!(!permitted(21, 20));
        assert!(permitted(2, 20));
        assert!(permitted(2, 21));
    }
    #[test]
    fn compact_batch_exactly_matches_prior_expansion_including_overlapping_base_labels() {
        for current in [
            (20..124).collect::<BTreeSet<_>>(),
            [2, 3, 20].into_iter().collect(),
        ] {
            let previous = (10..20).collect::<BTreeSet<_>>();
            let base = [2, 3, 4].into_iter().collect::<BTreeSet<_>>();
            let compact = FileBatch::new(&previous, &current, &base, 100, 1, 5).unwrap();
            assert_eq!(compact.rules.len(), 10);
            let mut old = previous.clone();
            old.extend(&current);
            old.extend(&base);
            let mut expected = BTreeSet::new();
            for &target in &current {
                for &parent in &old {
                    for kind in [1, 2] {
                        expected.insert((1, 1, parent, target, kind));
                        if parent != target {
                            for source in [1, 5] {
                                expected.insert((2, source, parent, target, kind));
                            }
                        }
                    }
                    if parent != target {
                        expected.insert((2, 5, parent, target, 3));
                    }
                }
            }
            for &target in base.difference(&current) {
                for &parent in &old {
                    if parent != target {
                        for kind in [1, 2, 3] {
                            expected.insert((2, 5, parent, target, kind));
                        }
                    }
                }
            }
            let mut actual = BTreeSet::new();
            for rule in &compact.rules {
                for origin in compact.members.iter().filter(|m| m.set == rule.origin.id) {
                    for target in compact.members.iter().filter(|m| m.set == rule.target.id) {
                        if rule.flags == 0 || origin.label != target.label {
                            actual.insert((
                                rule.action,
                                rule.source.id,
                                origin.label,
                                target.label,
                                rule.kind,
                            ));
                        }
                    }
                }
            }
            assert_eq!(actual, expected);
            assert!(compact.members.len() < 300);
            assert!(!compact.members.iter().any(|member| member.label == 999));
        }
    }
}
