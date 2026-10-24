//! Packed compiler, packed checker and witness projection.
use super::{
    air, assert_rejected, challenge_operand, changed_pilout, constraint_labeled, fixed_column,
    fixed_operand, hint_fields, r1cs_bytes, reuse_expression, with_unreferenced_operand,
    witness_column, wtns_bytes, Change,
};
use crate::{
    contract::{label, REFERENCE_RELATION},
    layout::{LayoutConfig, Strategy},
    network::{ConversionOptions, Reuse},
    packed, packed_check, proto as pb, r1cs, sha256_hex, witness, PackedArtifact, P,
};
use prost::Message;

/// Wires 0..5: wire 1 is a public output, wire 2 a public input, wires 3 and 4
/// private. Equations: w1 · w2 = w3 and w1 · w3 = w4.
fn source() -> Vec<u8> {
    r1cs_bytes([5, 1, 1, 2], &[[1, 2, 3], [1, 3, 4]])
}

pub fn config(width: usize) -> LayoutConfig {
    LayoutConfig {
        options: ConversionOptions {
            width,
            chunk: 2,
            reuse: Reuse::Available,
            fold: true,
        },
        strategy: Strategy::Components,
        min_rows: 64,
    }
}

/// `source()` packed at `width`; width 3 needs a copy argument, width 4 does not.
fn artifact(width: usize) -> PackedArtifact {
    packed::compile(&source(), &config(width)).unwrap()
}

/// One shared input connects more equations than fit on a row, so a copy
/// argument is needed even at large widths.
fn connected_source(n: u32) -> Vec<u8> {
    let wires = 2 * n + 2;
    let equations: Vec<_> = (0..n).map(|i| [1, 2 * i + 2, 2 * i + 3]).collect();
    r1cs_bytes([wires, 1, 1, wires - 3], &equations)
}

/// An artifact with a copy argument and ratio columns.
fn ratio_artifact(width: usize) -> (Vec<u8>, PackedArtifact) {
    let source = connected_source(width as u32 + 1);
    let artifact = packed::compile(&source, &config(width)).unwrap();
    assert!(artifact.certificate.layout.requires_copy);
    (source, artifact)
}

/// Apply `change` to the artifact's PILout, update the certificate's hash, and
/// require the packed checker to reject the result with `expected`. The
/// unchanged artifact, decoded and encoded again, must be accepted.
fn rejects(
    expected: &str,
    source: &[u8],
    mut artifact: PackedArtifact,
    change: impl FnOnce(&mut pb::PilOut),
) {
    let unchanged = changed_pilout(&artifact.pilout, |_| {});
    assert!(packed_check::check(source, &unchanged, &artifact.certificate).is_ok());
    let bytes = changed_pilout(&artifact.pilout, change);
    artifact.certificate.pilout_sha256 = sha256_hex(&bytes);
    assert_rejected(
        packed_check::check(source, &bytes, &artifact.certificate),
        expected,
    );
}

/// `rejects` on `source()` packed at width 3, with a copy argument.
fn rejects_copy(expected: &str, change: impl FnOnce(&mut pb::PilOut)) {
    rejects(expected, &source(), artifact(3), change);
}

/// `rejects` on an artifact with one ratio column.
fn rejects_ratio(expected: &str, change: impl FnOnce(&mut pb::PilOut)) {
    let (source, artifact) = ratio_artifact(8);
    rejects(expected, &source, artifact, change);
}

#[test]
fn layouts_with_and_without_copy_argument_are_accepted() {
    for width in [3, 4] {
        let a = artifact(width);
        assert_eq!(a.certificate.layout.requires_copy, width == 3);
        assert!(packed_check::check(&source(), &a.pilout, &a.certificate).is_ok());
    }
}

#[test]
fn circuits_without_public_values_are_accepted() {
    // `source()` with its output and input marked private.
    let source = r1cs_bytes([5, 0, 0, 4], &[[1, 2, 3], [1, 3, 4]]);
    for width in [3, 4] {
        let a = packed::compile(&source, &config(width)).unwrap();
        assert!(a.certificate.layout.public_bindings.is_empty());
        assert!(packed_check::check(&source, &a.pilout, &a.certificate).is_ok());
    }
}

