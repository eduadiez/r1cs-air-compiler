//! Tests of the public library API.
use r1cs_air_compiler::{self as air, ConversionOptions, LayoutConfig, Reuse, Strategy};
use std::error::Error as _;

const SOURCE: &[u8] = include_bytes!("../fixtures/xor/kernel.r1cs");
const WITNESS: &[u8] = include_bytes!("../fixtures/xor/input.wtns");

fn config() -> LayoutConfig {
    LayoutConfig {
        options: ConversionOptions {
            width: 8,
            chunk: 4,
            reuse: Reuse::Prefix,
            fold: true,
        },
        strategy: Strategy::Local,
        min_rows: 64,
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
    assert_eq!(plan.helper_count(), certificate.helper_count());
    assert_eq!(plan.requires_copy(), certificate.requires_copy());
    assert!(plan.checked_equations() > 0);
    let report = air::check_packed(SOURCE, &packed.pilout, certificate)?;
    assert!(report.accepted && report.requires_copy);
    let witness = air::project_witness(SOURCE, &packed.pilout, certificate, WITNESS)?;
    assert_eq!(witness.trace.len(), plan.rows() * plan.width());
    assert_eq!(witness.publics, [3_974_687_968, 305_419_896, 4_275_878_552]);
    Ok(())
}

#[test]
fn relation_names_and_prime_are_fixed() {
    assert_eq!(air::REFERENCE_RELATION, "r1cs-per-row-publics-at-row-zero");
    assert_eq!(air::PACKED_RELATION, "r1cs-packed-static-copy");
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
