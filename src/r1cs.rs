//! Reader for R1CS version 1 and WTNS version 2 files over the Goldilocks field.
//! Anything else is rejected, including custom gates and other fields. The
//! compiler and the checkers both use this parser, so both rely on it being correct.
use crate::{
    contract::{MAX_CIRCUIT_SIZE, MAX_R1CS_BYTES},
    ensure,
    field::FieldElement,
    Context, Result, P,
};
use std::collections::{BTreeMap, BTreeSet};

/// Linear combination as `(wire, coefficient)` terms; wire 0 is the constant 1.
pub type LinearCombination = Vec<(u32, FieldElement)>;
/// The equation `A · B = C`, as the linear combinations `[A, B, C]`.
pub type Equation = [LinearCombination; 3];
/// A parsed R1CS circuit.
#[derive(Clone, Debug)]
pub struct Circuit {
    /// Number of wires, including the constant wire 0.
    pub wires: u32,
    /// Number of public wires: outputs and public inputs, wires `1..=publics`.
    pub publics: u32,
    pub equations: Vec<Equation>,
}

/// Little-endian reader over a byte slice.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or("offset overflow")?;
        let result = self.data.get(self.at..end).ok_or_else(|| {
            format!(
                "truncated binary input at byte {} (need {n} bytes)",
                self.at
            )
        })?;
        self.at = end;
        Ok(result)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn end(&self) -> Result<()> {
        ensure(self.at == self.data.len(), "trailing section bytes")
    }
    /// A linear combination: a term count, then `(wire, coefficient)` pairs.
    fn linear_combination(&mut self, wires: u32) -> Result<LinearCombination> {
        let n = self.u32()? as usize;
        ensure(
            n <= (self.data.len() - self.at) / 12,
            "invalid linear-combination length",
        )?;
        let mut seen = BTreeSet::new();
        let mut terms = Vec::with_capacity(n);
        for _ in 0..n {
            let wire = self.u32()?;
            let coefficient = self.u64()?;
            ensure(
                wire < wires,
                format!("wire {wire} outside declared range 0..{wires}"),
            )?;
            ensure(
                coefficient < P,
                format!("noncanonical coefficient for wire {wire}"),
            )?;
            ensure(
                seen.insert(wire),
                format!("duplicate linear-combination wire {wire}"),
            )?;
            terms.push((wire, FieldElement(coefficient)));
        }
        terms.sort_by_key(|(wire, _)| *wire);
        Ok(terms)
    }
}

/// Parse an R1CS file.
pub fn read(data: &[u8]) -> Result<Circuit> {
    ensure(
        data.len() <= MAX_R1CS_BYTES,
        "R1CS file exceeds the size limit",
    )?;
    let mut reader = Reader::new(data);
    ensure(reader.take(4)? == b"r1cs", "not an R1CS file")?;
    ensure(reader.u32()? == 1, "unsupported R1CS version")?;
    let section_count = reader.u32()?;
    ensure(
        (2..=3).contains(&section_count),
        "expected 2 or 3 sections (custom gates are not supported)",
    )?;
    let mut sections = BTreeMap::new();
    for _ in 0..section_count {
        let tag = reader.u32()?;
        let length = usize::try_from(reader.u64()?)?;
        ensure(
            (1..=3).contains(&tag),
            "unsupported section type (custom gates are not supported)",
        )?;
        ensure(
            sections.insert(tag, reader.take(length)?).is_none(),
            "duplicate R1CS section",
        )?;
    }
    reader.end()?;
    let mut header = Reader::new(sections.get(&1).ok_or("R1CS header section missing")?);
    ensure(header.u32()? == 8, "field elements must be 8 bytes")?;
    ensure(header.u64()? == P, "field must be Goldilocks")?;
    let wires = header.u32()?;
    let outputs = header.u32()?;
    let inputs = header.u32()?;
    let private = header.u32()?;
    let labels = header.u64()?;
    let equation_count = header.u32()?;
    header.end()?;
    let publics = outputs
        .checked_add(inputs)
        .ok_or("public count overflows")?;
    // With --O2, Circom may remove private input wires but still count them
    // in the header, so their count is only checked against the label count.
    ensure(
        wires > publics && labels >= u64::from(wires),
        "wire count must exceed the public count and not exceed the label count",
    )?;
    ensure(
        u64::from(private) + u64::from(publics) < labels,
        "input counts must be below the label count",
    )?;
    ensure(
        wires <= MAX_CIRCUIT_SIZE && equation_count <= MAX_CIRCUIT_SIZE,
        format!("circuit exceeds {MAX_CIRCUIT_SIZE} wires or equations"),
    )?;
    let mut constraints = Reader::new(sections.get(&2).ok_or("R1CS constraint section missing")?);
    ensure(
        equation_count as usize <= constraints.data.len() / 12,
        "equation count larger than the constraint section",
    )?;
    let mut equations = Vec::with_capacity(equation_count as usize);
    for index in 0..equation_count {
        equations.push([
            constraints
                .linear_combination(wires)
                .context(format!("equation {index}, factor A"))?,
            constraints
                .linear_combination(wires)
                .context(format!("equation {index}, factor B"))?,
            constraints
                .linear_combination(wires)
                .context(format!("equation {index}, factor C"))?,
        ]);
    }
    constraints.end()?;
    if let Some(map) = sections.get(&3) {
        ensure(
            map.len() == wires as usize * 8,
            "wire-label map must hold one label per wire",
        )?;
        let mut map = Reader::new(map);
        for _ in 0..wires {
            ensure(map.u64()? < labels, "wire label out of range")?;
        }
    }
    Ok(Circuit {
        wires,
        publics,
        equations,
    })
}

/// Parse a WTNS file for a circuit with `wires` wires: one value per wire,
/// starting with the constant 1.
pub fn read_witness(data: &[u8], wires: u32) -> Result<Vec<u64>> {
    let mut reader = Reader::new(data);
    ensure(reader.take(4)? == b"wtns", "not a WTNS file")?;
    ensure(reader.u32()? == 2, "unsupported WTNS version")?;
    ensure(reader.u32()? == 2, "WTNS file must have two sections")?;
    let mut sections = BTreeMap::new();
    for _ in 0..2 {
        let id = reader.u32()?;
        let length = usize::try_from(reader.u64()?)?;
        ensure(
            (1..=2).contains(&id) && sections.insert(id, reader.take(length)?).is_none(),
            "WTNS sections must be one header (1) and one value section (2)",
        )?;
    }
    reader.end()?;
    let mut header = Reader::new(sections.get(&1).ok_or("WTNS header section missing")?);
    ensure(header.u32()? == 8, "WTNS field elements must be 8 bytes")?;
    ensure(header.u64()? == P, "WTNS field must be Goldilocks")?;
    ensure(
        header.u32()? == wires,
        "WTNS wire count differs from the circuit",
    )?;
    header.end()?;
    let bytes = sections.get(&2).ok_or("WTNS value section missing")?;
    ensure(
        bytes.len() == wires as usize * 8,
        "WTNS value section must hold one value per wire",
    )?;
    let mut values = Reader::new(bytes);
    let mut result = vec![];
    for wire in 0..wires {
        let value = values.u64()?;
        ensure(value < P, format!("WTNS value of wire {wire} not below P"))?;
        result.push(value);
    }
    ensure(result.first() == Some(&1), "WTNS wire 0 must be 1")?;
    Ok(result)
}
