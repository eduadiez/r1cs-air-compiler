//! Module compilation and the composition check. Hosts are built here the way
//! an integration would: the module fragment's expressions and constraints are
//! copied into a larger AIR, with the module's columns moved to other indices.
use super::{assert_rejected, r1cs_with_terms};
use crate::{
    api, contract::STAGE_ONE, proto as pb, CompositionBinding, ModuleArtifact, WireColumn, P,
};
use pb::{constraint::Constraint, expression::Operation, operand::Operand};
use prost::Message;

/// Wires 1 (public output) and 2 (public input), private wires 3 and 4, and an
/// unused private wire 5. Equations: w1 · w2 = w3, w1 · w3 = 2·w4 and the
/// linear 0 = w4 + 3 - w3.
fn source() -> Vec<u8> {
    r1cs_with_terms(
        [6, 1, 1, 3],
        &[
            [vec![(1, 1)], vec![(2, 1)], vec![(3, 1)]],
            [vec![(1, 1)], vec![(3, 1)], vec![(4, 2)]],
            [vec![], vec![], vec![(0, P - 3), (3, 1), (4, P - 1)]],
        ],
    )
}

fn module(optimize: bool) -> ModuleArtifact {
    api::compile_module(&source(), &[1, 2, 4], optimize).unwrap()
}

/// A host AIR of `rows` rows holding `artifact`'s fragment with every module
/// column `c` moved to `offset + c`, plus `extra` stage-one columns after it,
/// a stage-two column and a challenge, and the constraint
/// `host column 0 = w1` (column 0 stands for an adapter's copy of the export).
pub fn host(artifact: &ModuleArtifact, offset: u32, extra: u32) -> pb::PilOut {
    let mut pilout = pb::PilOut::decode(&artifact.pilout[..]).unwrap();
    let air = &mut pilout.air_groups[0].airs[0];
    let width = air.stage_widths[0];
    for expression in &mut air.expressions {
        for operand in operands(expression) {
            if let Some(Operand::WitnessCol(column)) = &mut operand.operand {
                column.col_idx += offset;
            }
        }
    }
    air.num_rows = Some(64);
    air.stage_widths = vec![offset + width + extra, 1];
    let first = air.expressions.len() as u32;
    let witness = |stage, col_idx| pb::Operand {
        operand: Some(Operand::WitnessCol(pb::operand::WitnessCol {
            stage,
            col_idx,
            row_offset: 0,
        })),
    };
    let w1 = artifact.certificate.wire_columns()[1].unwrap() + offset;
    let challenge = pb::Operand {
        operand: Some(Operand::Challenge(pb::operand::Challenge {
            stage: 2,
            idx: 0,
        })),
    };
    air.expressions.push(binary(
        Op::Sub,
        witness(STAGE_ONE, 0),
        witness(STAGE_ONE, w1),
    ));
    // A stand-in for a bus constraint over stage two.
    air.expressions
        .push(binary(Op::Mul, witness(2, 0), challenge));
    for root in [first, first + 1] {
        air.constraints.push(every_row(root));
    }
    pilout.num_challenges = vec![0, 1];
    pilout
}

#[derive(Clone, Copy)]
enum Op {
    Sub,
    Mul,
}

fn binary(op: Op, lhs: pb::Operand, rhs: pb::Operand) -> pb::Expression {
    let operands = pb::expression::Mul {
        lhs: Some(lhs),
        rhs: Some(rhs),
    };
    let operation = match op {
        Op::Sub => Operation::Sub(pb::expression::Sub {
            lhs: operands.lhs,
            rhs: operands.rhs,
        }),
        Op::Mul => Operation::Mul(operands),
    };
    pb::Expression {
        operation: Some(operation),
    }
}

fn every_row(root: u32) -> pb::Constraint {
    pb::Constraint {
        constraint: Some(Constraint::EveryRow(pb::constraint::EveryRow {
            expression_idx: Some(pb::operand::Expression { idx: root }),
            debug_line: None,
        })),
    }
}

