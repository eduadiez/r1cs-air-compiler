//! Compiles a circuit to the packed PILout: helpers from `network`, placement
//! from `layout`, one constraint per slot, public bindings, and, when a wire
//! occupies several cells, a copy argument that makes those cells equal.
use crate::{
    api::PackedArtifact,
    certificate::{Layout, Network, PackedCertificate},
    contract::{
        label, ALPHA, CERTIFICATE_VERSION, FIRST_COLUMN, GAMMA, MAX_DEGREE, PACKED_RELATION,
        PRODUCT_COLUMN, PRODUCT_COLUMN_NAME, PRODUCT_HINT, RATIOS_SYMBOL, RATIO_HINT, STAGE_TWO,
        ZERO_COLUMN_NAME,
    },
    dag::{challenge, constant, operand, stage_two, trace, Op},
    emit::{hint, symbol, Builder},
    ensure,
    field::{mul, pow, FieldElement, COSET, GENERATOR},
    layout::{self, LayoutConfig},
    network,
    placement_check::{self, Checked},
    proto as pb,
    r1cs::{self, Circuit},
    sha256_hex, Context, Result, P,
};
use pb::{operand::Operand, SymbolType};
use std::collections::{BTreeMap, BTreeSet};

/// References to the copy-argument fixed columns.
struct CopyColumns {
    /// `ω^r`.
    id: pb::Operand,
    /// The columns in the copy argument, those holding a cell of a wire with
    /// several cells, in increasing order.
    columns: Vec<usize>,
    /// `S{c}` for each of those columns `c`.
    permuted: Vec<pb::Operand>,
    first: pb::Operand,
}
/// Column values that are 1 on the rows `on` and 0 elsewhere.
fn indicator(rows: usize, on: impl IntoIterator<Item = usize>) -> Vec<u64> {
    let on: BTreeSet<_> = on.into_iter().collect();
    (0..rows).map(|r| u64::from(on.contains(&r))).collect()
}
/// Packed-only parts of the PILout.
impl Builder {
    /// Shape terms with shape position `i` read from stage-one column `columns[i]`.
    fn shape_linear_combination(
        &mut self,
        terms: &[(i32, FieldElement)],
        columns: &[usize],
    ) -> pb::Operand {
        let mut result = constant(0);
        for (position, coefficient) in terms {
            let value = if *position < 0 {
                constant(1)
            } else {
                trace(columns[*position as usize] as u32)
            };
            let term = self.expression(Op::Mul, constant(coefficient.0), value);
            result = self.expression(Op::Add, result, term);
        }
        result
    }
    /// Add one selector column `E{id}` per selector, and for each slot the
    /// constraint `E · (A·B - C) = 0`.
    fn constrain_slots(&mut self, layout: &Layout) {
        let selectors: Vec<_> = layout
            .selectors
            .iter()
            .map(|slot| {
                let values = indicator(layout.rows, slot.rows.iter().copied());
                self.fixed(&format!("E{}", slot.id), values)
            })
            .collect();
        for slot in &layout.slots {
            let terms = &layout.groups[slot.shape].terms;
            let left = self.shape_linear_combination(&terms[0], &slot.columns);
            let right = self.shape_linear_combination(&terms[1], &slot.columns);
            let output = self.shape_linear_combination(&terms[2], &slot.columns);
            let product = self.expression(Op::Mul, left, right);
            let residual = self.expression(Op::Sub, product, output);
            let active = self.expression(Op::Mul, selectors[slot.selector].clone(), residual);
            self.constrain(active, label::PACKED_EQUATION.into());
        }
    }
    /// Bind public values to their cells (SPEC.md, "Public values"). The `q`
    /// rows holding public cells are numbered `i = 0..q` and split into groups
    /// of `s = ceil(sqrt(q))`: `PH{h}` is 1 on rows with `i / s = h` and `PL{l}`
    /// on rows with `i % s = l`, so `PH · PL` selects one row with about
    /// `2·sqrt(q)` columns. For each column `c` holding public cells, `PA{c}` is 1 on those
    /// rows, and the constraint is `PA · a[c] = Σ PH · PL · public`.
    fn bind_publics(&mut self, layout: &Layout) {
        let public_rows: BTreeSet<_> = layout
            .public_bindings
            .iter()
            .map(|binding| binding.row)
            .collect();
        if public_rows.is_empty() {
            return;
        }
        let index: BTreeMap<_, _> = public_rows
            .iter()
            .enumerate()
            .map(|(position, row)| (*row, position))
            .collect();
        let side = ceil_sqrt(public_rows.len());
        let mut group_column = |name: String, select: &dyn Fn(usize) -> bool| {
            let rows = index
                .iter()
                .filter(|(_, position)| select(**position))
                .map(|(row, _)| *row);
            let values = indicator(layout.rows, rows);
            self.fixed(&name, values)
        };
        let high: Vec<_> = (0..public_rows.len().div_ceil(side))
            .map(|group| group_column(format!("PH{group}"), &|position| position / side == group))
            .collect();
        let low: Vec<_> = (0..side)
            .map(|member| {
                group_column(format!("PL{member}"), &|position| position % side == member)
            })
            .collect();
        let mut columns = vec![];
        for binding in &layout.public_bindings {
            if !columns.contains(&binding.col) {
                columns.push(binding.col);
            }
        }
        for col in columns {
            let bindings: Vec<_> = layout
                .public_bindings
                .iter()
                .filter(|binding| binding.col == col)
                .collect();
            let pa = self.fixed(
                &format!("PA{col}"),
                indicator(layout.rows, bindings.iter().map(|binding| binding.row)),
            );
            let terms = bindings
                .iter()
                .map(|binding| {
                    let position = index[&binding.row];
                    let row = self.expression(
                        Op::Mul,
                        high[position / side].clone(),
                        low[position % side].clone(),
                    );
                    let public = operand(Operand::PublicValue(pb::operand::PublicValue {
                        idx: binding.public,
                    }));
                    self.expression(Op::Mul, row, public)
                })
                .collect();
            let bound = self.balanced_sum(terms);
            let cell = self.expression(Op::Mul, pa, trace(col as u32));
            let residual = self.expression(Op::Sub, cell, bound);
            self.constrain(residual, label::PACKED_PUBLIC.into());
        }
    }
    /// Sum `terms` as a balanced tree, so expression depth grows only
    /// logarithmically with the number of terms.
    fn balanced_sum(&mut self, mut terms: Vec<pb::Operand>) -> pb::Operand {
        while terms.len() > 1 {
            terms = terms
                .chunks(2)
                .map(|pair| match pair {
                    [a, b] => self.expression(Op::Add, a.clone(), b.clone()),
                    [a] => a.clone(),
                    _ => unreachable!("chunks of two"),
                })
                .collect();
        }
        terms.pop().unwrap_or_else(|| constant(0))
    }
    /// Add the copy-argument fixed columns `ID`, `S0..S{width-1}` and `FIRST`.
    fn copy_columns(&mut self, layout: &Layout, map: &[u32]) -> Result<CopyColumns> {
        let (labels, permuted) = labels(map, layout.rows, layout.width)?;
        let id = self.fixed(
            "ID",
            (0..layout.rows)
                .map(|row| labels[row * layout.width])
                .collect(),
        );
        let columns = copy_columns(map, layout.width);
        let permuted = columns
            .iter()
            .map(|column| {
                let values = (0..layout.rows)
                    .map(|row| permuted[row * layout.width + column])
                    .collect();
                self.fixed(&format!("S{column}"), values)
            })
            .collect();
        let first = self.fixed(FIRST_COLUMN, indicator(layout.rows, [0]));
        Ok(CopyColumns {
            id,
            columns,
            permuted,
            first,
        })
    }
    /// Copy argument (SPEC.md, "Copy argument"): ratio columns of degree at
    /// most `MAX_DEGREE`, the running product `z[r] · den[r] = z[r-1] · num[r]` (cyclic,
    /// so row 0 reads the last row) and `LAST · (z - 1) = 0`.
    fn constrain_copies(&mut self, columns: CopyColumns) {
        // No simplification: the checker compares these constraints operation
        // by operation.
        self.without_simplification(|builder| builder.constrain_copies_exactly(columns));
    }
    fn constrain_copies_exactly(&mut self, columns: CopyColumns) {
        let (numerators, denominators) =
            self.copy_factors(&columns.id, &columns.columns, columns.permuted);
        let (numerator, denominator, ratios) =
            self.constrain_ratios(numerators.len(), &numerators, &denominators);
        self.air.stage_widths[1] += ratios;
        if ratios > 0 {
            self.symbols.push(symbol(
                RATIOS_SYMBOL,
                SymbolType::WitnessCol,
                1,
                Some(STAGE_TWO),
                vec![ratios],
            ));
        }
        let z = stage_two(PRODUCT_COLUMN);
        let previous = operand(Operand::WitnessCol(pb::operand::WitnessCol {
            stage: STAGE_TWO,
            col_idx: PRODUCT_COLUMN,
            row_offset: -1,
        }));
        // z is 1 on the last row, so row 0 reads a previous value of 1, as if
        // the product started at row 0.
        let left = self.expression(Op::Mul, z.clone(), denominator.clone());
        let right = self.expression(Op::Mul, previous, numerator.clone());
        let recurrence = self.expression(Op::Sub, left, right);
        self.constrain(recurrence, label::COPY_RECURRENCE.into());
        let Some(Operand::FixedCol(first)) = columns.first.operand else {
            unreachable!("fixed() returns a fixed column")
        };
        let last = operand(Operand::FixedCol(pb::operand::FixedCol {
            idx: first.idx,
            row_offset: 1,
        }));
        let end = self.expression(Op::Sub, z.clone(), constant(1));
        let closure = self.expression(Op::Mul, last, end);
        self.constrain(closure, label::COPY_CLOSURE.into());
        let reference = self.expression(Op::Add, z, constant(0));
        self.hints.push(hint(
            PRODUCT_HINT,
            vec![
                ("reference", reference),
                ("numerator_air", numerator),
                ("denominator_air", denominator),
                ("numerator_direct", constant(1)),
                ("denominator_direct", constant(1)),
                ("result", constant(1)),
            ],
        ));
    }
    /// For each column `c` with value `w`: the numerator factor `w + α·S_c + γ`,
    /// with the permuted label column `S_c`, and the denominator factor
    /// `w + α·COSET^c·ID + γ`, with the cell's own label.
    fn copy_factors(
        &mut self,
        id: &pb::Operand,
        columns: &[usize],
        permuted: Vec<pb::Operand>,
    ) -> (Vec<pb::Operand>, Vec<pb::Operand>) {
        let alpha = challenge(ALPHA);
        let gamma = challenge(GAMMA);
        let (mut numerators, mut denominators) = (vec![], vec![]);
        for (column, label) in columns.iter().zip(permuted) {
            let shift = pow(COSET, *column as u64);
            let own_label = self.expression(Op::Mul, constant(shift), id.clone());
            let own_label = self.expression(Op::Mul, alpha.clone(), own_label);
            let own = self.expression(Op::Add, trace(*column as u32), own_label);
            let own = self.expression(Op::Add, own, gamma.clone());
            let permuted_label = self.expression(Op::Mul, alpha.clone(), label);
            let permuted = self.expression(Op::Add, trace(*column as u32), permuted_label);
            let permuted = self.expression(Op::Add, permuted, gamma.clone());
            numerators.push(permuted);
            denominators.push(own);
        }
        (numerators, denominators)
    }
    /// Constrain stage-two ratio columns `ratio_i · den_i = num_i`, each of
    /// degree at most `MAX_DEGREE`, until the remaining factors fit in the
    /// running product: at most `MAX_DEGREE - 1` numerator factors (counting
    /// the last ratio) and as many denominator factors. Returns the remaining
    /// numerator and denominator products and the number of ratio columns.
    fn constrain_ratios(
        &mut self,
        width: usize,
        numerators: &[pb::Operand],
        denominators: &[pb::Operand],
    ) -> (pb::Operand, pb::Operand, u32) {
        // Next unused numerator and denominator factor.
        let (mut next_num, mut next_den, mut ratios) = (0, 0, 0);
        let mut numerator = constant(1);
        if width >= MAX_DEGREE {
            while ratios == 0
                || width - next_num + 1 >= MAX_DEGREE
                || width - next_den >= MAX_DEGREE
            {
                // The first ratio takes `MAX_DEGREE` numerators; later ones one
                // fewer, plus the previous ratio.
                let take = (MAX_DEGREE - usize::from(ratios > 0)).min(width - next_num);
                numerator = self.multiply(numerator, &numerators[next_num..next_num + take]);
                next_num += take;
                let take = (MAX_DEGREE - 1).min(width - next_den);
                let denominator =
                    self.multiply(constant(1), &denominators[next_den..next_den + take]);
                next_den += take;
                ratios += 1;
                let ratio = stage_two(ratios);
                let left = self.expression(Op::Mul, ratio.clone(), denominator.clone());
                let residual = self.expression(Op::Sub, left, numerator.clone());
                self.constrain(residual, label::COPY_RATIO.into());
                self.hints.push(hint(
                    RATIO_HINT,
                    vec![
                        ("reference", ratio.clone()),
                        ("numerator", numerator),
                        ("denominator", denominator),
                    ],
                ));
                numerator = ratio;
            }
        }
        let numerator = self.multiply(numerator, &numerators[next_num..]);
        let denominator = self.multiply(constant(1), &denominators[next_den..]);
        (numerator, denominator, ratios)
    }
    /// `start · f₀ · f₁ · …`, multiplied left to right.
    fn multiply(&mut self, start: pb::Operand, factors: &[pb::Operand]) -> pb::Operand {
        factors.iter().fold(start, |product, factor| {
            self.expression(Op::Mul, product, factor.clone())
        })
    }
}
/// Smallest `s` with `s * s >= n`.
fn ceil_sqrt(n: usize) -> usize {
    let s = n.isqrt();
    if s * s < n {
        s + 1
    } else {
        s
    }
}

