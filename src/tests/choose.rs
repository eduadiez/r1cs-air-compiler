//! Layout settings: their defaults, and the search for a missing row count or
//! row group width.
use super::{assert_limit, assert_rejected, packed::config, r1cs_bytes};
use crate::{
    certificate::Layout,
    choose::{self, cell_map, estimate},
    layout::{self, LayoutConfig},
    packed, packed_check, placement_check, proto as pb, r1cs,
    rewrite::Reuse,
};
use prost::Message;

const XOR: &[u8] = include_bytes!("../../fixtures/xor/kernel.r1cs");
const XOR_LAYOUT: &[u8] = include_bytes!("../../fixtures/xor/layout.json");

#[test]
fn missing_settings_take_their_defaults() {
    let artifact = packed::compile(XOR, &LayoutConfig::default()).unwrap();
    let config = &artifact.config;
    let width = config.width.unwrap();
    assert!([4, 8, 16, 32, 64, 128].contains(&width));
    assert_eq!(
        (
            config.auxiliary_terms,
            config.reuse_auxiliaries,
            config.fold_constants
        ),
        (Some((width / 2).max(2)), Some(Reuse::Available), Some(true))
    );
    assert!(config.row_group_width == Some(width) || config.row_group_width == Some(2 * width));
    assert_eq!(config.min_rows, Some(artifact.certificate.layout.min_rows));
    packed_check::check(XOR, &artifact.pilout, &artifact.certificate).unwrap();
    // The settings used, given back, build the same artifact.
    assert_eq!(packed::compile(XOR, config).unwrap(), artifact);
}

/// `xor` has three equations of 33 signals: at the starting width, 64, they
/// fill long rows and make the trace 36 columns wide, while narrow widths
/// split them and stack everything into a few columns.
#[test]
fn the_search_narrows_the_width_when_that_is_cheaper() {
    let circuit = r1cs::read(XOR).unwrap();
    let cost = |config: &LayoutConfig| {
        let (_, rewritten, layout) = choose::layout(&circuit, config).unwrap();
        estimate(&packed::build_pilout(&rewritten, &layout, &cell_map(&layout)).unwrap())
    };
    let (chosen, _, layout) = choose::layout(&circuit, &LayoutConfig::default()).unwrap();
    let at_64 = LayoutConfig {
        width: Some(64),
        ..LayoutConfig::default()
    };
    assert!(chosen.width.unwrap() < 64, "{chosen:?}");
    assert!(layout.width < choose::layout(&circuit, &at_64).unwrap().2.width);
    assert!(cost(&chosen) < cost(&at_64));
}

#[test]
fn given_settings_limit_the_widths_tried() {
    let circuit = r1cs::read(XOR).unwrap();
    let chosen = |config: LayoutConfig| choose::layout(&circuit, &config).unwrap().0.width.unwrap();
    // auxiliaryTerms must be below the width, and the width at most rowGroupWidth.
    assert!(
        chosen(LayoutConfig {
            auxiliary_terms: Some(20),
            ..LayoutConfig::default()
        }) > 20
    );
    assert!(
        chosen(LayoutConfig {
            row_group_width: Some(8),
            ..LayoutConfig::default()
        }) <= 8
    );
    let impossible = LayoutConfig {
        auxiliary_terms: Some(100),
        row_group_width: Some(100),
        ..LayoutConfig::default()
    };
    assert_rejected(
        choose::layout(&circuit, &impossible),
        "no width fits the given auxiliaryTerms and rowGroupWidth",
    );
}

#[test]
fn auxiliary_terms_default_to_half_the_width() {
    for (width, terms) in [(3, 2), (4, 2), (9, 4), (64, 32)] {
        let mut settings = config(width);
        settings.auxiliary_terms = None;
        let artifact = packed::compile(XOR, &settings).unwrap();
        assert_eq!(
            artifact.config.auxiliary_terms,
            Some(terms),
            "width {width}"
        );
    }
}

#[test]
fn given_settings_are_kept() {
    let settings: LayoutConfig = serde_json::from_slice(XOR_LAYOUT).unwrap();
    assert_eq!(packed::compile(XOR, &settings).unwrap().config, settings);
}

#[test]
fn the_search_picks_the_lowest_estimate() {
    let circuit = r1cs::read(XOR).unwrap();
    // Every sweep tries 64, 128 and 256 rows: it stops only after the
    // estimate has risen twice.
    let mut settings = config(8);
    settings.min_rows = None;
    settings.row_group_width = None;
    let (chosen, rewritten, layout) = choose::layout(&circuit, &settings).unwrap();
    let cost = |layout: &Layout| {
        estimate(&packed::build_pilout(&rewritten, layout, &cell_map(layout)).unwrap())
    };
    let lowest = cost(&layout);
    for row_group_width in [8, 16] {
        for min_rows in [64, 128, 256] {
            let candidate = layout::pack(&rewritten, 8, min_rows, row_group_width).unwrap();
            assert!(
                lowest <= cost(&candidate),
                "{min_rows} rows and row groups of {row_group_width} beat {chosen:?}"
            );
        }
    }
}

#[test]
fn cell_maps_match_the_placement_check() {
    let circuit = r1cs::read(XOR).unwrap();
    for width in [3, 4, 8] {
        let (_, rewritten, layout) = choose::layout(&circuit, &config(width)).unwrap();
        let checked = placement_check::check(&circuit, &rewritten, &layout).unwrap();
        assert_eq!(cell_map(&layout), checked.map, "width {width}");
    }
}

/// pil2-proofman's setup of this PILout reports 128 extended rows, 16 + 21 + 6
/// committed columns, 26 fixed columns and 303 quotient operations, which the
/// cost model turns into 5.5645 ms.
#[test]
fn estimates_use_the_counts_of_the_prover_setup() {
    let settings: LayoutConfig = serde_json::from_slice(XOR_LAYOUT).unwrap();
    let artifact = packed::compile(XOR, &settings).unwrap();
    let pilout = pb::PilOut::decode(artifact.pilout.as_slice()).unwrap();
    assert!((estimate(&pilout) - 5.564_512_576).abs() < 1e-6);
}

#[test]
fn searches_in_which_nothing_fits_fail_with_a_limit() {
    let settings = LayoutConfig {
        width: Some(1024),
        min_rows: Some(1 << 16),
        ..LayoutConfig::default()
    };
    assert_limit(packed::compile(XOR, &settings), "65536 rows");
    // 40 001 wires in equations need more cells than 64 rows of 1024 columns
    // hold, so that candidate is rejected without being packed.
    let equations: Vec<_> = (0..40_000).map(|i| [1, 2 * i + 2, 2 * i + 3]).collect();
    let source = r1cs_bytes([80_002, 1, 1, 79_999], &equations);
    let mut settings = config(3);
    settings.row_group_width = None;
    assert_limit(
        packed::compile(&source, &settings),
        "80001 wires need more than 1024 columns of 64 rows",
    );
}