fn operands(expression: &mut pb::Expression) -> Vec<&mut pb::Operand> {
    match expression.operation.as_mut().unwrap() {
        Operation::Add(x) => vec![x.lhs.as_mut().unwrap(), x.rhs.as_mut().unwrap()],
        Operation::Sub(x) => vec![x.lhs.as_mut().unwrap(), x.rhs.as_mut().unwrap()],
        Operation::Mul(x) => vec![x.lhs.as_mut().unwrap(), x.rhs.as_mut().unwrap()],
        Operation::Neg(x) => vec![x.value.as_mut().unwrap()],
    }
}

fn check(
    artifact: &ModuleArtifact,
    host: &pb::PilOut,
    binding: &CompositionBinding,
) -> crate::Result<crate::CompositionReport> {
    api::check_composition(
        &source(),
        &host.encode_to_vec(),
        &artifact.certificate,
        binding,
    )
}

#[test]
fn module_fragments_hold_every_source_equation_and_no_publics() {
    for optimize in [false, true] {
        let artifact = module(optimize);
        let report = api::check_module(&source(), &artifact.pilout, &artifact.certificate).unwrap();
        assert_eq!(
            (report.source_equations, report.width, report.exports),
            (3, 5, 3)
        );
        let pilout = pb::PilOut::decode(&artifact.pilout[..]).unwrap();
        let air = &pilout.air_groups[0].airs[0];
        // The circuit's two public values are not bound to anything.
        assert_eq!(pilout.num_public_values, 0);
        assert!(air.fixed_cols.is_empty() && pilout.hints.is_empty());
        assert_eq!((air.num_rows, &air.stage_widths[..]), (None, &[5][..]));
        assert_eq!(air.constraints.len(), 3);
        // The unused wire 5 keeps its column.
        assert_eq!(artifact.certificate.wire_columns()[5], Some(4));
    }
}

#[test]
fn changed_module_fragments_are_rejected() {
    let artifact = module(false);
    let rejects = |expected: &str, change: &dyn Fn(&mut pb::PilOut)| {
        let mut pilout = pb::PilOut::decode(&artifact.pilout[..]).unwrap();
        change(&mut pilout);
        let bytes = pilout.encode_to_vec();
        let mut certificate = artifact.certificate.clone();
        certificate.pilout_sha256 = crate::sha256_hex(&bytes);
        assert_rejected(api::check_module(&source(), &bytes, &certificate), expected);
    };
    rejects("one constraint per source equation", &|p| {
        p.air_groups[0].airs[0].constraints.pop();
    });
    rejects("constraint 1 differs from source equation 1", &|p| {
        p.air_groups[0].airs[0].constraints.swap(1, 2);
    });
    rejects("a module has no row count", &|p| {
        p.air_groups[0].airs[0].num_rows = Some(64);
    });
    rejects("a module has no challenges, public values", &|p| {
        p.num_public_values = 2
    });
    rejects("a module has only stage one", &|p| {
        p.air_groups[0].airs[0].stage_widths.push(1);
    });
    rejects("a module has no fixed", &|p| {
        p.air_groups[0].airs[0]
            .fixed_cols
            .push(pb::FixedCol::default());
    });
    rejects("without a label", &|p| {
        if let Some(Constraint::EveryRow(row)) =
            &mut p.air_groups[0].airs[0].constraints[0].constraint
        {
            row.debug_line = Some("x".into());
        }
    });
}

#[test]
fn invalid_exports_are_rejected() {
    assert_rejected(
        api::compile_module(&source(), &[0], false),
        "export 0: not a nonconstant wire of the circuit",
    );
    assert_rejected(
        api::compile_module(&source(), &[6], false),
        "export 6: not a nonconstant wire of the circuit",
    );
    assert_rejected(
        api::compile_module(&source(), &[2, 2], false),
        "export 2: listed twice",
    );
    let artifact = module(false);
    let mut certificate = artifact.certificate.clone();
    certificate.exports.push(1);
    assert_rejected(
        api::check_module(&source(), &artifact.pilout, &certificate),
        "export 1: listed twice",
    );
}

#[test]
fn modules_embedded_at_an_offset_are_preserved() {
    for optimize in [false, true] {
        let artifact = module(optimize);
        let host = host(&artifact, 3, 2);
        let binding = CompositionBinding::with_offset(&artifact.certificate, 0, 0, 3);
        let report = check(&artifact, &host, &binding).unwrap();
        assert!(report.preserved);
        assert_eq!(report.rows, 64);
        assert_eq!((report.source_equations, report.trivial_equations), (3, 0));
        let exports: Vec<_> = report.exports.iter().map(|e| (e.wire, e.column)).collect();
        assert_eq!(exports, [(1, 3), (2, 4), (4, 6)]);
        assert_eq!(report.claim, crate::COMPOSITION_CLAIM);
    }
}

