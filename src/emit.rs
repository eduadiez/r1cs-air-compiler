//! PILout construction shared by the reference and packed compilers. The
//! checkers do not use this module.
use crate::{
    contract::{
        label, AIR_NAME, ALPHA, CHALLENGE_NAMES, GAMMA, PILOUT_NAME, PRODUCT_COLUMN,
        PUBLICS_SYMBOL, STAGE_ONE, STAGE_TWO, SUM_HINT, TRACE_SYMBOL,
    },
    dag::{challenge, constant, field_bytes, operand, stage_two, trace, Dag, Op},
    proto as pb, r1cs, P,
};
use pb::{operand::Operand, SymbolType};
use prost::Message;

/// A symbol of the single AIR.
pub fn symbol(
    name: &str,
    kind: SymbolType,
    id: u32,
    stage: Option<u32>,
    lengths: Vec<u32>,
) -> pb::Symbol {
    pb::Symbol {
        name: name.into(),
        air_group_id: Some(0),
        air_id: Some(0),
        r#type: kind as i32,
        id,
        stage,
        dim: lengths.len() as u32,
        lengths,
        ..Default::default()
    }
}

/// The AIR being built, with its expressions, symbols and hints.
pub struct Builder {
    pub air: pb::Air,
    pub dag: Dag,
    pub symbols: Vec<pb::Symbol>,
    pub hints: Vec<pb::Hint>,
}
impl Builder {
    /// An AIR with `rows` rows, `width` stage-one columns and one stage-two
    /// column named `product`. With `optimize`, expressions are simplified.
    pub fn new(rows: u32, width: u32, product: &str, optimize: bool) -> Self {
        Self {
            air: pb::Air {
                name: Some(AIR_NAME.into()),
                num_rows: Some(rows),
                stage_widths: vec![width, 1],
                aggregable: true,
                ..Default::default()
            },
            dag: Dag::new(optimize),
            symbols: vec![
                symbol(
                    TRACE_SYMBOL,
                    SymbolType::WitnessCol,
                    0,
                    Some(STAGE_ONE),
                    vec![width],
                ),
                symbol(
                    product,
                    SymbolType::WitnessCol,
                    PRODUCT_COLUMN,
                    Some(STAGE_TWO),
                    vec![],
                ),
            ],
            hints: vec![],
        }
    }

    /// Constrain stage-two column 0 to zero and add its `gsum_col` hint.
    /// Artifacts without a permutation argument use this column (`stage2_zero`), so
    /// both artifact kinds have a stage two. The hint numerator is written as
    /// `α - α`: zero, but an expression over a stage-two challenge. The
    /// expressions are never simplified, whatever the optimization mode.
    pub fn constrain_zero_column(&mut self) {
        self.constrain(stage_two(PRODUCT_COLUMN), label::ZERO_COLUMN.into());
        let alpha = challenge(ALPHA);
        let zero = self.dag.raw(Op::Sub, alpha.clone(), alpha);
        let reference = self
            .dag
            .raw(Op::Add, stage_two(PRODUCT_COLUMN), constant(0));
        let fields = [
            ("reference", reference),
            ("numerator_air", zero),
            ("denominator_air", constant(1)),
            ("numerator_direct", constant(0)),
            ("denominator_direct", constant(1)),
        ];
        self.hints.push(hint(SUM_HINT, fields));
    }

