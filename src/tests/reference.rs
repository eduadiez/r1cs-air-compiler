//! Reference compiler and checker.
use super::{
    air, assert_rejected, challenge_operand, changed_pilout, constraint_labeled, fixed_operand,
    hint_fields, operand, parser::small_source, r1cs_bytes, reuse_expression,
    with_unreferenced_operand, witness_column, Change,
};
use crate::{
    checker, compiler, contract::PACKED_RELATION, proto as pb, r1cs, sha256_hex,
    ReferenceCertificate, P,
};

fn compile(source: &[u8], optimize: bool) -> (Vec<u8>, ReferenceCertificate) {
    let circuit = r1cs::read(source).unwrap();
    compiler::compile(source, &circuit, 64, optimize).unwrap()
}

/// Apply `change` to the PILout of `small_source()`, update the certificate's
/// hash, and require the reference checker to reject it with `expected`. The
/// unchanged artifact, decoded and encoded again, must be accepted.
fn rejects(expected: &str, change: impl FnOnce(&mut pb::PilOut)) {
    let source = small_source();
    let (bytes, mut cert) = compile(&source, false);
    let unchanged = changed_pilout(&bytes, |_| {});
    assert!(checker::check(&source, &unchanged, &cert).is_ok());
    let bytes = changed_pilout(&bytes, change);
    cert.pilout_sha256 = sha256_hex(&bytes);
    assert_rejected(checker::check(&source, &bytes, &cert), expected);
}

/// Require the reference checker to reject a changed certificate with `expected`.
fn rejects_certificate(expected: &str, change: impl FnOnce(&mut ReferenceCertificate)) {
    let source = small_source();
    let (bytes, mut cert) = compile(&source, false);
    assert!(checker::check(&source, &bytes, &cert).is_ok());
    change(&mut cert);
    assert_rejected(checker::check(&source, &bytes, &cert), expected);
}

#[test]
fn compiled_artifacts_are_accepted() {
    let source = small_source();
    for optimize in [false, true] {
        let (bytes, cert) = compile(&source, optimize);
        assert!(checker::check(&source, &bytes, &cert).unwrap().accepted);
    }
}

#[test]
fn optimized_artifacts_are_smaller_and_deterministic() {
    let source = small_source();
    let (reference, _) = compile(&source, false);
    let (optimized, cert) = compile(&source, true);
    assert_eq!(compile(&source, true), (optimized.clone(), cert));
    assert!(optimized.len() < reference.len());
}

#[test]
fn field_boundary_coefficients_preserve_the_relation_in_both_modes() {
    let base = small_source();
    let n = base.len();
    for a in [0, 1, 2, P - 2, P - 1] {
        for b in [0, 1, 2, P - 2, P - 1] {
            for c in [0, 1, 2, P - 2, P - 1] {
                let mut source = base.clone();
                for (offset, value) in [(n - 40, a), (n - 24, b), (n - 8, c)] {
                    super::set_u64(&mut source, offset, value);
                }
                for optimize in [false, true] {
                    let (bytes, cert) = compile(&source, optimize);
                    assert!(checker::check(&source, &bytes, &cert).is_ok());
                }
            }
        }
    }
}

#[test]
fn circuits_without_public_values_are_accepted() {
    let source = r1cs_bytes([4, 0, 0, 3], &[[2, 3, 1]]);
    for optimize in [false, true] {
        let (bytes, cert) = compile(&source, optimize);
        assert!(checker::check(&source, &bytes, &cert).is_ok());
    }
}