#[test]
fn reordered_and_shared_host_expressions_are_accepted() {
    // Reverse the expression order (a host may import them in any order) and
    // make each constraint reference its root through one more `x + 0`.
    let artifact = module(true);
    let mut host = host(&artifact, 1, 0);
    let air = &mut host.air_groups[0].airs[0];
    let count = air.expressions.len() as u32;
    let moved = |idx: u32| count - 1 - idx;
    air.expressions.reverse();
    for expression in &mut air.expressions {
        for operand in operands(expression) {
            if let Some(Operand::Expression(e)) = &mut operand.operand {
                e.idx = moved(e.idx);
            }
        }
    }
    let zero = pb::Operand {
        operand: Some(Operand::Constant(pb::operand::Constant {
            value: vec![0, 0],
        })),
    };
    for constraint in &mut air.constraints {
        if let Some(Constraint::EveryRow(row)) = &mut constraint.constraint {
            let root = row.expression_idx.as_mut().unwrap();
            let wrapper = pb::Expression {
                operation: Some(Operation::Add(pb::expression::Add {
                    lhs: Some(pb::Operand {
                        operand: Some(Operand::Expression(pb::operand::Expression {
                            idx: moved(root.idx),
                        })),
                    }),
                    rhs: Some(zero.clone()),
                })),
            };
            root.idx = air.expressions.len() as u32;
            air.expressions.push(wrapper);
        }
    }
    let binding = CompositionBinding::with_offset(&artifact.certificate, 0, 0, 1);
    assert!(check(&artifact, &host, &binding).unwrap().preserved);
}

#[test]
fn changed_hosts_and_bindings_are_rejected() {
    let artifact = module(false);
    let good = host(&artifact, 2, 0);
    let binding = CompositionBinding::with_offset(&artifact.certificate, 0, 0, 2);
    assert!(check(&artifact, &good, &binding).is_ok());
    let missing = "is not enforced by any every-row constraint of the selected AIR";
    // A missing equation.
    let mut host = good.clone();
    host.air_groups[0].airs[0].constraints.remove(1);
    assert_rejected(
        check(&artifact, &host, &binding),
        &format!("source equation 1 {missing}"),
    );
    // A changed coefficient: 2·w4 becomes 3·w4.
    let mut host = good.clone();
    let two = pb::operand::Constant { value: vec![2] };
    let constant = host.air_groups[0].airs[0]
        .expressions
        .iter_mut()
        .flat_map(operands)
        .find(|o| o.operand == Some(Operand::Constant(two.clone())))
        .unwrap();
    constant.operand = Some(Operand::Constant(pb::operand::Constant { value: vec![3] }));
    assert_rejected(
        check(&artifact, &host, &binding),
        &format!("source equation 1 {missing}"),
    );
    // An equation that holds only on the first row.
    let mut host = good.clone();
    let constraint = &mut host.air_groups[0].airs[0].constraints[0];
    if let Some(Constraint::EveryRow(row)) = constraint.constraint.take() {
        constraint.constraint = Some(Constraint::FirstRow(pb::constraint::FirstRow {
            expression_idx: row.expression_idx,
            debug_line: None,
        }));
    }
    assert_rejected(
        check(&artifact, &host, &binding),
        &format!("source equation 0 {missing}"),
    );
    // A module column read at the next row.
    let mut host = good.clone();
    for expression in &mut host.air_groups[0].airs[0].expressions {
        for operand in operands(expression) {
            if let Some(Operand::WitnessCol(column)) = &mut operand.operand {
                if column.col_idx == 2 {
                    column.row_offset = 1;
                }
            }
        }
    }
    assert_rejected(
        check(&artifact, &host, &binding),
        &format!("source equation 0 {missing}"),
    );
    // A wrong export mapping: wires 1 and 2 swapped.
    let mut swapped = binding.clone();
    swapped.columns[0].column = 3;
    swapped.columns[1].column = 2;
    assert_rejected(check(&artifact, &good, &swapped), "source equation 1");
    // An accidental alias: wires 1 and 2 in one column.
    let mut aliased = binding.clone();
    aliased.columns[1].column = 2;
    assert_rejected(
        check(&artifact, &good, &aliased),
        "binding: wires 1 and 2 share column 2",
    );
    // Missing and repeated wires, a column out of range and an unknown AIR.
    let mut short = binding.clone();
    short.columns.pop();
    assert_rejected(
        check(&artifact, &good, &short),
        "binding: wire 5 has no column",
    );
    let mut repeated = binding.clone();
    repeated.columns[1].wire = 1;
    assert_rejected(
        check(&artifact, &good, &repeated),
        "binding: wire 1 is listed twice",
    );
    let mut outside = binding.clone();
    outside.columns[4] = WireColumn { wire: 5, column: 7 };
    assert_rejected(
        check(&artifact, &good, &outside),
        "binding: column 7 of wire 5 is not a stage-one column of the AIR (7)",
    );
    let mut elsewhere = binding.clone();
    elsewhere.air = 1;
    assert_rejected(
        check(&artifact, &good, &elsewhere),
        "the binding selects no AIR",
    );
    // Another source.
    let other = r1cs_with_terms([6, 1, 1, 3], &[[vec![(1, 1)], vec![(2, 1)], vec![(3, 1)]]]);
    assert_rejected(
        api::check_composition(
            &other,
            &good.encode_to_vec(),
            &artifact.certificate,
            &binding,
        ),
        "certificate sourceSha256 does not match the source",
    );
}

