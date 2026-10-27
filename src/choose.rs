//! Completes a layout configuration. A missing `width`, `minRows` or
//! `rowGroupWidth` is chosen by building the PILout of each candidate and
//! estimating how long pil2-proofman takes to prove it; the other settings
//! take their defaults. The checkers do not use this module.
use crate::{
    certificate::{Layout, RewrittenCircuit},
    contract::{MAX_ROWS, MAX_WIDTH, MIN_ROWS},
    ensure, ensure_within_limit,
    layout::{self, LayoutConfig},
    packed, proto as pb,
    r1cs::Circuit,
    rewrite::{self, Reuse, RewriteOptions},
    Context, Error, ErrorKind, Result,
};
use std::collections::BTreeSet;

/// Equation widths the search may try when the configuration leaves `width`
/// out. A narrow width splits long equations into more auxiliary wires but
/// packs them into fewer columns; which is cheaper depends on the circuit.
const WIDTHS: [usize; 6] = [4, 8, 16, 32, 64, 128];
/// The width the search starts from.
const FIRST_WIDTH: usize = 64;

/// The rewritten circuit and layout of `circuit` for `config`, and the
/// settings they were built with, every field filled in.
///
/// Without `width`, the search starts at `FIRST_WIDTH` and moves to narrower
/// widths of `WIDTHS` until the estimate rises; if no narrower width is
/// better, it moves to wider ones instead, again until the estimate rises. Only widths compatible with the
/// given `auxiliaryTerms` and `rowGroupWidth` are tried. For each width,
/// without `minRows`, the candidates are 64, 128, … rows, skipping those whose
/// trace exceeds a size limit and stopping, once one fits, when a candidate no
/// longer fits or the estimate has risen twice in a row; without
/// `rowGroupWidth`, each of `width` and `2 · width` (up to `MAX_WIDTH`) gets its
/// own sweep, and the sweeps run in parallel. The candidate with the lowest
/// estimate wins; ties go to the first tried.
pub fn layout(
    circuit: &Circuit,
    config: &LayoutConfig,
) -> Result<(LayoutConfig, RewrittenCircuit, Layout)> {
    let widths: Vec<usize> = match config.width {
        Some(width) => vec![width],
        None => WIDTHS
            .into_iter()
            .filter(|w| config.auxiliary_terms.is_none_or(|t| t < *w))
            .filter(|w| config.row_group_width.is_none_or(|g| g >= *w))
            .collect(),
    };
    ensure(
        !widths.is_empty(),
        "layout options: no width fits the given auxiliaryTerms and rowGroupWidth; set width",
    )?;
    // Start at FIRST_WIDTH, or the allowed width nearest below it.
    let start = widths.iter().rposition(|w| *w <= FIRST_WIDTH).unwrap_or(0);
    let mut best: Option<Found> = None;
    let mut too_large = None;
    let mut try_width = |width: usize| -> Result<Option<f64>> {
        let (found, error) = with_width(circuit, config, width)?;
        too_large = too_large.take().or(error);
        Ok(found.map(|(cost, settings, rewritten, layout)| {
            if best.as_ref().is_none_or(|(lowest, ..)| cost < *lowest) {
                best = Some((cost, settings, rewritten, layout));
            }
            cost
        }))
    };
    let first = try_width(widths[start])?;
    // Narrower widths while the estimate falls (or no width fits yet).
    let mut previous = first;
    let mut improved = false;
    for &width in widths[..start].iter().rev() {
        let cost = try_width(width)?;
        match (previous, cost) {
            (Some(p), Some(c)) if c > p => break,
            (Some(p), Some(c)) if c < p => improved = true,
            (None, Some(_)) => improved = true,
            _ => {}
        }
        previous = cost.or(previous);
    }
    // Wider widths only if no narrower one did better.
    if !improved {
        let mut previous = first;
        for &width in &widths[start + 1..] {
            let cost = try_width(width)?;
            if let (Some(p), Some(c)) = (previous, cost) {
                if c > p {
                    break;
                }
            }
            previous = cost.or(previous);
        }
    }
    match (best, too_large) {
        (Some((_, settings, rewritten, layout)), _) => Ok((settings, rewritten, layout)),
        (None, Some(error)) => Err(error).context("layout geometry"),
        (None, None) => unreachable!("every width tries at least one candidate"),
    }
}

