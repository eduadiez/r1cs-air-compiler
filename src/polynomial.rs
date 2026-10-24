//! Exact polynomials over the field, and an evaluator that turns AIR
//! expressions into polynomials. Used only by the checkers.
use crate::{
    contract::{
        ALPHA, GAMMA, MAX_EVALUATED_TERMS, MAX_PRODUCT_TERMS, PRODUCT_COLUMN, STAGE_ONE, STAGE_TWO,
    },
    ensure,
    field::{add, mul, neg},
    pilout_check::{binary_operation, scalar, BinaryOp},
    proto as pb, Context, Result,
};
use std::collections::BTreeMap;

/// A polynomial variable.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Var {
    Witness { stage: u32, column: u32 },
    Public(u32),
    Fixed(u32),
    Challenge { stage: u32, index: u32 },
}
impl Var {
    /// Stage-one column `column`.
    pub fn trace(column: u32) -> Self {
        Var::Witness {
            stage: STAGE_ONE,
            column,
        }
    }
    /// Stage-two column 0.
    pub fn product() -> Self {
        Var::Witness {
            stage: STAGE_TWO,
            column: PRODUCT_COLUMN,
        }
    }
}

/// Polynomial as a map from monomial (a sorted list of variables) to its
/// nonzero coefficient.
pub type Polynomial = BTreeMap<Vec<Var>, u64>;

/// The constant polynomial `x`.
pub fn constant(x: u64) -> Polynomial {
    if x == 0 {
        BTreeMap::new()
    } else {
        BTreeMap::from([(vec![], x)])
    }
}
/// The polynomial `v`.
pub fn variable(v: Var) -> Polynomial {
    BTreeMap::from([(vec![v], 1)])
}
/// Add `value` to the coefficient of `key`, removing it if it becomes zero.
pub fn accumulate<K: Ord>(p: &mut BTreeMap<K, u64>, key: K, value: u64) {
    let value = add(p.get(&key).copied().unwrap_or(0), value);
    if value == 0 {
        p.remove(&key);
    } else {
        p.insert(key, value);
    }
}
/// `a + b`.
pub fn sum(a: &Polynomial, b: &Polynomial) -> Polynomial {
    let mut result = a.clone();
    for (key, value) in b {
        accumulate(&mut result, key.clone(), *value);
    }
    result
}
/// `a - b`.
pub fn difference(a: &Polynomial, b: &Polynomial) -> Polynomial {
    let mut result = a.clone();
    for (key, value) in b {
        accumulate(&mut result, key.clone(), neg(*value));
    }
    result
}
/// `a · b`. Fails above degree 3 or `MAX_PRODUCT_TERMS` term products.
pub fn product(a: &Polynomial, b: &Polynomial) -> Result<Polynomial> {
    ensure(
        a.len().saturating_mul(b.len()) <= MAX_PRODUCT_TERMS,
        "polynomial product size limit",
    )?;
    let mut result = BTreeMap::new();
    for (x, a) in a {
        for (y, b) in b {
            let mut key = [x.as_slice(), y.as_slice()].concat();
            ensure(key.len() <= 3, "polynomial degree above 3")?;
            key.sort();
            accumulate(&mut result, key, mul(*a, *b));
        }
    }
    Ok(result)
}

/// Evaluates the AIR's expressions, in order, to polynomials.
pub struct Interpreter<'a> {
    air: &'a pb::Air,
    /// Number of stage-one columns.
    width: u32,
    publics: u32,
    /// Polynomial of each expression evaluated so far.
    pub values: Vec<Polynomial>,
}
impl<'a> Interpreter<'a> {
    /// An evaluator for `air` with `width` stage-one columns and `publics`
    /// public values; no expression is evaluated yet.
    pub fn new(air: &'a pb::Air, width: u32, publics: u32) -> Self {
        Self {
            air,
            width,
            publics,
            values: vec![],
        }
    }
    /// Polynomial of one operand. Only current-row references are allowed, and
    /// stage two may only be column 0.
    pub fn operand(&self, operand: &pb::Operand) -> Result<Polynomial> {
        use pb::operand::Operand;
        match operand.operand.as_ref().ok_or("missing operand")? {
            Operand::Constant(x) => Ok(constant(scalar(&x.value)?)),
            Operand::WitnessCol(x) => {
                ensure(x.row_offset == 0, "row offsets are not allowed here")?;
                ensure(
                    (x.stage == STAGE_ONE && x.col_idx < self.width)
                        || (x.stage == STAGE_TWO && x.col_idx == PRODUCT_COLUMN),
                    "witness column out of range",
                )?;
                Ok(variable(Var::Witness {
                    stage: x.stage,
                    column: x.col_idx,
                }))
            }
            Operand::PublicValue(x) => {
                ensure(x.idx < self.publics, "public value out of range")?;
                Ok(variable(Var::Public(x.idx)))
            }
            Operand::FixedCol(x) => {
                ensure(
                    x.idx < self.air.fixed_cols.len() as u32 && x.row_offset == 0,
                    "fixed column out of range or with a row offset",
                )?;
                Ok(variable(Var::Fixed(x.idx)))
            }
            Operand::Challenge(x) => {
                ensure(
                    x.stage == STAGE_TWO && (x.idx == ALPHA || x.idx == GAMMA),
                    "challenge out of range",
                )?;
                Ok(variable(Var::Challenge {
                    stage: x.stage,
                    index: x.idx,
                }))
            }
            Operand::Expression(x) => Ok(self
                .values
                .get(x.idx as usize)
                .ok_or("expression refers to itself or a later expression")?
                .clone()),
            Operand::ProofValue(_)
            | Operand::AirGroupValue(_)
            | Operand::PeriodicCol(_)
            | Operand::AirValue(_)
            | Operand::CustomCol(_) => Err("unsupported operand type".into()),
        }
    }
    /// Evaluate the first `limit` expressions. An expression may only refer to
    /// earlier ones.
    pub fn evaluate_until(&mut self, limit: usize) -> Result<()> {
        let mut terms = 0usize;
        for (index, expression) in self.air.expressions.iter().take(limit).enumerate() {
            let (op, lhs, rhs) =
                binary_operation(expression).context(format!("expression {index}"))?;
            let lhs = self
                .operand(lhs)
                .context(format!("expression {index}, lhs"))?;
            let rhs = self
                .operand(rhs)
                .context(format!("expression {index}, rhs"))?;
            let value = match op {
                BinaryOp::Add => sum(&lhs, &rhs),
                BinaryOp::Sub => difference(&lhs, &rhs),
                BinaryOp::Mul => {
                    product(&lhs, &rhs).context(format!("expression {index}, product"))?
                }
            };
            terms = terms.saturating_add(value.len());
            ensure(
                terms <= MAX_EVALUATED_TERMS,
                "evaluated expressions exceed the term limit",
            )?;
            self.values.push(value);
        }
        Ok(())
    }
}