    /// Run `f` with expression simplification turned off.
    pub fn without_simplification<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let optimize = std::mem::replace(&mut self.dag.optimize, false);
        let result = f(self);
        self.dag.optimize = optimize;
        result
    }

    /// The expression `lhs kind rhs`, simplified in optimized mode.
    pub fn expression(&mut self, kind: Op, lhs: pb::Operand, rhs: pb::Operand) -> pb::Operand {
        self.dag.push(kind, lhs, rhs)
    }

    /// Add the fixed column `Kernel.{name}` and return a reference to it.
    pub fn fixed(&mut self, name: &str, values: Vec<u64>) -> pb::Operand {
        let id = self.air.fixed_cols.len() as u32;
        self.air.fixed_cols.push(pb::FixedCol {
            values: values.into_iter().map(field_bytes).collect(),
        });
        self.symbols.push(symbol(
            &format!("{AIR_NAME}.{name}"),
            SymbolType::FixedCol,
            id,
            Some(0),
            vec![],
        ));
        operand(Operand::FixedCol(pb::operand::FixedCol {
            idx: id,
            row_offset: 0,
        }))
    }

    /// Constrain `expression` to zero on every row. An operand that is not an
    /// expression is first wrapped as `operand + 0`, since a constraint must
    /// reference an expression.
    pub fn constrain(&mut self, mut expression: pb::Operand, label: String) {
        if !matches!(expression.operand, Some(Operand::Expression(_))) {
            expression = self.dag.raw(Op::Add, expression, constant(0));
        }
        let Some(Operand::Expression(expression)) = expression.operand else {
            unreachable!("the operand was just made an expression")
        };
        self.air.constraints.push(pb::Constraint {
            constraint: Some(pb::constraint::Constraint::EveryRow(
                pb::constraint::EveryRow {
                    expression_idx: Some(expression),
                    debug_line: Some(label),
                },
            )),
        });
    }

    /// A source linear combination; wire 0 is the constant 1 and wire `w > 0`
    /// is stage-one column `w - 1`.
    pub fn linear_combination(&mut self, terms: &r1cs::LinearCombination) -> pb::Operand {
        let mut result = constant(0);
        for (wire, k) in terms {
            let wire = if *wire == 0 {
                constant(1)
            } else {
                trace(wire - 1)
            };
            let product = self.expression(Op::Mul, constant(k.0), wire);
            result = self.expression(Op::Add, result, product);
        }
        result
    }

    /// Add the stage-two challenge symbols and the public-value symbol, then
    /// encode the PILout.
    pub fn finish(mut self, publics: u32) -> Vec<u8> {
        // The two stage-two challenges, called α and γ in the permutation argument.
        for index in [ALPHA, GAMMA] {
            self.symbols.push(pb::Symbol {
                name: CHALLENGE_NAMES[index as usize].into(),
                r#type: SymbolType::Challenge as i32,
                id: index,
                stage: Some(STAGE_TWO),
                ..Default::default()
            });
        }
        if publics > 0 {
            self.symbols.push(pb::Symbol {
                name: PUBLICS_SYMBOL.into(),
                r#type: SymbolType::PublicValue as i32,
                dim: 1,
                lengths: vec![publics],
                ..Default::default()
            });
        }
        self.air.expressions = self.dag.nodes;
        let output = pb::PilOut {
            name: Some(PILOUT_NAME.into()),
            base_field: P.to_be_bytes().to_vec(),
            air_groups: vec![pb::AirGroup {
                name: Some(AIR_NAME.into()),
                airs: vec![self.air],
                ..Default::default()
            }],
            num_challenges: vec![0, 2],
            num_public_values: publics,
            symbols: self.symbols,
            hints: self.hints,
            ..Default::default()
        };
        output.encode_to_vec()
    }
}

/// A hint of the single AIR with one array field of named operands.
pub fn hint(name: &str, fields: impl IntoIterator<Item = (&'static str, pb::Operand)>) -> pb::Hint {
    pb::Hint {
        name: name.into(),
        air_group_id: Some(0),
        air_id: Some(0),
        hint_fields: vec![pb::HintField {
            value: Some(pb::hint_field::Value::HintFieldArray(pb::HintFieldArray {
                hint_fields: fields
                    .into_iter()
                    .map(|(name, v)| pb::HintField {
                        name: Some(name.into()),
                        value: Some(pb::hint_field::Value::Operand(v)),
                    })
                    .collect(),
            })),
            ..Default::default()
        }],
    }
}
