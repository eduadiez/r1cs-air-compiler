//! Tests of the public library API.
use r1cs_air_compiler::{self as air, LayoutConfig, Reuse, RewriteOptions};
use std::error::Error as _;

const SOURCE: &[u8] = include_bytes!("../fixtures/xor/kernel.r1cs");
const WITNESS: &[u8] = include_bytes!("../fixtures/xor/input.wtns");

fn config() -> LayoutConfig {
    LayoutConfig {
        options: RewriteOptions {
            width: 8,
            auxiliary_terms: 4,
            reuse_auxiliaries: Reuse::Prefix,
            fold_constants: true,
        },
        min_rows: 64,
        row_group_width: None,
    }
}

#[test]
fn reference_artifacts_compile_and_check() -> air::Result<()> {
    for artifact in [
        air::compile_reference(SOURCE, 64)?,
        air::compile_optimized_reference(SOURCE, 64)?,
    ] {
        let certificate = &artifact.certificate;
        assert_eq!(certificate.relation(), air::REFERENCE_RELATION);
        assert_eq!(certificate.version(), 1);
        assert_eq!((certificate.rows(), certificate.width()), (64, 96));
        assert_eq!(certificate.wire_columns()[0], None);
        let report = air::check_reference(SOURCE, &artifact.pilout, certificate)?;
        assert!(report.accepted && !report.formally_verified);
        assert_eq!(report.relation, air::REFERENCE_RELATION);
    }
    Ok(())
}

#[test]
fn packed_artifacts_plan_compile_check_and_project() -> air::Result<()> {
    let plan = air::plan_layout(SOURCE, &config())?;
    let packed = air::compile_packed(SOURCE, &config())?;
    let certificate = &packed.certificate;
    assert_eq!(certificate.relation(), air::PACKED_RELATION);
    assert_eq!(plan.map(), packed.map);
    assert_eq!(plan.rows(), certificate.rows());
    assert_eq!(plan.width(), certificate.width());
    assert_eq!(plan.auxiliary_count(), certificate.auxiliary_count());
    assert_eq!(
        plan.requires_permutation(),
        certificate.requires_permutation()
    );
    assert!(plan.checked_equations() > 0);
    let report = air::check_packed(SOURCE, &packed.pilout, certificate)?;
    assert!(report.accepted && report.requires_permutation);
    let witness = air::project_witness(SOURCE, &packed.pilout, certificate, WITNESS)?;
    assert_eq!(witness.trace.len(), plan.rows() * plan.width());
    assert_eq!(witness.publics, [3_974_687_968, 305_419_896, 4_275_878_552]);
    Ok(())
}

#[test]
fn relation_names_and_prime_are_fixed() {
    assert_eq!(air::REFERENCE_RELATION, "r1cs-per-row-publics-at-row-zero");
    assert_eq!(air::PACKED_RELATION, "r1cs-packed-static-copy");
    assert_eq!(air::MODULE_RELATION, "r1cs-module-per-row");
    assert_eq!(air::GOLDILOCKS_PRIME, 0xffff_ffff_0000_0001);
}

#[test]
fn errors_name_their_stage_and_keep_their_causes() {
    let error = air::compile_reference(b"bad", 64).unwrap_err();
    assert_eq!(error.to_string(), "reference source");
    let cause = error.source().expect("the parser error is the source");
    assert!(cause.to_string().contains("truncated"), "{cause}");
    assert_eq!(format!("{error:#}"), format!("reference source: {cause}"));
    let error = air::compile_reference(SOURCE, 3).unwrap_err();
    assert!(format!("{error:#}").starts_with("reference compilation: rows must be"));
}

#[test]
fn invalid_witnesses_are_rejected() -> air::Result<()> {
    let packed = air::compile_packed(SOURCE, &config())?;
    let error =
        air::project_witness(SOURCE, &packed.pilout, &packed.certificate, b"x").unwrap_err();
    assert!(format!("{error:#}").starts_with("witness projection: "));
    Ok(())
}

#[test]
fn errors_tell_whether_a_size_limit_was_exceeded() -> air::Result<()> {
    let kind = |error: air::Error| error.kind();
    let error = air::compile_reference(b"bad", 64).unwrap_err();
    assert_eq!(kind(error), air::ErrorKind::Invalid);
    let error = air::compile_reference(SOURCE, 3).unwrap_err();
    assert_eq!(kind(error), air::ErrorKind::Invalid);
    let packed = air::compile_packed(SOURCE, &config())?;
    let error =
        air::project_witness(SOURCE, &packed.pilout, &packed.certificate, b"x").unwrap_err();
    assert_eq!(kind(error), air::ErrorKind::Invalid);
    // 1024 columns and 65536 rows exceed the largest trace.
    let mut large = config();
    large.options.width = 1024;
    large.options.auxiliary_terms = 512;
    large.min_rows = 1 << 16;
    let error = air::compile_packed(SOURCE, &large).unwrap_err();
    assert_eq!(kind(error), air::ErrorKind::Limit);
    Ok(())
}

/// Slow in a debug build; CI runs it with `--release --ignored`.
#[test]
#[ignore = "large fixture; run with --release --ignored"]
fn circuits_too_large_for_the_reference_layout_exceed_a_limit() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/sha256/kernel.r1cs");
    let source = std::fs::read(path).unwrap();
    let error = air::compile_reference(&source, 64).unwrap_err();
    assert_eq!(error.kind(), air::ErrorKind::Limit, "{error:#}");
    let error = air::compile_optimized_reference(&source, 64).unwrap_err();
    assert_eq!(error.kind(), air::ErrorKind::Limit, "{error:#}");
}

#[test]
fn modules_compile_and_compose_into_a_host() -> air::Result<()> {
    let module = air::compile_module(SOURCE, &[1, 2], false)?;
    let certificate = &module.certificate;
    assert_eq!(certificate.relation(), air::MODULE_RELATION);
    assert_eq!(certificate.exports(), [1, 2]);
    let report = air::check_module(SOURCE, &module.pilout, certificate)?;
    assert!(report.accepted && !report.formally_verified);
    // Every reference constraint over column w - 1 enforces source equation
    // w on every row, so the reference artifact is a host at offset 0.
    let host = air::compile_reference(SOURCE, 64)?;
    let binding = air::CompositionBinding::with_offset(certificate, 0, 0, 0);
    let report = air::check_composition(SOURCE, &host.pilout, certificate, &binding)?;
    assert!(report.preserved && !report.formally_verified);
    assert_eq!(report.claim, air::COMPOSITION_CLAIM);
    let exports: Vec<_> = report.exports.iter().map(|e| (e.wire, e.column)).collect();
    assert_eq!(exports, [(1, 0), (2, 1)]);
    let shifted = air::CompositionBinding::with_offset(certificate, 0, 0, 1);
    assert!(air::check_composition(SOURCE, &host.pilout, certificate, &shifted).is_err());
    Ok(())
}