/// The columns holding a cell of a wire that occupies several cells, in
/// increasing order; only they take part in the copy argument, since in every
/// other column each cell keeps its own label.
fn copy_columns(map: &[u32], width: usize) -> Vec<usize> {
    let mut cells = BTreeMap::<u32, usize>::new();
    for wire in map.iter().filter(|wire| **wire != 0) {
        *cells.entry(*wire).or_default() += 1;
    }
    let columns: BTreeSet<usize> = map
        .iter()
        .enumerate()
        .filter(|(_, wire)| cells.get(wire).is_some_and(|count| *count > 1))
        .map(|(cell, _)| cell % width)
        .collect();
    columns.into_iter().collect()
}

/// Copy-argument labels in row-major cell order. Each cell's own label is
/// `ω^r · COSET^c`. In the permuted labels, each cell of a wire holds the label
/// of the wire's previous cell, cyclically; an unused cell or a wire with one
/// cell keeps its own label.
fn labels(map: &[u32], rows: usize, width: usize) -> Result<(Vec<u64>, Vec<u64>)> {
    let omega = pow(GENERATOR, (P - 1) / rows as u64);
    ensure(
        pow(omega, rows as u64) == 1 && pow(omega, rows as u64 / 2) != 1,
        "no row domain of this size",
    )?;
    let mut labels = vec![];
    let mut row_label = 1;
    for _ in 0..rows {
        let mut shift = 1;
        for _ in 0..width {
            labels.push(mul(row_label, shift));
            shift = mul(shift, COSET);
        }
        row_label = mul(row_label, omega);
    }
    ensure(
        labels.iter().collect::<BTreeSet<_>>().len() == labels.len(),
        "copy labels must be distinct",
    )?;
    // Swapping each later cell of a wire with its first cell leaves every
    // cell holding the label of the wire's previous cell, cyclically.
    let mut permuted = labels.clone();
    let mut first = BTreeMap::new();
    for (cell, wire) in map.iter().enumerate() {
        if *wire == 0 {
            continue;
        }
        if let Some(first_cell) = first.get(wire) {
            permuted.swap(cell, *first_cell);
        } else {
            first.insert(*wire, cell);
        }
    }
    Ok((labels, permuted))
}