/// The composition check reads the host AIR, not the module fragment: an
/// equation cancelled by another host operand is not enforced.
#[test]
fn host_operands_never_cancel_a_module_equation() {
    let artifact = module(false);
    let mut host = host(&artifact, 0, 0);
    let air = &mut host.air_groups[0].airs[0];
    let Some(Constraint::EveryRow(row)) = &air.constraints[1].constraint else {
        unreachable!()
    };
    let root = row.expression_idx.unwrap().idx;
    // equation 1 + (fixed column 0) - (fixed column 0 at the next row)
    let fixed = |row_offset| pb::Operand {
        operand: Some(Operand::FixedCol(pb::operand::FixedCol {
            idx: 0,
            row_offset,
        })),
    };
    air.fixed_cols.push(pb::FixedCol::default());
    air.expressions.push(binary(Op::Sub, fixed(0), fixed(1)));
    let difference = air.expressions.len() as u32 - 1;
    air.expressions.push(pb::Expression {
        operation: Some(Operation::Add(pb::expression::Add {
            lhs: Some(pb::Operand {
                operand: Some(Operand::Expression(pb::operand::Expression { idx: root })),
            }),
            rhs: Some(pb::Operand {
                operand: Some(Operand::Expression(pb::operand::Expression {
                    idx: difference,
                })),
            }),
        })),
    });
    let changed = air.expressions.len() as u32 - 1;
    air.constraints[1] = every_row(changed);
    let binding = CompositionBinding::with_offset(&artifact.certificate, 0, 0, 0);
    assert_rejected(
        check(&artifact, &host, &binding),
        "source equation 1 is not enforced",
    );
}

/// An equation whose `A · B - C` is the zero polynomial holds on every row
/// without a constraint, so a host may leave it out.
#[test]
fn trivial_equations_need_no_host_constraint() {
    let source = r1cs_with_terms(
        [3, 0, 0, 2],
        &[
            [vec![(1, 1)], vec![(1, 1)], vec![(2, 1)]],
            [vec![], vec![], vec![]],
        ],
    );
    let artifact = api::compile_module(&source, &[2], false).unwrap();
    let mut host = pb::PilOut::decode(&artifact.pilout[..]).unwrap();
    host.air_groups[0].airs[0].num_rows = Some(64);
    host.air_groups[0].airs[0].constraints.pop();
    let binding = CompositionBinding::with_offset(&artifact.certificate, 0, 0, 0);
    let check = |host: &pb::PilOut| {
        api::check_composition(
            &source,
            &host.encode_to_vec(),
            &artifact.certificate,
            &binding,
        )
    };
    let report = check(&host).unwrap();
    assert_eq!((report.source_equations, report.trivial_equations), (2, 1));
    host.air_groups[0].airs[0].constraints.clear();
    assert_rejected(check(&host), "source equation 0 is not enforced");
}