#[test]
fn public_row_selectors_are_accepted_at_square_boundaries() {
    // Equation i is p_i · x_i = y_i and fills one width-3 row, so each public
    // value lands in its own row. 4 and 9 rows are square boundaries of the
    // PH/PL grouping.
    for k in [4u32, 9] {
        let wires = 1 + 3 * k;
        let equations: Vec<_> = (0..k)
            .map(|i| [1 + i, 1 + k + 2 * i, 2 + k + 2 * i])
            .collect();
        let source = r1cs_bytes([wires, 0, k, wires - 1 - k], &equations);
        let mut settings = config(3);
        settings.options.reuse = Reuse::None;
        let a = packed::compile(&source, &settings).unwrap();
        let rows: std::collections::BTreeSet<_> = a
            .certificate
            .layout
            .public_bindings
            .iter()
            .map(|b| b.row)
            .collect();
        assert_eq!(rows.len(), k as usize);
        assert!(packed_check::check(&source, &a.pilout, &a.certificate).is_ok());
    }
}

#[test]
fn unused_public_wire_gets_its_own_row() {
    // Wire 1 is public but in no equation; the only equation fills row 0.
    let source = r1cs_bytes([5, 0, 1, 3], &[[2, 3, 4]]);
    let a = packed::compile(&source, &config(3)).unwrap();
    assert_eq!(a.certificate.layout.arithmetic_rows, 1);
    assert_eq!(a.certificate.layout.used_rows, 2);
    assert!(packed_check::check(&source, &a.pilout, &a.certificate).is_ok());
}

#[test]
fn ratio_column_counts_match_the_formula() {
    for width in [3, 4, 5, 6, 7, 8, 9, 12, 13, 16, 35, 36, 37, 64, 96, 192] {
        let (source, a) = ratio_artifact(width);
        packed_check::check(&source, &a.pilout, &a.certificate).unwrap();
        let pilout = pb::PilOut::decode(a.pilout.as_slice()).unwrap();
        let ratios = if width <= 3 {
            0
        } else {
            ((width - 1) / 4).max(1) as u32
        };
        assert_eq!(pilout.air_groups[0].airs[0].stage_widths[1], 1 + ratios);
        let ratio_hints = pilout.hints.iter().filter(|h| h.name == "im_col").count();
        assert_eq!(ratio_hints, ratios as usize);
    }
}

#[test]
fn inputs_above_small_size_limits_are_accepted() {
    // Inputs of a few hundred KiB pass every size limit, so a limit computed
    // with the wrong operator (for example `256 * 1024` for 256 MiB) fails here.
    let source = connected_source(6000);
    assert!(source.len() > 256 * 1024 + 1024);
    let circuit = r1cs::read(&source).unwrap();
    let (bytes, cert) = crate::compiler::compile(&source, &circuit, 64, false).unwrap();
    assert!(bytes.len() > 128 * 1024 + 1024);
    assert!(crate::checker::check(&source, &bytes, &cert).is_ok());
    let a = packed::compile(&source, &config(8)).unwrap();
    assert!(a.pilout.len() > 128 * 1024 + 1024);
    assert!(packed_check::check(&source, &a.pilout, &a.certificate).is_ok());
}

#[test]
fn changed_fixed_columns_are_rejected() {
    rejects_copy("fixed column E0 differs at row 0", |p| {
        fixed_column(p, "E0").values[0] = vec![7];
    });
    rejects_copy("fixed column S0 differs at row 0", |p| {
        fixed_column(p, "S0").values[0] = vec![1];
    });
    rejects_copy("fixed column ID differs at row 1", |p| {
        fixed_column(p, "ID").values[1] = vec![1];
    });
}