#[test]
fn changed_constraints_are_rejected() {
    rejects("constraint count", |p| {
        air(p).constraints.remove(1);
    });
    rejects("constraint count", |p| {
        let extra = air(p).constraints[1].clone();
        air(p).constraints.push(extra);
    });
    rejects("constraint 1 differs from the expected polynomial", |p| {
        reuse_expression(p, 1, 0);
    });
    rejects("constraint 1 must be labeled 'R1CS 0'", |p| {
        air(p).constraints[1] = air(p).constraints[0].clone();
    });
    rejects("expression refers to itself or a later expression", |p| {
        let root = constraint_labeled(p, "R1CS 0");
        let Some(pb::constraint::Constraint::EveryRow(row)) = &air(p).constraints[root].constraint
        else {
            unreachable!("reference constraints apply to every row")
        };
        let index = row.expression_idx.unwrap().idx;
        air(p).expressions[index as usize].operation =
            Some(pb::expression::Operation::Add(pb::expression::Add {
                lhs: Some(operand(pb::operand::Operand::Expression(
                    pb::operand::Expression { idx: index },
                ))),
                rhs: Some(operand(pb::operand::Operand::Constant(
                    pb::operand::Constant { value: vec![] },
                ))),
            }));
    });
}

#[test]
fn changed_first_column_is_rejected() {
    rejects("FIRST column values", |p| {
        air(p).fixed_cols[0].values[1] = vec![1];
    });
    rejects("FIRST column length", |p| {
        air(p).fixed_cols[0].values.pop();
    });
    rejects("fixed column count", |p| {
        air(p).fixed_cols.push(pb::FixedCol::default());
    });
}

#[test]
fn changed_hints_are_rejected() {
    rejects("hint 'gsum_col': name, AIR or field layout", |p| {
        p.hints[0].name = "renamed".into();
    });
    rejects("hint 'gsum_col': field names", |p| {
        hint_fields(p, "gsum_col").pop();
    });
    rejects("hint 'gsum_col': field names", |p| {
        hint_fields(p, "gsum_col")[0].name = Some("renamed".into());
    });
    // Each part of `z + 0` and `α - α` is compared.
    let set_field = |p: &mut pb::PilOut, name: &str, value: pb::Operand| {
        let fields = hint_fields(p, "gsum_col");
        let field = fields
            .iter_mut()
            .find(|f| f.name.as_deref() == Some(name))
            .unwrap();
        field.value = Some(pb::hint_field::Value::Operand(value));
    };
    let expression = |p: &mut pb::PilOut, op: pb::expression::Operation| {
        let expressions = &mut air(p).expressions;
        expressions.push(pb::Expression {
            operation: Some(op),
        });
        operand(pb::operand::Operand::Expression(pb::operand::Expression {
            idx: expressions.len() as u32 - 1,
        }))
    };
    let zero = || {
        operand(pb::operand::Operand::Constant(pb::operand::Constant {
            value: vec![],
        }))
    };
    let reference = "hint 'gsum_col': field 'reference' differs from the expected expression";
    let numerator = "hint 'gsum_col': field 'numerator_air' differs from the expected expression";
    // `a[0] + 0`: stage one instead of stage two.
    rejects(reference, |p| {
        let sum = pb::expression::Operation::Add(pb::expression::Add {
            lhs: Some(operand(witness_column(1, 0, 0))),
            rhs: Some(zero()),
        });
        let value = expression(p, sum);
        set_field(p, "reference", value);
    });
    for (op, lhs, rhs) in [("add", 0, 0), ("sub", 1, 0), ("sub", 0, 1), ("sub", 1, 1)] {
        rejects(numerator, |p| {
            let (lhs, rhs) = (
                Some(operand(challenge_operand(2, lhs))),
                Some(operand(challenge_operand(2, rhs))),
            );
            let operation = if op == "add" {
                pb::expression::Operation::Add(pb::expression::Add { lhs, rhs })
            } else {
                pb::expression::Operation::Sub(pb::expression::Sub { lhs, rhs })
            };
            let value = expression(p, operation);
            set_field(p, "numerator_air", value);
        });
    }
    // A zero numerator that is not written as α - α.
    rejects(
        "hint 'gsum_col': field 'numerator_air' differs from the expected expression",
        |p| {
            let fields = hint_fields(p, "gsum_col");
            let numerator = fields
                .iter_mut()
                .find(|f| f.name.as_deref() == Some("numerator_air"))
                .unwrap();
            numerator.value = Some(pb::hint_field::Value::Operand(operand(
                pb::operand::Operand::Constant(pb::operand::Constant { value: vec![] }),
            )));
        },
    );
}

