//! Evaluates a compiled AIR on a concrete trace, as a prover and verifier
//! would: the stage-two columns are filled from the hints, then every
//! constraint is evaluated on every row. It reads only the PILout and uses
//! neither the compiler nor the checkers, so it is an independent oracle for
//! the soundness and completeness properties of `SPEC.md`.
//!
//! Filling stage two from the hints loses no generality: given nonzero
//! denominators, the ratio and running product constraints determine those
//! columns uniquely, and the `stage2_zero` constraint forces its column to 0.
use crate::{
    field::{add, inverse, mul, neg},
    proto as pb,
};
use pb::{constraint::Constraint, expression::Operation, operand::Operand};
use std::collections::BTreeMap;

/// Why a trace does not satisfy an AIR.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// Constraint `constraint` is nonzero on row `row`.
    Violated { constraint: usize, row: usize },
    /// A hint divides by zero for these challenges, so stage two cannot be
    /// filled; the copy argument's failure case of `SPEC.md`.
    ZeroDenominator,
}

/// A stage-one trace (row by row), the public values and the two stage-two
/// challenges, in index order.
pub struct Trace<'a> {
    pub stage_one: &'a [u64],
    pub publics: &'a [u64],
    pub challenges: [u64; 2],
}

/// Fill stage two from the hints and evaluate every constraint on every row.
pub fn evaluate(pilout: &pb::PilOut, trace: &Trace) -> Result<(), Failure> {
    let air = &pilout.air_groups[0].airs[0];
    let rows = air.num_rows.unwrap() as usize;
    let widths = &air.stage_widths;
    assert_eq!(trace.stage_one.len(), rows * widths[0] as usize);
    let mut columns = Columns {
        air,
        rows,
        fixed: air
            .fixed_cols
            .iter()
            .map(|column| column.values.iter().map(|v| field(v)).collect())
            .collect(),
        stage_one: trace.stage_one,
        width: widths[0] as usize,
        stage_two: vec![None; widths[1] as usize],
        publics: trace.publics,
        challenges: trace.challenges,
    };
    for hint in &pilout.hints {
        columns.fill(hint)?;
    }
    assert!(
        columns.stage_two.iter().all(Option::is_some),
        "a stage-two column has no hint"
    );
    for row in 0..rows {
        let mut memo = vec![None; air.expressions.len()];
        for (index, constraint) in air.constraints.iter().enumerate() {
            let (root, applies) = match constraint.constraint.as_ref().unwrap() {
                Constraint::EveryRow(c) => (c.expression_idx.unwrap().idx, true),
                Constraint::FirstRow(c) => (c.expression_idx.unwrap().idx, row == 0),
                Constraint::LastRow(c) => (c.expression_idx.unwrap().idx, row == rows - 1),
                Constraint::EveryFrame(_) => panic!("frame constraints are not supported"),
            };
            if applies && columns.expression(root as usize, row, &mut memo) != 0 {
                return Err(Failure::Violated {
                    constraint: index,
                    row,
                });
            }
        }
    }
    Ok(())
}

/// A field element in the PILout's big-endian encoding.
fn field(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |value, byte| (value << 8) | u64::from(*byte))
}

struct Columns<'a> {
    air: &'a pb::Air,
    rows: usize,
    fixed: Vec<Vec<u64>>,
    stage_one: &'a [u64],
    width: usize,
    stage_two: Vec<Option<Vec<u64>>>,
    publics: &'a [u64],
    challenges: [u64; 2],
}

