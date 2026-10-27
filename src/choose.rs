//! Completes a layout configuration. Settings left out of a [`LayoutConfig`]
//! take their defaults; a missing `minRows` or `rowGroupWidth` is chosen by
//! building the PILout of each candidate and estimating how long pil2-proofman
//! takes to prove it. The checkers do not use this module.
use crate::{
    certificate::{Layout, RewrittenCircuit},
    contract::{MAX_ROWS, MAX_WIDTH, MIN_ROWS},
    ensure_within_limit,
    layout::{self, LayoutConfig},
    packed, proto as pb,
    r1cs::Circuit,
    rewrite::{self, Reuse, RewriteOptions},
    Context, Error, ErrorKind, Result,
};
use std::collections::BTreeSet;

/// Equation width when the configuration leaves it out.
const DEFAULT_WIDTH: usize = 64;

/// The rewritten circuit and layout of `circuit` for `config`, and the
/// settings they were built with, every field filled in.
///
/// Without `minRows`, the candidates are 64, 128, … rows, skipping those whose
/// trace exceeds a size limit and stopping, once one fits, when a candidate
/// no longer fits or the estimate has risen twice in a row. Without
/// `rowGroupWidth`, each of `width` and `2 · width` (up to `MAX_WIDTH`) gets
/// its own sweep, and the sweeps run in parallel. The candidate with the
/// lowest estimate wins; ties go to the smaller row group width, then to the
/// fewer rows.
pub fn layout(
    circuit: &Circuit,
    config: &LayoutConfig,
) -> Result<(LayoutConfig, RewrittenCircuit, Layout)> {
    let width = config.width.unwrap_or(DEFAULT_WIDTH);
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
    if let (Some(min_rows), Some(row_group_width)) = (config.min_rows, config.row_group_width) {
        let layout = layout::pack(&rewritten, width, min_rows, row_group_width)
            .context("layout geometry")?;
        return Ok((settings(min_rows, row_group_width), rewritten, layout));
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
    match (best, too_large) {
        (Some((_, settings, layout)), _) => Ok((settings, rewritten, layout)),
        (None, Some(error)) => Err(error).context("layout geometry"),
        (None, None) => unreachable!("every sweep tries at least one candidate"),
    }
}

/// A layout with its estimated cost and settings.
type Candidate = (f64, LayoutConfig, Layout);
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