#[test]
fn changed_module_certificates_are_rejected() {
    let artifact = module(false);
    let rejects = |expected: &str, change: &dyn Fn(&mut crate::ModuleCertificate)| {
        let mut certificate = artifact.certificate.clone();
        change(&mut certificate);
        assert_rejected(
            api::check_module(&source(), &artifact.pilout, &certificate),
            expected,
        );
    };
    rejects("wire map dimensions", &|c| c.width += 1);
    rejects("wire map dimensions", &|c| c.wire_columns.push(Some(5)));
    rejects("wire map dimensions", &|c| c.wire_columns[0] = Some(0));
    rejects("each nonconstant wire needs its own column", &|c| {
        c.wire_columns[2] = Some(0);
    });
    let rejects_fragment = |expected: &str, change: &dyn Fn(&mut pb::PilOut)| {
        let mut pilout = pb::PilOut::decode(&artifact.pilout[..]).unwrap();
        change(&mut pilout);
        let bytes = pilout.encode_to_vec();
        let mut certificate = artifact.certificate.clone();
        certificate.pilout_sha256 = crate::sha256_hex(&bytes);
        assert_rejected(api::check_module(&source(), &bytes, &certificate), expected);
    };
    rejects_fragment("AIR group must be named 'Kernel'", &|p| {
        p.air_groups[0].name = Some("Other".into());
    });
    rejects_fragment("AIR group must be named 'Kernel'", &|p| {
        p.air_groups[0]
            .air_group_values
            .push(pb::AirGroupValue::default());
    });
    // A source with only the constant wire has nothing to embed.
    let constant_only = r1cs_with_terms([1, 0, 0, 0], &[[vec![], vec![], vec![(0, 1)]]]);
    assert_rejected(
        api::compile_module(&constant_only, &[], false),
        "circuit has no nonconstant wires",
    );
    let mut certificate = artifact.certificate.clone();
    certificate.source_sha256 = crate::sha256_hex(&constant_only);
    certificate.width = 0;
    certificate.wire_columns = vec![None];
    certificate.exports.clear();
    let binding = CompositionBinding::with_offset(&certificate, 0, 0, 0);
    let host = host(&artifact, 0, 0).encode_to_vec();
    assert_rejected(
        api::check_composition(&constant_only, &host, &certificate, &binding),
        "circuit has no nonconstant wires",
    );
}

#[test]
fn malformed_and_negated_host_expressions() {
    let artifact = module(false);
    let binding = CompositionBinding::with_offset(&artifact.certificate, 0, 0, 0);
    // x - y written as x + (-y) is the same polynomial.
    let mut negated = host(&artifact, 0, 0);
    let air = &mut negated.air_groups[0].airs[0];
    let mut changed = 0;
    for index in 0..air.expressions.len() {
        if let Some(Operation::Sub(sub)) = air.expressions[index].operation.clone() {
            let negation = air.expressions.len() as u32;
            air.expressions.push(pb::Expression {
                operation: Some(Operation::Neg(pb::expression::Neg { value: sub.rhs })),
            });
            air.expressions[index].operation = Some(Operation::Add(pb::expression::Add {
                lhs: sub.lhs,
                rhs: Some(pb::Operand {
                    operand: Some(Operand::Expression(pb::operand::Expression {
                        idx: negation,
                    })),
                }),
            }));
            changed += 1;
        }
    }
    assert!(changed > 0);
    assert!(check(&artifact, &negated, &binding).unwrap().preserved);
    // A constraint or operand referring to the expression after the last one.
    let mut host_after = host(&artifact, 0, 0);
    let count = host_after.air_groups[0].airs[0].expressions.len() as u32;
    host_after.air_groups[0].airs[0]
        .constraints
        .push(every_row(count));
    assert_rejected(
        check(&artifact, &host_after, &binding),
        "expression index out of range",
    );
    let mut operand_after = host(&artifact, 0, 0);
    let air = &mut operand_after.air_groups[0].airs[0];
    let count = air.expressions.len() as u32;
    air.expressions.push(binary(
        Op::Mul,
        pb::Operand {
            operand: Some(Operand::Expression(pb::operand::Expression {
                idx: count + 1,
            })),
        },
        pb::Operand {
            operand: Some(Operand::Constant(pb::operand::Constant { value: vec![1] })),
        },
    ));
    air.constraints.push(every_row(count));
    assert_rejected(
        check(&artifact, &operand_after, &binding),
        "expression index out of range",
    );
    // A constant equal to P.
    let mut too_large = host(&artifact, 0, 0);
    let two = pb::operand::Constant { value: vec![2] };
    let constant = too_large.air_groups[0].airs[0]
        .expressions
        .iter_mut()
        .flat_map(operands)
        .find(|o| o.operand == Some(Operand::Constant(two.clone())))
        .unwrap();
    constant.operand = Some(Operand::Constant(pb::operand::Constant {
        value: P.to_be_bytes().to_vec(),
    }));
    assert_rejected(
        check(&artifact, &too_large, &binding),
        "field element not below P",
    );
}

