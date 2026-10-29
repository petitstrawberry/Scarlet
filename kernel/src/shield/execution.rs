//! Monotonic, ABI-neutral constraints on entry into the kernel.
//! Rules are immutable and inherited by value; adding a rule cannot remove an
//! ancestor's restriction. Adapters own call numbering and rejection encoding.
use alloc::{sync::Arc, vec::Vec};

pub struct Invocation {
    pub domain: &'static str,
    pub number: u64,
    pub instruction: u64,
    pub arguments: [u64; 6],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denial {
    Return(usize),
    AbortThread,
    AbortProcess,
    Exception { kind: u32, detail: u32 },
}
#[derive(Clone, Copy)]
pub struct Verdict {
    /// Lower ranks dominate. Equal ranks select the most recently added rule.
    pub rank: u8,
    pub denial: Option<Denial>,
}
impl Verdict {
    pub const ALLOW: Self = Self {
        rank: u8::MAX,
        denial: None,
    };
    pub const ABORT: Self = Self {
        rank: 0,
        denial: Some(Denial::AbortProcess),
    };
}
pub trait CallConstraint: Send + Sync {
    fn evaluate(&self, call: &Invocation) -> Verdict;
}
#[derive(Clone, Default)]
pub struct Restrictions {
    no_new_authority: bool,
    rules: Vec<Arc<dyn CallConstraint>>,
    cost: usize,
}
impl Restrictions {
    pub fn no_new_authority(&self) -> bool {
        self.no_new_authority
    }
    pub fn seal_authority(&mut self) {
        self.no_new_authority = true;
    }
    pub fn is_constrained(&self) -> bool {
        !self.rules.is_empty()
    }
    pub fn append(&mut self, rule: Arc<dyn CallConstraint>, cost: usize) -> Result<(), ()> {
        let cost = self
            .cost
            .checked_add(cost)
            .and_then(|v| v.checked_add(4))
            .ok_or(())?;
        if cost > 32768 || self.rules.len() >= 256 {
            return Err(());
        }
        self.rules.push(rule);
        self.cost = cost;
        Ok(())
    }
    pub fn evaluate(&self, call: &Invocation) -> Option<Denial> {
        let mut result = Verdict::ALLOW;
        for rule in self.rules.iter().rev() {
            let next = rule.evaluate(call);
            if next.rank < result.rank {
                result = next;
            }
        }
        result.denial
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Rule(u8, usize);
    impl CallConstraint for Rule {
        fn evaluate(&self, _: &Invocation) -> Verdict {
            Verdict {
                rank: self.0,
                denial: Some(Denial::Return(self.1)),
            }
        }
    }
    #[test_case]
    fn restrictions_keep_ancestor_denials_and_newest_equal_rank() {
        let call = Invocation {
            domain: "test",
            number: 1,
            instruction: 0,
            arguments: [0; 6],
        };
        let mut parent = Restrictions::default();
        parent.seal_authority();
        parent.append(Arc::new(Rule(4, 7)), 4).unwrap();
        let mut child = parent.clone();
        child.append(Arc::new(Rule(5, 8)), 4).unwrap();
        assert_eq!(child.evaluate(&call), Some(Denial::Return(7)));
        child.append(Arc::new(Rule(4, 9)), 4).unwrap();
        assert_eq!(child.evaluate(&call), Some(Denial::Return(9)));
        assert_eq!(parent.evaluate(&call), Some(Denial::Return(7)));
        assert!(child.no_new_authority());
        assert!(child.append(Arc::new(Rule(0, 0)), usize::MAX).is_err());
        assert_eq!(child.evaluate(&call), Some(Denial::Return(9)));
    }
}
