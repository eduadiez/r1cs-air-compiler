//! Rewrites source equations so that each one references at most `width`
//! distinct wires, by introducing auxiliary wires constrained to equal linear
//! combinations of other wires. Also computes the canonical gate type of an equation.
use crate::{
    certificate::{Auxiliary, GateTerms, RewrittenCircuit},
    contract::{MAX_WIDTH, MIN_WIDTH},
    ensure,
    field::{add, inverse, mul, neg, FieldElement},
    r1cs::{Circuit, Equation, LinearCombination},
    Result, P,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// When an existing auxiliary wire may stand in for terms of a later equation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Reuse {
    /// Every rewrite creates a new auxiliary wire.
    None,
    /// Reuse an auxiliary wire when the chosen terms are a multiple of its terms.
    Prefix,
    /// Like `Prefix`, and also search all sides of the equation for the
    /// longest auxiliary wire whose terms appear there up to a common factor.
    Available,
}
/// Settings for rewriting source equations with auxiliary wires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RewriteOptions {
    /// Maximum number of distinct nonconstant wires per equation (`3..=1024`).
    pub width: usize,
    /// Number of terms replaced by one new auxiliary wire (`2..width`).
    pub auxiliary_terms: usize,
    /// When an existing auxiliary wire may be reused.
    pub reuse_auxiliaries: Reuse,
    /// If one factor of `A · B = C` is a constant `k`, rewrite the equation as
    /// `0 = C - k · (other factor)` before splitting.
    pub fold_constants: bool,
}
/// Sum terms with the same wire, drop zero coefficients and sort by wire.
fn canonical(terms: impl IntoIterator<Item = (u32, FieldElement)>) -> LinearCombination {
    let mut result = BTreeMap::new();
    for (wire, k) in terms {
        let sum = add(result.get(&wire).copied().unwrap_or(0), k.0);
        result.insert(wire, sum);
    }
    result
        .into_iter()
        .filter(|(_, k)| *k != 0)
        .map(|(wire, k)| (wire, FieldElement(k)))
        .collect()
}
/// The source equations with every side in canonical form.
fn source_equations(source: &Circuit) -> Vec<Equation> {
    source
        .equations
        .iter()
        .map(|equation| equation.clone().map(canonical))
        .collect()
}
/// Number of distinct nonconstant wires in `equation`.
fn equation_width(equation: &Equation) -> usize {
    equation
        .iter()
        .flatten()
        .filter(|(w, _)| *w != 0)
        .map(|(w, _)| *w)
        .collect::<BTreeSet<_>>()
        .len()
}
/// The linear equation `0 · 0 = terms - auxiliary` that defines an auxiliary wire.
fn auxiliary_equation(auxiliary: &Auxiliary) -> Equation {
    let terms = auxiliary.terms.iter().copied();
    [
        vec![],
        vec![],
        canonical(terms.chain([(auxiliary.id, FieldElement(P - 1))])),
    ]
}
/// Auxiliary definitions followed by the rewritten equations: the order in
/// which [`Placement::index`](crate::certificate::Placement::index) numbers them.
pub fn equations(rewritten: &RewrittenCircuit) -> Vec<Equation> {
    rewritten
        .auxiliaries
        .iter()
        .map(auxiliary_equation)
        .chain(rewritten.equations.iter().cloned())
        .collect()
}