/// Build the helper network and layout and check the placement.
pub fn plan(circuit: &Circuit, config: &LayoutConfig) -> Result<(Network, Layout, Checked)> {
    let network = network::convert(circuit, &config.options).context("layout options")?;
    let layout = layout::pack(&network, config).context("layout geometry")?;
    let checked =
        placement_check::check(circuit, &network, &layout).context("layout validation")?;
    Ok((network, layout, checked))
}

/// Build the helper network and layout, check the placement, and generate the
/// PILout and certificate. The caller runs the PILout checker.
pub fn compile(source: &[u8], config: &LayoutConfig) -> Result<PackedArtifact> {
    let circuit = r1cs::read(source).context("packed source")?;
    let (network, layout, checked) = plan(&circuit, config)?;
    let pilout = build_pilout(&network, &layout, &checked.map)?;
    let certificate = PackedCertificate {
        version: CERTIFICATE_VERSION,
        relation: PACKED_RELATION.into(),
        source_sha256: sha256_hex(source),
        pilout_sha256: sha256_hex(&pilout),
        network,
        layout,
    };
    Ok(PackedArtifact {
        pilout,
        map: checked.map,
        certificate,
    })
}

/// Generate the PILout. Fixed columns come in this order: `ID`, `S0..S{width-1}`
/// and `FIRST` (only with a copy argument), the selectors `E*`, then `PH*`,
/// `PL*` and `PA*`.
fn build_pilout(network: &Network, layout: &Layout, map: &[u32]) -> Result<Vec<u8>> {
    let product = if layout.requires_copy {
        PRODUCT_COLUMN_NAME
    } else {
        ZERO_COLUMN_NAME
    };
    let mut builder = Builder::new(layout.rows as u32, layout.width as u32, product, true);
    let copy = if layout.requires_copy {
        Some(builder.copy_columns(layout, map)?)
    } else {
        builder.constrain_zero_column();
        None
    };
    builder.constrain_slots(layout);
    builder.bind_publics(layout);
    if let Some(columns) = copy {
        builder.constrain_copies(columns);
    }
    Ok(builder.finish(network.publics))
}
