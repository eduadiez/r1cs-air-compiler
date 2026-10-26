//! Rewrites source equations so that each one references at most `width`
//! distinct wires, by introducing helper wires constrained to equal linear
//! combinations of other wires. Also computes the canonical shape of an equation.
use crate::{
    certificate::{Helper, Network, ShapeTerms},
    contract::{MAX_WIDTH, MIN_WIDTH},
    ensure,
    field::{add, inverse, mul, neg, FieldElement},
    r1cs::{Circuit, Equation, LinearCombination},
    Result, P,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// When an existing helper may stand in for terms of a later equation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Reuse {
    /// Every rewrite creates a new helper.
    None,
    /// Reuse a helper when the chosen terms are a multiple of its terms.
    Prefix,
    /// Like `Prefix`, and also search all sides of the equation for the
    /// longest helper whose terms appear there up to a common factor.
    Available,
}
/// Settings for rewriting source equations with helpers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionOptions {
    /// Maximum number of distinct nonconstant wires per equation (`3..=1024`).
    pub width: usize,
    /// Number of terms replaced by one new helper (`2..width`).
    pub chunk: usize,
    /// When an existing helper may be reused.
    pub reuse: Reuse,
    /// If one factor of `A · B = C` is a constant `k`, rewrite the equation as
    /// `0 = C - k · (other factor)` before splitting.
    pub fold: bool,
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
/// The linear equation `0 · 0 = terms - helper` that defines a helper.
fn helper_equation(helper: &Helper) -> Equation {
    let terms = helper.terms.iter().copied();
    [
        vec![],
        vec![],
        canonical(terms.chain([(helper.id, FieldElement(P - 1))])),
    ]
}
/// Helper equations followed by the converted source equations: the order in
/// which [`Placement::index`](crate::certificate::Placement::index) numbers them.
pub fn equations(network: &Network) -> Vec<Equation> {
    network
        .helpers
        .iter()
        .map(helper_equation)
        .chain(network.converted.iter().cloned())
        .collect()
}

/// Rewrite each source equation until it references at most `options.width`
/// distinct wires. Each step replaces some terms `Σ kᵢ·wᵢ` of one side by
/// `k₀·h`, where `h` is a helper constrained to `Σ (kᵢ/k₀)·wᵢ`, so its first
/// coefficient is 1. Helpers only reference earlier wires.
pub fn convert(source: &Circuit, options: &ConversionOptions) -> Result<Network> {
    ensure(
        (MIN_WIDTH..=MAX_WIDTH).contains(&options.width)
            && (2..options.width).contains(&options.chunk),
        format_args!(
            "options.width must be {MIN_WIDTH}..={MAX_WIDTH} and options.chunk 2..width (width={}, chunk={})",
            options.width, options.chunk
        ),
    )?;
    let mut network = Network {
        original_wires: source.wires,
        wires: source.wires,
        publics: source.publics,
        helpers: vec![],
        converted: vec![],
    };
    // Helper id for each helper's terms, used to reuse helpers across equations.
    let mut reusable = BTreeMap::new();
    for mut equation in source_equations(source) {
        if options.fold {
            equation = fold_constant_factor(equation);
        }
        while equation_width(&equation) > options.width {
            let (side, chosen, existing) = choose_chunk(&equation, options, &network.helpers)?;
            let scale = chosen[0].1 .0;
            let inverse_scale = inverse(scale);
            let normalized: LinearCombination = chosen
                .iter()
                .map(|(wire, k)| (*wire, FieldElement(mul(k.0, inverse_scale))))
                .collect();
            let reused = existing.or_else(|| {
                if options.reuse == Reuse::None {
                    None
                } else {
                    reusable.get(&normalized).copied()
                }
            });
            let id = match reused {
                Some(id) => id,
                None => {
                    // Each new helper removes at least one term, so the helper
                    // count is bounded by the input size and the id fits in u32.
                    let id = network.original_wires + network.helpers.len() as u32;
                    ensure(
                        normalized.iter().all(|(wire, _)| *wire < id),
                        "helper would refer to itself or a later wire",
                    )?;
                    reusable.insert(normalized.clone(), id);
                    network.helpers.push(Helper {
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
                "a helper must reduce the number of terms",
            )?;
            equation[side] = next;
        }
        network.converted.push(equation);
    }
    network.wires += network.helpers.len() as u32;
    Ok(network)
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

/// Choose the side and terms to replace by a helper: by default the first
/// `chunk` nonconstant terms of the side with the most terms; with
/// `Reuse::Available`, the terms of the longest existing helper that appear,
/// up to a common factor, on any side. Returns the side, the chosen terms and
/// the reused helper id, if any.
fn choose_chunk(
    equation: &Equation,
    options: &ConversionOptions,
    helpers: &[Helper],
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
    let mut chosen: LinearCombination = sides[0].1.iter().take(options.chunk).copied().collect();
    ensure(
        chosen.len() >= 2,
        "cannot split an equation side with fewer than two terms",
    )?;
    let mut existing = None;
    if options.reuse == Reuse::Available {
        for (candidate, terms) in &sides {
            let coefficients: BTreeMap<_, _> = terms.iter().copied().collect();
            for helper in helpers {
                let Some(first) = coefficients.get(&helper.terms[0].0) else {
                    continue;
                };
                // The helper appears on this side as `scale` times its terms.
                let scale = mul(first.0, inverse(helper.terms[0].1 .0));
                let appears = helper
                    .terms
                    .iter()
                    .all(|(wire, k)| coefficients.get(wire).map(|c| c.0) == Some(mul(scale, k.0)));
                if helper.terms.len() < 2 || !appears {
                    continue;
                }
                if existing.is_none() || helper.terms.len() > chosen.len() {
                    side = *candidate;
                    chosen = helper
                        .terms
                        .iter()
                        .map(|(wire, k)| (*wire, FieldElement(mul(scale, k.0))))
                        .collect();
                    existing = Some(helper.id);
                }
            }
        }
    }
    Ok((side, chosen, existing))
}

/// An equation with its wires abstracted: equations with the same shape differ
/// only in which wires they use, so one constraint can check all of them.
pub struct Shape {
    /// Serialized `terms`; equal keys mean equal shapes. [`shape`] keeps the
    /// orientation of `A` and `B` with the smaller key, so the format of this
    /// string is part of the compiler's deterministic output.
    pub key: String,
    /// The equation's wires; position `i` in `terms` stands for `wires[i]`.
    pub wires: Vec<u32>,
    pub terms: ShapeTerms,
}
/// Canonical shape of `equation`. Wires are ordered by their coefficients in `A`, `B`
/// and `C`, ties broken by wire id, and `A` and `B` are swapped when that gives
/// a smaller key.
pub fn shape(equation: &Equation) -> Shape {
    fn orient(equation: &Equation) -> Shape {
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
        Shape {
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