/// Operands a fragment does not declare are rejected even when they cancel
/// out of a constraint polynomial, and even when no constraint uses them.
#[test]
fn undeclared_operands_in_module_fragments_are_rejected() {
    let artifact = module(false);
    let witness = |stage, col_idx, row_offset| pb::Operand {
        operand: Some(Operand::WitnessCol(pb::operand::WitnessCol {
            stage,
            col_idx,
            row_offset,
        })),
    };
    let alpha = pb::Operand {
        operand: Some(Operand::Challenge(pb::operand::Challenge {
            stage: 2,
            idx: 0,
        })),
    };
    let fixed = pb::Operand {
        operand: Some(Operand::FixedCol(pb::operand::FixedCol {
            idx: 0,
            row_offset: 0,
        })),
    };
    let public = pb::Operand {
        operand: Some(Operand::PublicValue(pb::operand::PublicValue { idx: 0 })),
    };
    let message = "a module may only use constants, expressions and its stage-one columns";
    let rejects = |message: &str, operand: pb::Operand, cancel: bool| {
        let mut pilout = pb::PilOut::decode(&artifact.pilout[..]).unwrap();
        let air = &mut pilout.air_groups[0].airs[0];
        // operand - operand, then (equation 0) + (operand - operand) when cancelled.
        let difference = air.expressions.len() as u32;
        air.expressions
            .push(binary(Op::Sub, operand.clone(), operand));
        if cancel {
            let Some(Constraint::EveryRow(row)) = &air.constraints[0].constraint else {
                unreachable!()
            };
            let root = row.expression_idx.unwrap().idx;
            let expression = |idx| pb::Operand {
                operand: Some(Operand::Expression(pb::operand::Expression { idx })),
            };
            air.expressions.push(pb::Expression {
                operation: Some(Operation::Add(pb::expression::Add {
                    lhs: Some(expression(root)),
                    rhs: Some(expression(difference)),
                })),
            });
            air.constraints[0] = every_row(difference + 1);
        }
        let bytes = pilout.encode_to_vec();
        let mut certificate = artifact.certificate.clone();
        certificate.pilout_sha256 = crate::sha256_hex(&bytes);
        assert_rejected(api::check_module(&source(), &bytes, &certificate), message);
    };
    for cancel in [true, false] {
        rejects(message, witness(2, 0, 0), cancel);
        rejects(message, alpha.clone(), cancel);
        rejects(message, fixed.clone(), cancel);
        rejects(message, public.clone(), cancel);
        rejects(message, witness(STAGE_ONE, 5, 0), cancel);
        rejects(message, witness(STAGE_ONE, 0, 1), cancel);
    }
    // Negation is never generated.
    let mut pilout = pb::PilOut::decode(&artifact.pilout[..]).unwrap();
    pilout.air_groups[0].airs[0]
        .expressions
        .push(pb::Expression {
            operation: Some(Operation::Neg(pb::expression::Neg {
                value: Some(witness(STAGE_ONE, 0, 0)),
            })),
        });
    let bytes = pilout.encode_to_vec();
    let mut certificate = artifact.certificate.clone();
    certificate.pilout_sha256 = crate::sha256_hex(&bytes);
    assert_rejected(
        api::check_module(&source(), &bytes, &certificate),
        "only addition, subtraction and multiplication are allowed",
    );
}
