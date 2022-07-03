use crate::zero::values::ZeroValuesPov;
use std::borrow::{Borrow, Cow};

pub mod node;
pub mod tree;

pub mod step;
pub mod wrapper;

/// A board evaluation, either as returned by the network or as the final output of a zero tree search.
#[derive(Debug, Clone)]
pub struct MuZeroEvaluation<'a> {
    /// The (normalized) values.
    pub values: ZeroValuesPov,

    /// The (un-normalized) policy "vector", containing all possible moves.
    pub policy_logits: Cow<'a, [f32]>,
}

impl MuZeroEvaluation<'_> {
    pub fn shallow_clone(&self) -> MuZeroEvaluation {
        MuZeroEvaluation {
            values: self.values,
            policy_logits: Cow::Borrowed(self.policy_logits.borrow()),
        }
    }
}