/// Rewrite each source equation until it references at most `options.width`
/// distinct wires. Each step replaces some terms `Σ kᵢ·wᵢ` of one side by
/// `k₀·h`, where `h` is an auxiliary wire constrained to `Σ (kᵢ/k₀)·wᵢ`, so its first
/// coefficient is 1. Auxiliary wires only reference earlier wires.
pub fn convert(source: &Circuit, options: &RewriteOptions) -> Result<RewrittenCircuit> {
    ensure(
        (MIN_WIDTH..=MAX_WIDTH).contains(&options.width)
            && (2..options.width).contains(&options.auxiliary_terms),
        format_args!(
            "options.width must be {MIN_WIDTH}..={MAX_WIDTH} and options.auxiliary_terms 2..width (width={}, chunk={})",
            options.width, options.auxiliary_terms
        ),
    )?;
    let mut rewritten = RewrittenCircuit {
        original_wires: source.wires,
        wires: source.wires,
        publics: source.publics,
        auxiliaries: vec![],
        equations: vec![],
    };
    // Auxiliary id for each auxiliary wire's terms, used to reuse auxiliary wires across equations.
    let mut reusable = BTreeMap::new();
    for mut equation in source_equations(source) {
        if options.fold_constants {
            equation = fold_constant_factor(equation);
        }
        while equation_width(&equation) > options.width {
            let (side, chosen, existing) =
                choose_terms(&equation, options, &rewritten.auxiliaries)?;
            let scale = chosen[0].1 .0;
            let inverse_scale = inverse(scale);
            let normalized: LinearCombination = chosen
                .iter()
                .map(|(wire, k)| (*wire, FieldElement(mul(k.0, inverse_scale))))
                .collect();
            let reused = existing.or_else(|| {
                if options.reuse_auxiliaries == Reuse::None {
                    None
                } else {
                    reusable.get(&normalized).copied()
                }
            });
            let id = match reused {
                Some(id) => id,
                None => {
                    // Each new auxiliary wire removes at least one term, so the auxiliary wire
                    // count is bounded by the input size and the id fits in u32.
                    let id = rewritten.original_wires + rewritten.auxiliaries.len() as u32;
                    ensure(
                        normalized.iter().all(|(wire, _)| *wire < id),
                        "auxiliary wire would refer to itself or a later wire",
                    )?;
                    reusable.insert(normalized.clone(), id);
                    rewritten.auxiliaries.push(Auxiliary {
                        id,
                        terms: normalized,
                    });
                    id
                }
            };
            // Replace the chosen terms, which equal scale·h, by the single term scale·h.
            let removed: BTreeSet<_> = chosen.iter().map(|(wire, _)| *wire).collect();
            let next = canonical(
                equation[side]
                    .iter()
                    .filter(|(wire, _)| !removed.contains(wire))
                    .copied()
                    .chain([(id, FieldElement(scale))]),
            );
            ensure(
                next.len() < equation[side].len(),
                "an auxiliary wire must reduce the number of terms",
            )?;
            equation[side] = next;
        }
        rewritten.equations.push(equation);
    }
    rewritten.wires += rewritten.auxiliaries.len() as u32;
    Ok(rewritten)
}

/// If `A` or `B` is a constant `k`, rewrite `A·B = C` as the linear equation
/// `0·0 = C - k·(other factor)`. Otherwise return `equation` unchanged.
fn fold_constant_factor(equation: Equation) -> Equation {
    for side in 0..2 {
        if equation[side].iter().all(|(w, _)| *w == 0) {
            let k = equation[side].first().map_or(0, |(_, k)| k.0);
            return [
                vec![],
                vec![],
                canonical(
                    equation[2].iter().copied().chain(
                        equation[1 - side]
                            .iter()
                            .map(|(w, c)| (*w, FieldElement(neg(mul(k, c.0))))),
                    ),
                ),
            ];
        }
    }
    equation
}