#[test]
fn every_metadata_field_is_checked() {
    let changes: Vec<(&str, Change)> = vec![
        (
            "PILout must be named",
            Box::new(|p| p.name = Some("other".into())),
        ),
        (
            "field must be Goldilocks",
            Box::new(|p| p.base_field = vec![17]),
        ),
        (
            "two stage-two challenges",
            Box::new(|p| p.num_challenges = vec![0, 3]),
        ),
        ("public value count", Box::new(|p| p.num_public_values += 1)),
        (
            "unsupported proof values",
            Box::new(|p| p.num_proof_values = vec![1]),
        ),
        (
            "unsupported proof values",
            Box::new(|p| p.public_tables.push(Default::default())),
        ),
        (
            "unsupported proof values",
            Box::new(|p| p.expressions.push(Default::default())),
        ),
        (
            "unsupported proof values",
            Box::new(|p| p.constraints.push(Default::default())),
        ),
        (
            "AIR group must be named",
            Box::new(|p| p.air_groups[0].name = Some("Other".into())),
        ),
        (
            "unsupported AIR group values",
            Box::new(|p| p.air_groups[0].air_group_values.push(Default::default())),
        ),
        (
            "exactly one AIR required",
            Box::new(|p| p.air_groups[0].airs.push(Default::default())),
        ),
        (
            "AIR must be named",
            Box::new(|p| air(p).name = Some("Other".into())),
        ),
        ("AIR row count", Box::new(|p| air(p).num_rows = Some(128))),
        ("AIR stage widths", Box::new(|p| air(p).stage_widths[1] = 2)),
        (
            "AIR must be aggregable",
            Box::new(|p| air(p).aggregable = false),
        ),
        (
            "unsupported periodic columns",
            Box::new(|p| air(p).periodic_cols.push(Default::default())),
        ),
        (
            "unsupported periodic columns",
            Box::new(|p| air(p).air_values.push(Default::default())),
        ),
        (
            "unsupported periodic columns",
            Box::new(|p| air(p).custom_commits.push(Default::default())),
        ),
        (
            "symbol 'a' has unexpected fields",
            Box::new(|p| p.symbols[0].r#type = 1),
        ),
        (
            "symbol 'a' has unexpected fields",
            Box::new(|p| p.symbols[0].id = 7),
        ),
        (
            "symbol 'a' has unexpected fields",
            Box::new(|p| p.symbols[0].stage = Some(3)),
        ),
        (
            "symbol 'a' has unexpected fields",
            Box::new(|p| p.symbols[0].air_group_id = Some(1)),
        ),
        (
            "symbol 'a' has unexpected fields",
            Box::new(|p| p.symbols[0].air_id = Some(1)),
        ),
        (
            "symbol 'a' has unexpected fields",
            Box::new(|p| p.symbols[0].lengths.push(1)),
        ),
        (
            "symbol 'a' has unexpected fields",
            Box::new(|p| p.symbols[0].dim += 1),
        ),
        (
            "symbol 'a' has unexpected fields",
            Box::new(|p| p.symbols[0].commit_id = Some(0)),
        ),
        (
            "symbol 'a' has unexpected fields",
            Box::new(|p| p.symbols[0].debug_line = Some("x".into())),
        ),
        (
            "symbol count",
            Box::new(|p| {
                p.symbols.pop();
            }),
        ),
        (
            "name, AIR or field layout",
            Box::new(|p| p.hints[0].air_group_id = Some(1)),
        ),
        (
            "name, AIR or field layout",
            Box::new(|p| p.hints[0].air_id = Some(1)),
        ),
        (
            "name, AIR or field layout",
            Box::new(|p| p.hints[0].hint_fields[0].name = Some("x".into())),
        ),
        (
            "name, AIR or field layout",
            Box::new(|p| {
                let field = p.hints[0].hint_fields[0].clone();
                p.hints[0].hint_fields.push(field);
            }),
        ),
    ];
    for (expected, change) in changes {
        rejects(expected, change);
    }
}

#[test]
fn malformed_constraints_expressions_and_hints_are_rejected() {
    use pb::{constraint::Constraint, expression::Operation, hint_field::Value};
    rejects("constraint 1 must apply to every row", |p| {
        let Some(Constraint::EveryRow(row)) = air(p).constraints[1].constraint.clone() else {
            unreachable!("reference constraints apply to every row")
        };
        air(p).constraints[1].constraint = Some(Constraint::FirstRow(pb::constraint::FirstRow {
            expression_idx: row.expression_idx,
            debug_line: row.debug_line,
        }));
    });
    rejects("constraint 1 has no expression", |p| {
        if let Some(Constraint::EveryRow(row)) = &mut air(p).constraints[1].constraint {
            row.expression_idx = None;
        }
    });
    rejects("unexpected or duplicate symbol 'renamed'", |p| {
        p.symbols[0].name = "renamed".into();
    });
    rejects("negation is not supported", |p| {
        air(p).expressions.push(pb::Expression {
            operation: Some(Operation::Neg(pb::expression::Neg {
                value: Some(operand(witness_column(1, 0, 0))),
            })),
        });
    });
    rejects("empty expression", |p| {
        air(p).expressions.push(pb::Expression { operation: None });
    });
    rejects("expression without rhs", |p| {
        air(p).expressions.push(pb::Expression {
            operation: Some(Operation::Add(pb::expression::Add {
                lhs: Some(operand(witness_column(1, 0, 0))),
                rhs: None,
            })),
        });
    });
    rejects("missing operand", |p| {
        air(p).expressions.push(pb::Expression {
            operation: Some(Operation::Add(pb::expression::Add {
                lhs: Some(pb::Operand { operand: None }),
                rhs: Some(operand(witness_column(1, 0, 0))),
            })),
        });
    });
    rejects("exactly one AIR group required", |p| {
        let group = p.air_groups[0].clone();
        p.air_groups.push(group);
    });
    rejects("hint 'gsum_col': fields must be an array", |p| {
        p.hints[0].hint_fields[0].value = Some(Value::Operand(operand(witness_column(1, 0, 0))));
    });
    rejects("hint 'gsum_col': unnamed field", |p| {
        hint_fields(p, "gsum_col")[0].name = None;
    });
    rejects(
        "hint 'gsum_col': field 'reference' is not an operand",
        |p| {
            let fields = hint_fields(p, "gsum_col");
            let reference = fields
                .iter_mut()
                .find(|f| f.name.as_deref() == Some("reference"))
                .unwrap();
            reference.value = Some(Value::StringValue("x".into()));
        },
    );
    rejects("hint 'gsum_col': duplicate field", |p| {
        let fields = hint_fields(p, "gsum_col");
        let name = fields[0].name.clone();
        fields[1].name = name;
    });
}

#[test]
fn unreferenced_expressions_with_valid_operands_are_accepted() {
    let source = small_source();
    let (bytes, mut cert) = compile(&source, false);
    let bytes = changed_pilout(&bytes, with_unreferenced_operand(witness_column(1, 2, 0)));
    cert.pilout_sha256 = sha256_hex(&bytes);
    assert!(checker::check(&source, &bytes, &cert).is_ok());
}

#[test]
fn out_of_range_operands_are_rejected() {
    use pb::operand::{self as o, Operand};
    let constant = |value: Vec<u8>| Operand::Constant(o::Constant { value });
    // The `small_source()` AIR has width 3, three public values and one fixed column.
    let cases = [
        (witness_column(1, 3, 0), "witness column out of range"),
        (witness_column(2, 1, 0), "witness column out of range"),
        (witness_column(3, 0, 0), "witness column out of range"),
        (witness_column(1, 0, 1), "row offsets are not allowed here"),
        (
            Operand::PublicValue(o::PublicValue { idx: 3 }),
            "public value out of range",
        ),
        (fixed_operand(1, 0), "fixed column out of range"),
        (fixed_operand(0, 1), "with a row offset"),
        (challenge_operand(2, 2), "challenge out of range"),
        (challenge_operand(1, 0), "challenge out of range"),
        (
            Operand::Expression(o::Expression { idx: 1_000 }),
            "expression refers to itself or a later expression",
        ),
        (constant(vec![0, 5]), "no leading zeros"),
        (constant(P.to_be_bytes().to_vec()), "not below P"),
        (constant(vec![1; 9]), "longer than 8 bytes"),
        (
            Operand::PeriodicCol(o::PeriodicCol {
                idx: 0,
                row_offset: 0,
            }),
            "unsupported operand type",
        ),
    ];
    for (bad, expected) in cases {
        rejects(expected, with_unreferenced_operand(bad));
    }
}

#[test]
fn unknown_protobuf_fields_are_rejected() {
    let source = small_source();
    let (mut bytes, mut cert) = compile(&source, false);
    bytes.extend([0xa0, 0x06, 0x01]);
    cert.pilout_sha256 = sha256_hex(&bytes);
    assert_rejected(
        checker::check(&source, &bytes, &cert),
        "not in canonical protobuf encoding",
    );
}

#[test]
fn every_certificate_field_is_checked() {
    rejects_certificate("unsupported certificate version 2", |c| c.version = 2);
    rejects_certificate("expected 'r1cs-per-row-publics-at-row-zero'", |c| {
        c.relation = PACKED_RELATION.into()
    });
    rejects_certificate("certificate sourceSha256 does not match the source", |c| {
        c.source_sha256 = "00".repeat(32)
    });
    rejects_certificate("certificate piloutSha256 does not match the PILout", |c| {
        c.pilout_sha256 = "00".repeat(32)
    });
    rejects_certificate("public wires must be 1..=publics in order", |c| {
        c.public_wires.swap(0, 1)
    });
    rejects_certificate("each nonconstant wire needs its own column", |c| {
        c.wire_columns[2] = c.wire_columns[1]
    });
    rejects_certificate("wire map dimensions", |c| c.wire_columns[0] = Some(0));
    rejects_certificate("wire map dimensions", |c| c.width += 1);
}

#[test]
fn unsupported_row_counts_are_rejected_even_when_consistent() {
    // The PILout geometry and FIRST column agree with the certificate.
    for rows in [32u32, 96] {
        let source = small_source();
        let (bytes, mut cert) = compile(&source, false);
        let bytes = changed_pilout(&bytes, |p| {
            air(p).num_rows = Some(rows);
            air(p).fixed_cols[0].values.resize(rows as usize, vec![]);
        });
        cert.rows = rows;
        cert.pilout_sha256 = sha256_hex(&bytes);
        assert_rejected(
            checker::check(&source, &bytes, &cert),
            "rows must be a power of two",
        );
    }
}

#[test]
fn circuits_without_nonconstant_wires_are_rejected() {
    let source = r1cs_bytes([1, 0, 0, 0], &[]);
    let circuit = r1cs::read(&source).unwrap();
    assert_rejected(
        compiler::compile(&source, &circuit, 64, false),
        "circuit has no nonconstant wires",
    );
    let cert = ReferenceCertificate {
        version: 1,
        relation: crate::REFERENCE_RELATION.into(),
        source_sha256: sha256_hex(&source),
        pilout_sha256: sha256_hex(&[]),
        rows: 64,
        width: 0,
        public_wires: vec![],
        wire_columns: vec![None],
    };
    assert_rejected(
        checker::check(&source, &[], &cert),
        "circuit has no nonconstant wires",
    );
}
