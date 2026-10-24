//! Builds the PILout expression list. In optimized mode, constants are folded,
//! identities that hold for every field value are applied (`x + 0 = x`,
//! `x · 1 = x`, `x · 0 = 0`, `x - x = 0`), and a repeated operation reuses the
//! existing expression. Nothing is assumed about witness values, such as being bits.
use crate::{
    contract::{STAGE_ONE, STAGE_TWO},
    field::{add, mul, neg},
    proto as pb,
};
use pb::{expression::Operation, operand::Operand};
use prost::Message;
use std::collections::BTreeMap;

/// A binary operation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Op {
    Add,
    Sub,
    Mul,
}

/// Wrap a protobuf operand variant.
pub fn operand(value: Operand) -> pb::Operand {
    pb::Operand {
        operand: Some(value),
    }
}
/// A field element as big-endian bytes without leading zeros (zero is empty).
pub fn field_bytes(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let start = bytes.iter().position(|&b| b != 0).unwrap_or(8);
    bytes[start..].to_vec()
}
/// A constant operand.
pub fn constant(value: u64) -> pb::Operand {
    operand(Operand::Constant(pb::operand::Constant {
        value: field_bytes(value),
    }))
}
/// The witness column `col_idx` of `stage`, at the current row.
pub fn witness(stage: u32, col_idx: u32) -> pb::Operand {
    operand(Operand::WitnessCol(pb::operand::WitnessCol {
        stage,
        col_idx,
        row_offset: 0,
    }))
}
/// Stage-one column `col_idx`, at the current row.
pub fn trace(col_idx: u32) -> pb::Operand {
    witness(STAGE_ONE, col_idx)
}
/// Stage-two column `col_idx`, at the current row.
pub fn stage_two(col_idx: u32) -> pb::Operand {
    witness(STAGE_TWO, col_idx)
}
/// Stage-two challenge `idx`.
pub fn challenge(idx: u32) -> pb::Operand {
    operand(Operand::Challenge(pb::operand::Challenge {
        stage: STAGE_TWO,
        idx,
    }))
}
fn value(x: &pb::Operand) -> Option<u64> {
    match x.operand.as_ref()? {
        Operand::Constant(c) => Some(c.value.iter().fold(0, |v, b| (v << 8) | u64::from(*b))),
        _ => None,
    }
}

/// An operation and its encoded operands.
type Key = (Op, Vec<u8>, Vec<u8>);
#[derive(Default)]
pub struct Dag {
    /// Expressions in PILout order; operands only refer to earlier entries.
    pub nodes: Vec<pb::Expression>,
    /// Whether [`Dag::push`] simplifies and reuses expressions.
    pub optimize: bool,
    /// Existing expression for each operation, with `+` and `·` operands sorted.
    cache: BTreeMap<Key, pb::Operand>,
}
impl Dag {
    /// An empty expression list; with `optimize`, expressions are simplified.
    pub fn new(optimize: bool) -> Self {
        Self {
            optimize,
            ..Default::default()
        }
    }
    /// Append `lhs kind rhs` as a new expression and return a reference to it.
    pub fn raw(&mut self, kind: Op, lhs: pb::Operand, rhs: pb::Operand) -> pb::Operand {
        let node = match kind {
            Op::Add => Operation::Add(pb::expression::Add {
                lhs: Some(lhs),
                rhs: Some(rhs),
            }),
            Op::Sub => Operation::Sub(pb::expression::Sub {
                lhs: Some(lhs),
                rhs: Some(rhs),
            }),
            Op::Mul => Operation::Mul(pb::expression::Mul {
                lhs: Some(lhs),
                rhs: Some(rhs),
            }),
        };
        let idx = self.nodes.len() as u32;
        self.nodes.push(pb::Expression {
            operation: Some(node),
        });
        operand(Operand::Expression(pb::operand::Expression { idx }))
    }
    /// Like [`Dag::raw`], but in optimized mode first try to simplify the
    /// operation or reuse an existing expression.
    pub fn push(&mut self, kind: Op, mut lhs: pb::Operand, mut rhs: pb::Operand) -> pb::Operand {
        if !self.optimize {
            return self.raw(kind, lhs, rhs);
        }
        let (a, b) = (value(&lhs), value(&rhs));
        if let (Some(a), Some(b)) = (a, b) {
            return constant(match kind {
                Op::Add => add(a, b),
                Op::Sub => add(a, neg(b)),
                Op::Mul => mul(a, b),
            });
        }
        match kind {
            Op::Add if a == Some(0) => return rhs,
            Op::Add | Op::Sub if b == Some(0) => return lhs,
            Op::Sub if lhs == rhs => return constant(0),
            Op::Mul if a == Some(0) || b == Some(0) => return constant(0),
            Op::Mul if a == Some(1) => return rhs,
            Op::Mul if b == Some(1) => return lhs,
            _ => {}
        }
        let (mut x, mut y) = (lhs.encode_to_vec(), rhs.encode_to_vec());
        if kind != Op::Sub && y < x {
            std::mem::swap(&mut x, &mut y);
            std::mem::swap(&mut lhs, &mut rhs);
        }
        let key = (kind, x, y);
        if let Some(existing) = self.cache.get(&key) {
            return existing.clone();
        }
        let result = self.raw(kind, lhs, rhs);
        self.cache.insert(key, result.clone());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::P;
    fn optimized() -> Dag {
        Dag {
            optimize: true,
            ..Default::default()
        }
    }

    #[test]
    fn constant_folding_uses_the_field() {
        let mut d = optimized();
        assert_eq!(d.push(Op::Add, constant(P - 1), constant(1)), constant(0));
        assert_eq!(d.push(Op::Sub, constant(0), constant(1)), constant(P - 1));
        assert_eq!(
            d.push(Op::Mul, constant(P - 1), constant(P - 1)),
            constant(1)
        );
        assert!(d.nodes.is_empty());
    }
    #[test]
    fn commutative_reuse_preserves_subtraction_order() {
        let mut d = optimized();
        let x = witness(1, 0);
        let y = witness(1, 1);
        assert_eq!(
            d.push(Op::Mul, x.clone(), y.clone()),
            d.push(Op::Mul, y.clone(), x.clone())
        );
        assert_ne!(d.push(Op::Sub, x.clone(), y.clone()), d.push(Op::Sub, y, x));
        assert_eq!(d.nodes.len(), 3);
    }
    #[test]
    fn identities_do_not_assume_bit_values() {
        let mut d = optimized();
        let x = witness(1, 0);
        assert_eq!(d.push(Op::Mul, constant(1), x.clone()), x);
        assert_eq!(d.push(Op::Add, x.clone(), constant(0)), x);
        assert_eq!(d.push(Op::Sub, x.clone(), x.clone()), constant(0));
        assert_ne!(d.push(Op::Mul, x.clone(), x.clone()), x);
    }
    #[test]
    fn unoptimized_mode_keeps_every_operation() {
        let mut d = Dag::default();
        assert_ne!(d.push(Op::Mul, constant(1), witness(1, 0)), witness(1, 0));
        assert_eq!(d.nodes.len(), 1);
    }
}