/// Choose the side and terms to replace by an auxiliary wire: by default the first
/// `chunk` nonconstant terms of the side with the most terms; with
/// `Reuse::Available`, the terms of the longest existing auxiliary wire that appear,
/// up to a common factor, on any side. Returns the side, the chosen terms and
/// the reused auxiliary id, if any.
fn choose_terms(
    equation: &Equation,
    options: &RewriteOptions,
    auxiliaries: &[Auxiliary],
) -> Result<(usize, LinearCombination, Option<u32>)> {
    // Each side's nonconstant terms, the side with the most terms first.
    let mut sides: Vec<(usize, LinearCombination)> = (0..3)
        .map(|side| {
            let terms = equation[side]
                .iter()
                .filter(|(wire, _)| *wire != 0)
                .copied();
            (side, terms.collect())
        })
        .collect();
    sides.sort_by_key(|(side, terms)| (std::cmp::Reverse(terms.len()), *side));
    let mut side = sides[0].0;
    let mut chosen: LinearCombination = sides[0]
        .1
        .iter()
        .take(options.auxiliary_terms)
        .copied()
        .collect();
    ensure(
        chosen.len() >= 2,
        "cannot split an equation side with fewer than two terms",
    )?;
    let mut existing = None;
    if options.reuse_auxiliaries == Reuse::Available {
        for (candidate, terms) in &sides {
            let coefficients: BTreeMap<_, _> = terms.iter().copied().collect();
            for auxiliary in auxiliaries {
                let Some(first) = coefficients.get(&auxiliary.terms[0].0) else {
                    continue;
                };
                // The auxiliary wire appears on this side as `scale` times its terms.
                let scale = mul(first.0, inverse(auxiliary.terms[0].1 .0));
                let appears = auxiliary
                    .terms
                    .iter()
                    .all(|(wire, k)| coefficients.get(wire).map(|c| c.0) == Some(mul(scale, k.0)));
                if auxiliary.terms.len() < 2 || !appears {
                    continue;
                }
                if existing.is_none() || auxiliary.terms.len() > chosen.len() {
                    side = *candidate;
                    chosen = auxiliary
                        .terms
                        .iter()
                        .map(|(wire, k)| (*wire, FieldElement(mul(scale, k.0))))
                        .collect();
                    existing = Some(auxiliary.id);
                }
            }
        }
    }
    Ok((side, chosen, existing))
}

/// An equation with its wires abstracted: equations with the same gate type differ
/// only in which wires they use, so one constraint can check all of them.
pub struct CanonicalGate {
    /// Serialized `terms`; equal keys mean equal gate types. [`canonical_gate`] keeps the
    /// orientation of `A` and `B` with the smaller key, so the format of this
    /// string is part of the compiler's deterministic output.
    pub key: String,
    /// The equation's wires; position `i` in `terms` stands for `wires[i]`.
    pub wires: Vec<u32>,
    pub terms: GateTerms,
}
/// Canonical gate type of `equation`. Wires are ordered by their coefficients in `A`, `B`
/// and `C`, ties broken by wire id, and `A` and `B` are swapped when that gives
/// a smaller key.
pub fn canonical_gate(equation: &Equation) -> CanonicalGate {
    fn orient(equation: &Equation) -> CanonicalGate {
        let maps = equation
            .clone()
            .map(|lc| lc.into_iter().collect::<BTreeMap<_, _>>());
        let mut wires: Vec<_> = equation
            .iter()
            .flatten()
            .filter(|(w, _)| *w != 0)
            .map(|(w, _)| *w)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        wires.sort_by_key(|w| {
            (
                [
                    maps[0].get(w).copied().unwrap_or(FieldElement(0)),
                    maps[1].get(w).copied().unwrap_or(FieldElement(0)),
                    maps[2].get(w).copied().unwrap_or(FieldElement(0)),
                ],
                *w,
            )
        });
        let ids: BTreeMap<_, _> = wires
            .iter()
            .enumerate()
            .map(|(i, w)| (*w, i as i32))
            .collect();
        let terms = equation.clone().map(|lc| {
            let mut t: Vec<_> = lc
                .iter()
                .map(|(w, k)| (if *w == 0 { -1 } else { ids[w] }, *k))
                .collect();
            t.sort_by_key(|(w, _)| *w);
            t
        });
        CanonicalGate {
            key: serde_json::to_string(&terms).expect("finite terms"),
            wires,
            terms,
        }
    }
    let a = orient(equation);
    let b = orient(&[
        equation[1].clone(),
        equation[0].clone(),
        equation[2].clone(),
    ]);
    if a.key <= b.key {
        a
    } else {
        b
    }
}