/// The best layout for one equation width, with its estimated cost (0 when
/// every setting is given, as nothing is compared), and the size limit error
/// of a candidate that did not fit, if any.
fn with_width(
    circuit: &Circuit,
    config: &LayoutConfig,
    width: usize,
) -> Result<(Option<Found>, Option<Error>)> {
    let options = RewriteOptions {
        width,
        auxiliary_terms: config.auxiliary_terms.unwrap_or((width / 2).max(2)),
        reuse_auxiliaries: config.reuse_auxiliaries.unwrap_or(Reuse::Available),
        fold_constants: config.fold_constants.unwrap_or(true),
    };
    let rewritten = rewrite::rewrite(circuit, &options).context("layout options")?;
    let settings = |min_rows, row_group_width| LayoutConfig {
        width: Some(width),
        auxiliary_terms: Some(options.auxiliary_terms),
        reuse_auxiliaries: Some(options.reuse_auxiliaries),
        fold_constants: Some(options.fold_constants),
        min_rows: Some(min_rows),
        row_group_width: Some(row_group_width),
    };
    if let (Some(_), Some(min_rows), Some(row_group_width)) =
        (config.width, config.min_rows, config.row_group_width)
    {
        let layout = layout::pack(&rewritten, width, min_rows, row_group_width)
            .context("layout geometry")?;
        let settings = settings(min_rows, row_group_width);
        return Ok((Some((0.0, settings, rewritten, layout)), None));
    }
    let row_group_widths = match config.row_group_width {
        Some(row_group_width) => vec![row_group_width],
        None => [width, 2 * width]
            .into_iter()
            .filter(|w| *w <= MAX_WIDTH)
            .collect(),
    };
    let row_counts: Vec<usize> = match config.min_rows {
        Some(min_rows) => vec![min_rows],
        None => (MIN_ROWS.trailing_zeros()..=MAX_ROWS.trailing_zeros())
            .map(|bits| 1 << bits)
            .collect(),
    };
    let placed_wires = placed_wires(&rewritten);
    let sweep = |row_group_width: usize| -> Result<Sweep> {
        let mut best: Option<Candidate> = None;
        let mut too_large = None;
        let mut previous = None;
        let mut rises = 0;
        for &min_rows in &row_counts {
            // Each wire in an equation needs a cell of its own, so a trace
            // with fewer cells cannot fit; this skips packing it.
            let packed = ensure_within_limit(
                placed_wires <= min_rows * MAX_WIDTH,
                format_args!(
                    "{placed_wires} wires need more than {MAX_WIDTH} columns of {min_rows} rows"
                ),
            )
            .and_then(|()| layout::pack(&rewritten, width, min_rows, row_group_width));
            let layout = match packed {
                Ok(layout) => layout,
                Err(error) if error.kind() == ErrorKind::Limit => {
                    if previous.is_some() {
                        break;
                    }
                    too_large = Some(error);
                    continue;
                }
                Err(error) => return Err(error).context("layout geometry"),
            };
            let pilout = packed::build_pilout(&rewritten, &layout, &cell_map(&layout))?;
            let cost = estimate(&pilout);
            if best.as_ref().is_none_or(|(lowest, ..)| cost < *lowest) {
                best = Some((cost, settings(min_rows, row_group_width), layout));
            }
            rises = if previous.is_some_and(|p| cost > p) {
                rises + 1
            } else {
                0
            };
            if rises == 2 {
                break;
            }
            previous = Some(cost);
        }
        Ok((best, too_large))
    };
    let sweeps: Vec<Result<Sweep>> = std::thread::scope(|scope| {
        let handles: Vec<_> = row_group_widths
            .iter()
            .map(|&row_group_width| scope.spawn(move || sweep(row_group_width)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a layout sweep panicked"))
            .collect()
    });
    let mut best: Option<Candidate> = None;
    let mut too_large = None;
    for result in sweeps {
        let (candidate, error) = result?;
        too_large = too_large.or(error);
        if let Some(candidate) = candidate {
            if best
                .as_ref()
                .is_none_or(|(lowest, ..)| candidate.0 < *lowest)
            {
                best = Some(candidate);
            }
        }
    }
    Ok((
        best.map(|(cost, settings, layout)| (cost, settings, rewritten, layout)),
        too_large,
    ))
}

/// A layout with its estimated cost and settings.
type Candidate = (f64, LayoutConfig, Layout);
/// A candidate with the rewritten circuit it places.
type Found = (f64, LayoutConfig, RewrittenCircuit, Layout);
/// The best candidate of a sweep, and the size limit error of a candidate
/// that did not fit, if any.
type Sweep = (Option<Candidate>, Option<Error>);

/// Number of wires that need a cell: the public wires and those in an
/// equation or auxiliary definition.
fn placed_wires(rewritten: &RewrittenCircuit) -> usize {
    let mut wires: BTreeSet<u32> = (1..=rewritten.publics).collect();
    for equation in &rewritten.equations {
        wires.extend(equation.iter().flatten().map(|(wire, _)| *wire));
    }
    for auxiliary in &rewritten.auxiliaries {
        wires.insert(auxiliary.id);
        wires.extend(auxiliary.terms.iter().map(|(wire, _)| *wire));
    }
    wires.remove(&0);
    wires.len()
}

/// The wire in each cell of `layout`, row by row, 0 for an unused cell: what
/// the placement check returns for a valid layout.
pub fn cell_map(layout: &Layout) -> Vec<u32> {
    let mut map = vec![0; layout.rows * layout.width];
    for placement in &layout.placements {
        for (wire, column) in placement.wires.iter().zip(&placement.columns) {
            map[placement.row * layout.width + column] = *wire;
        }
    }
    for binding in &layout.public_bindings {
        map[binding.row * layout.width + binding.col] = binding.wire;
    }
    map
}

/// Estimated time, in milliseconds, for pil2-proofman to prove `pilout`,
/// leaving out the FRI query phase. The model was fitted to 90 timed proofs of
/// six circuits (median error 10%); it ranks layouts and is not a measured
/// time. Its inputs are the counts the prover's setup derives from the PILout:
///
/// - extended rows: every packed constraint has degree 3, so the prover
///   doubles the rows;
/// - committed columns: stage one, stage two (3 each, as its values lie in the
///   cubic extension) and the 2 quotient columns (3 each);
/// - fixed columns;
/// - quotient operations: one per expression and two per constraint, less 2
///   (one fewer still without a permutation argument).
pub fn estimate(pilout: &pb::PilOut) -> f64 {
    let air = &pilout.air_groups[0].airs[0];
    let extended = 2.0 * f64::from(air.num_rows.unwrap_or(0)) / 1e6;
    let committed = f64::from(air.stage_widths[0] + 3 * air.stage_widths[1] + 6);
    let fixed = air.fixed_cols.len() as f64;
    let operations = (air.expressions.len() + 2 * air.constraints.len()).saturating_sub(2) as f64;
    extended * (0.229 * operations + 10.6 * committed + 1.38 * fixed + 532.0)
        + 4.5
        + 0.0134 * (committed + fixed)
}