#[test]
fn changed_constraints_are_rejected() {
    rejects_copy(
        "constraint 0 differs from the expected slot or public constraint",
        |p| reuse_expression(p, 0, 1),
    );
    rejects_copy(
        "copy recurrence or closure differs from the expected one",
        |p| {
            let closure = constraint_labeled(p, label::COPY_CLOSURE);
            let recurrence = constraint_labeled(p, label::COPY_RECURRENCE);
            reuse_expression(p, closure, recurrence);
        },
    );
    rejects_ratio(
        "copy ratio constraint 0 differs from the expected one",
        |p| {
            let ratio = constraint_labeled(p, label::COPY_RATIO);
            let recurrence = constraint_labeled(p, label::COPY_RECURRENCE);
            reuse_expression(p, ratio, recurrence);
        },
    );
    rejects_copy("must be labeled 'copy closure'", |p| {
        let closure = constraint_labeled(p, label::COPY_CLOSURE);
        let Some(pb::constraint::Constraint::EveryRow(row)) =
            &mut air(p).constraints[closure].constraint
        else {
            unreachable!("packed constraints apply to every row")
        };
        row.debug_line = Some("other".into());
    });
}

#[test]
fn changed_challenge_in_the_copy_argument_is_rejected() {
    // Swap one challenge leaf, keeping all operands valid.
    for (from, to) in [(0, 1), (1, 0)] {
        rejects_ratio(
            "copy recurrence or closure differs from the expected one",
            |p| {
                use pb::expression::Operation;
                for expression in air(p).expressions.iter_mut().rev() {
                    let (lhs, rhs) = match &mut expression.operation {
                        Some(Operation::Add(x)) => (&mut x.lhs, &mut x.rhs),
                        Some(Operation::Sub(x)) => (&mut x.lhs, &mut x.rhs),
                        Some(Operation::Mul(x)) => (&mut x.lhs, &mut x.rhs),
                        Some(Operation::Neg(_)) | None => continue,
                    };
                    for operand in [lhs, rhs].into_iter().flatten() {
                        if let Some(pb::operand::Operand::Challenge(c)) = &mut operand.operand {
                            if c.idx == from {
                                c.idx = to;
                                return;
                            }
                        }
                    }
                }
                panic!("no challenge operand");
            },
        );
    }
}

#[test]
fn changed_hints_are_rejected() {
    rejects_copy("hint 'gprod_col': name, AIR or field layout", |p| {
        p.hints[0].name = "gsum_col".into();
    });
    rejects_copy("hint 'gprod_col': field names", |p| {
        hint_fields(p, "gprod_col").pop();
    });
    rejects_copy("hint 'gprod_col': field names", |p| {
        hint_fields(p, "gprod_col")[0].name = Some("renamed".into());
    });
    rejects_ratio(
        "hint 'im_col': field 'numerator' differs from the expected expression",
        |p| {
            let fields = hint_fields(p, "im_col");
            let position = |name: &str| {
                fields
                    .iter()
                    .position(|f| f.name.as_deref() == Some(name))
                    .unwrap()
            };
            let (numerator, denominator) = (position("numerator"), position("denominator"));
            fields[numerator].value = fields[denominator].value.clone();
        },
    );
    rejects_ratio("hint count", |p| {
        p.hints.remove(0);
    });
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
            Box::new(|p| p.base_field = vec![1]),
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
        (
            "AIR stage widths",
            Box::new(|p| air(p).stage_widths[0] += 1),
        ),
        (
            "AIR stage widths",
            Box::new(|p| air(p).stage_widths[1] += 1),
        ),
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
            "unexpected fixed column",
            Box::new(|p| air(p).fixed_cols.push(Default::default())),
        ),
        (
            "is missing",
            Box::new(|p| {
                air(p).fixed_cols.pop();
            }),
        ),
        (
            "must have one value per row",
            Box::new(|p| {
                air(p).fixed_cols[0].values.pop();
            }),
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
    for width in [3, 4] {
        for (expected, change) in &changes {
            rejects(expected, &source(), artifact(width), change);
        }
    }
}

