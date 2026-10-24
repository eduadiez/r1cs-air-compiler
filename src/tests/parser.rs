//! R1CS and WTNS parsing, and the decimal form of certificate coefficients.
use super::{
    assert_limit, assert_rejected, header, r1cs_bytes, r1cs_with_terms, set_u32, set_u64,
    wtns_bytes,
};
use crate::{field::FieldElement, r1cs, P};

/// Wires 0..4: wire 1 is a public output, wires 2 and 3 public inputs. One
/// equation: w2 · w3 = w1.
pub fn small_source() -> Vec<u8> {
    r1cs_bytes([4, 1, 2, 0], &[[2, 3, 1]])
}

fn with_header(edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut source = small_source();
    edit(&mut source);
    source
}

#[test]
fn every_truncation_is_rejected() {
    let source = small_source();
    for end in 0..source.len() {
        assert!(r1cs::read(&source[..end]).is_err(), "truncation at {end}");
    }
}

#[test]
fn malformed_files_are_rejected() {
    let mut trailing = small_source();
    trailing.push(0);
    assert_rejected(r1cs::read(&trailing), "trailing section bytes");
    let mut version = small_source();
    set_u32(&mut version, 4, 2);
    assert_rejected(r1cs::read(&version), "unsupported R1CS version");
    let mut prime = small_source();
    prime[28] ^= 1;
    assert_rejected(r1cs::read(&prime), "field must be Goldilocks");
    assert_rejected(r1cs::read(b"r1cx"), "not an R1CS file");
}

#[test]
fn sections_and_header_fields_are_checked() {
    let with_sections = |count: u32, tags: &[u32]| {
        let mut bytes = b"r1cs".to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(count.to_le_bytes());
        for tag in tags {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(0u64.to_le_bytes());
        }
        bytes
    };
    assert_rejected(
        r1cs::read(&with_sections(4, &[])),
        "expected 2 or 3 sections",
    );
    assert_rejected(
        r1cs::read(&with_sections(2, &[1, 4])),
        "unsupported section type",
    );
    assert_rejected(
        r1cs::read(&with_sections(2, &[1, 1])),
        "duplicate R1CS section",
    );
    assert_rejected(
        r1cs::read(&with_sections(2, &[2, 3])),
        "R1CS header section missing",
    );
    let mut size = small_source();
    set_u32(&mut size, 24, 32);
    assert_rejected(r1cs::read(&size), "field elements must be 8 bytes");
    // Factor A with two terms on the same wire.
    let repeated = r1cs_with_terms(
        [4, 1, 2, 0],
        &[[vec![(2, 1), (2, 1)], vec![(3, 1)], vec![(1, 1)]]],
    );
    assert_rejected(r1cs::read(&repeated), "duplicate linear-combination wire 2");
}

#[test]
fn coefficients_and_wires_out_of_range_are_rejected() {
    let source = small_source();
    let n = source.len();
    let mut coefficient = source.clone();
    set_u64(&mut coefficient, n - 8, P);
    assert_rejected(
        r1cs::read(&coefficient),
        "noncanonical coefficient for wire 1",
    );
    let mut wire = source;
    set_u32(&mut wire, n - 12, 4);
    assert_rejected(r1cs::read(&wire), "wire 4 outside declared range 0..4");
}

#[test]
fn errors_name_the_equation_and_factor() {
    let mut source = small_source();
    set_u32(&mut source, 80, 999);
    assert_rejected(
        r1cs::read(&source),
        "equation 0, factor A: wire 999 outside declared range",
    );
}

#[test]
fn header_counts_are_checked() {
    // Every wire public: no private wire remains.
    let all_public = with_header(|s| set_u32(s, header::INPUTS, 3));
    assert_rejected(
        r1cs::read(&all_public),
        "wire count must exceed the public count",
    );
    // Fewer labels than wires.
    let few_labels = with_header(|s| {
        set_u32(s, header::OUTPUTS, 0);
        set_u32(s, header::INPUTS, 1);
        set_u64(s, header::LABELS, 3);
    });
    assert_rejected(r1cs::read(&few_labels), "not exceed the label count");
    // Private and public inputs exactly fill the label count.
    let full = with_header(|s| set_u32(s, header::PRIVATE, 1));
    assert_rejected(
        r1cs::read(&full),
        "input counts must be below the label count",
    );
    // More than the supported number of wires.
    let large = with_header(|s| {
        set_u32(s, header::WIRES, 1_000_001);
        set_u64(s, header::LABELS, 2_000_000);
    });
    assert_limit(r1cs::read(&large), "circuit exceeds 1000000 wires");
}

#[test]
fn private_inputs_removed_by_circom_are_accepted() {
    // Circom --O2 may remove private input wires but still count them.
    let source = with_header(|s| {
        set_u32(s, header::PRIVATE, 4);
        set_u64(s, header::LABELS, 8);
    });
    assert_eq!(r1cs::read(&source).unwrap().wires, 4);
}

#[test]
fn wire_labels_must_be_in_range() {
    let with_map = |label: u64| {
        let mut source = small_source();
        set_u32(&mut source, 8, 3);
        source.extend(3u32.to_le_bytes());
        source.extend(32u64.to_le_bytes());
        for l in [0, 1, 2, label] {
            source.extend(l.to_le_bytes());
        }
        source
    };
    assert!(r1cs::read(&with_map(3)).is_ok());
    assert_rejected(r1cs::read(&with_map(4)), "wire label out of range");
}

#[test]
fn witness_values_are_checked() {
    assert_eq!(
        r1cs::read_witness(&wtns_bytes(&[1, 3, 5]), 3).unwrap(),
        [1, 3, 5]
    );
    assert_rejected(
        r1cs::read_witness(&wtns_bytes(&[0, 3, 5]), 3),
        "WTNS wire 0 must be 1",
    );
    assert_rejected(
        r1cs::read_witness(&wtns_bytes(&[1, P, 5]), 3),
        "WTNS value of wire 1 not below P",
    );
    assert_rejected(
        r1cs::read_witness(&wtns_bytes(&[1, 3]), 3),
        "WTNS wire count differs from the circuit",
    );
    assert_rejected(r1cs::read_witness(b"invalid", 3), "not a WTNS file");
    let mut version = wtns_bytes(&[1, 3, 5]);
    set_u32(&mut version, 4, 1);
    assert_rejected(r1cs::read_witness(&version, 3), "unsupported WTNS version");
    let mut prime = wtns_bytes(&[1, 3, 5]);
    prime[28] ^= 1;
    assert_rejected(
        r1cs::read_witness(&prime, 3),
        "WTNS field must be Goldilocks",
    );
}

#[test]
fn certificate_coefficients_must_be_canonical() {
    let parse = |text: &str| serde_json::from_str::<FieldElement>(&format!("\"{text}\""));
    for valid in ["0", "1", "18446744069414584320"] {
        assert!(parse(valid).is_ok(), "{valid}");
    }
    for invalid in ["18446744069414584321", "18446744073709551615", "01", "+1"] {
        let error = parse(invalid).unwrap_err().to_string();
        assert!(error.contains("must be below P"), "{invalid}: {error}");
    }
    let error = parse("x").unwrap_err().to_string();
    assert!(error.contains("not a decimal number"), "{error}");
}