impl Columns<'_> {
    /// Row `row + offset`, wrapping around the trace.
    fn shifted(&self, row: usize, offset: i32) -> usize {
        (row as i64 + i64::from(offset)).rem_euclid(self.rows as i64) as usize
    }

    fn operand(&self, operand: &pb::Operand, row: usize, memo: &mut [Option<u64>]) -> u64 {
        match operand.operand.as_ref().unwrap() {
            Operand::Constant(c) => field(&c.value),
            Operand::Challenge(c) => {
                assert_eq!(c.stage, 2, "only stage-two challenges are expected");
                self.challenges[c.idx as usize]
            }
            Operand::PublicValue(p) => self.publics[p.idx as usize],
            Operand::FixedCol(f) => self.fixed[f.idx as usize][self.shifted(row, f.row_offset)],
            Operand::WitnessCol(w) => {
                let row = self.shifted(row, w.row_offset);
                match w.stage {
                    1 => self.stage_one[row * self.width + w.col_idx as usize],
                    2 => self.stage_two[w.col_idx as usize]
                        .as_ref()
                        .expect("stage-two column read before its hint")[row],
                    stage => panic!("unexpected stage {stage}"),
                }
            }
            Operand::Expression(e) => self.expression(e.idx as usize, row, memo),
            other => panic!("unsupported operand {other:?}"),
        }
    }

    fn expression(&self, index: usize, row: usize, memo: &mut [Option<u64>]) -> u64 {
        if let Some(value) = memo[index] {
            return value;
        }
        let operand = |o: &Option<pb::Operand>, memo: &mut [Option<u64>]| {
            self.operand(o.as_ref().unwrap(), row, memo)
        };
        let value = match self.air.expressions[index].operation.as_ref().unwrap() {
            Operation::Add(x) => add(operand(&x.lhs, memo), operand(&x.rhs, memo)),
            Operation::Sub(x) => add(operand(&x.lhs, memo), neg(operand(&x.rhs, memo))),
            Operation::Mul(x) => mul(operand(&x.lhs, memo), operand(&x.rhs, memo)),
            Operation::Neg(x) => neg(operand(&x.value, memo)),
        };
        memo[index] = Some(value);
        value
    }

    /// The stage-two column that a hint's `reference` field names: the column
    /// itself (`im_col`) or the expression `column + 0` (the other hints).
    fn reference(&self, operand: &pb::Operand) -> usize {
        let column = match &operand.operand {
            Some(Operand::Expression(e)) => match &self.air.expressions[e.idx as usize].operation {
                Some(Operation::Add(x)) => x.lhs.as_ref().and_then(|o| o.operand.as_ref()),
                other => panic!("hint reference is {other:?}"),
            },
            other => other.as_ref(),
        };
        match column {
            Some(Operand::WitnessCol(w)) if w.stage == 2 && w.row_offset == 0 => w.col_idx as usize,
            other => panic!("hint reference names {other:?}"),
        }
    }

    /// Compute the column of one hint on every row:
    /// - `im_col`: `numerator / denominator`;
    /// - `gprod_col`: the running product of `numerator_air / denominator_air`,
    ///   starting from 1;
    /// - `gsum_col`: the running sum of the same ratio, starting from 0.
    fn fill(&mut self, hint: &pb::Hint) -> Result<(), Failure> {
        let Some(pb::hint_field::Value::HintFieldArray(array)) = &hint.hint_fields[0].value else {
            panic!("hint without a field array")
        };
        let fields: BTreeMap<&str, &pb::Operand> = array
            .hint_fields
            .iter()
            .map(|f| match &f.value {
                Some(pb::hint_field::Value::Operand(o)) => (f.name.as_deref().unwrap(), o),
                _ => panic!("hint field is not an operand"),
            })
            .collect();
        let (numerator, denominator) = match hint.name.as_str() {
            "im_col" => (fields["numerator"], fields["denominator"]),
            "gprod_col" | "gsum_col" => {
                // The direct terms are global and must leave the column as is:
                // a ratio of 1 for a product, 0 for a sum.
                let constant = |name: &str| match &fields[name].operand {
                    Some(Operand::Constant(c)) => field(&c.value),
                    _ => panic!("{name} is not a constant"),
                };
                let identity = u64::from(hint.name == "gprod_col");
                assert_eq!(constant("numerator_direct"), identity);
                assert_eq!(constant("denominator_direct"), 1);
                (fields["numerator_air"], fields["denominator_air"])
            }
            name => panic!("unexpected hint {name}"),
        };
        let column = self.reference(fields["reference"]);
        let mut values = Vec::with_capacity(self.rows);
        let mut accumulated = if hint.name == "gsum_col" { 0 } else { 1 };
        for row in 0..self.rows {
            let mut memo = vec![None; self.air.expressions.len()];
            let den = self.operand(denominator, row, &mut memo);
            if den == 0 {
                return Err(Failure::ZeroDenominator);
            }
            let ratio = mul(self.operand(numerator, row, &mut memo), inverse(den));
            accumulated = match hint.name.as_str() {
                "im_col" => ratio,
                "gprod_col" => mul(accumulated, ratio),
                _ => add(accumulated, ratio),
            };
            values.push(accumulated);
        }
        self.stage_two[column] = Some(values);
        Ok(())
    }
}

/// The evaluator accepts the honest trace of the xor fixture and rejects it
/// after each kind of change, so the property tests that rely on it cannot
/// pass by accepting everything.
#[test]
fn the_xor_trace_satisfies_its_air_and_changes_do_not() {
    use crate::api;
    let source = include_bytes!("../../fixtures/xor/kernel.r1cs");
    let wtns = include_bytes!("../../fixtures/xor/input.wtns");
    let artifact = api::compile_packed(source, &super::packed::config(8)).unwrap();
    let witness =
        api::project_witness(source, &artifact.pilout, &artifact.certificate, wtns).unwrap();
    let pilout = <pb::PilOut as prost::Message>::decode(&artifact.pilout[..]).unwrap();
    let challenges = [0x1234_5678_9abc_def0, 0x0fed_cba9_8765_4321];
    let check = |stage_one: &[u64], publics: &[u64]| {
        let trace = Trace {
            stage_one,
            publics,
            challenges,
        };
        evaluate(&pilout, &trace)
    };
    assert_eq!(check(&witness.trace, &witness.publics), Ok(()));
    // One cell of a wire with several cells: the copy argument fails.
    let wire = (1..)
        .find(|w| artifact.map.iter().filter(|m| *m == w).count() > 1)
        .unwrap();
    let cell = artifact.map.iter().position(|m| *m == wire).unwrap();
    let mut trace = witness.trace.clone();
    trace[cell] = add(trace[cell], 1);
    assert!(matches!(
        check(&trace, &witness.publics),
        Err(Failure::Violated { .. })
    ));
    // Every cell of that wire: the copy argument holds, an equation fails.
    let mut trace = witness.trace.clone();
    for (value, w) in trace.iter_mut().zip(&artifact.map) {
        if *w == wire {
            *value = add(*value, 1);
        }
    }
    assert!(matches!(
        check(&trace, &witness.publics),
        Err(Failure::Violated { .. })
    ));
    // A public value.
    let mut publics = witness.publics.clone();
    publics[0] = add(publics[0], 1);
    assert!(matches!(
        check(&witness.trace, &publics),
        Err(Failure::Violated { .. })
    ));
}