#[test]
fn operands_are_checked_against_the_layout() {
    use pb::operand::{self as o, Operand};
    let accepts = |width: usize, valid: Operand| {
        let mut a = artifact(width);
        let bytes = changed_pilout(&a.pilout, with_unreferenced_operand(valid));
        a.certificate.pilout_sha256 = sha256_hex(&bytes);
        assert!(packed_check::check(&source(), &bytes, &a.certificate).is_ok());
    };
    // Width 3 has a copy argument: ID, S0..S2 and FIRST (index 4), then selectors.
    let fixed_count = pb::PilOut::decode(artifact(3).pilout.as_slice())
        .unwrap()
        .air_groups[0]
        .airs[0]
        .fixed_cols
        .len() as u32;
    accepts(3, witness_column(1, 2, 0));
    accepts(3, witness_column(2, 0, -1));
    accepts(3, fixed_operand(4, 1));
    accepts(3, fixed_operand(fixed_count - 1, 0));
    accepts(3, Operand::PublicValue(o::PublicValue { idx: 1 }));
    accepts(3, challenge_operand(2, 1));
    let witness = "witness column out of range";
    let fixed = "fixed column out of range or with a row offset";
    let cases = [
        (witness_column(1, 3, 0), witness),
        (witness_column(1, 0, -1), witness),
        (witness_column(2, 1, 0), witness),
        (witness_column(2, 1, -1), witness),
        (witness_column(2, 0, 1), witness),
        (witness_column(3, 0, 0), witness),
        (fixed_operand(fixed_count, 0), fixed),
        (fixed_operand(3, 1), fixed),
        (fixed_operand(4, -1), fixed),
        (
            Operand::PublicValue(o::PublicValue { idx: 2 }),
            "public value out of range",
        ),
        (challenge_operand(2, 2), "challenge out of range"),
        (challenge_operand(1, 0), "challenge out of range"),
        (
            Operand::Expression(o::Expression { idx: 1_000_000 }),
            "expression refers to itself or a later expression",
        ),
    ];
    for (bad, expected) in cases {
        rejects(
            expected,
            &source(),
            artifact(3),
            with_unreferenced_operand(bad),
        );
    }
    // Without a copy argument no previous-row or next-row reference is allowed.
    accepts(4, witness_column(2, 0, 0));
    for (bad, expected) in [
        (witness_column(2, 0, -1), witness),
        (witness_column(2, 1, 0), witness),
        (fixed_operand(0, 1), fixed),
    ] {
        rejects(
            expected,
            &source(),
            artifact(4),
            with_unreferenced_operand(bad),
        );
    }
}

#[test]
fn certificate_fields_are_checked() {
    let check = |certificate| packed_check::check(&source(), &artifact(3).pilout, &certificate);
    assert!(check(artifact(3).certificate).is_ok());
    let mut changed = artifact(3).certificate;
    changed.source_sha256 = "00".repeat(32);
    assert_rejected(check(changed), "certificate version, relation or hashes");
    let mut changed = artifact(3).certificate;
    changed.pilout_sha256 = "00".repeat(32);
    assert_rejected(check(changed), "certificate version, relation or hashes");
    let mut changed = artifact(3).certificate;
    changed.version = 2;
    assert_rejected(check(changed), "certificate version, relation or hashes");
    let mut changed = artifact(3).certificate;
    changed.relation = REFERENCE_RELATION.into();
    assert_rejected(check(changed), "certificate version, relation or hashes");
    let mut changed = artifact(3).certificate;
    changed.layout.requires_copy = false;
    assert_rejected(check(changed), "requiresCopy must be set exactly when");
    let mut changed = artifact(3).certificate;
    changed.network.converted.pop();
    assert_rejected(check(changed), "one converted equation per source equation");
    let mut changed = artifact(4).certificate;
    changed.layout.public_bindings[0].wire = 4;
    let pilout = artifact(4).pilout;
    assert_rejected(
        packed_check::check(&source(), &pilout, &changed),
        "public binding i must bind public value i to wire i + 1",
    );
}

#[test]
fn witnesses_are_checked_and_projected() {
    let a = artifact(3);
    let values = [1, 3, 5, 15, 45];
    let projected = witness::project(&source(), &a.certificate, &wtns_bytes(&values)).unwrap();
    assert_eq!(projected.publics, [3, 5]);
    assert_eq!(projected.trace.len(), a.map.len());
    for (wire, value) in a.map.iter().zip(projected.trace) {
        let expected = if *wire == 0 {
            0
        } else {
            values[*wire as usize]
        };
        assert_eq!(value, expected);
    }
    assert_rejected(
        witness::project(&source(), &a.certificate, &wtns_bytes(&[1, 3, 5, 15, 46])),
        "witness does not satisfy equation 1",
    );
    assert_rejected(
        witness::project(&source(), &a.certificate, &wtns_bytes(&[1, 3, P, 15, 45])),
        "WTNS value of wire 2 not below P",
    );
}
