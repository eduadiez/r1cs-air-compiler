//! Checker for packed artifacts. It rebuilds what the PILout must contain from
//! the source and the certificate and compares it with the actual PILout. It
//! does not call the compiler.
//!
//! Steps, in order:
//! 1. certificate version, relation and hashes, then the placement check;
//! 2. PILout field, names and geometry;
//! 3. every fixed column, rebuilt from the layout;
//! 4. symbol table;
//! 5. gate and public-binding constraints, compared as exact polynomials;
//! 6. permutation-argument constraints and all hints, compared operation by operation.
use crate::{
    certificate::{Layout, PackedCertificate},
    contract::{
        label, AIR_NAME, ALPHA, CHALLENGE_NAMES, FIRST_COLUMN, GAMMA, MAX_DEGREE, PACKED_RELATION,
        PRODUCT_COLUMN, PRODUCT_COLUMN_NAME, PRODUCT_HINT, PUBLICS_SYMBOL, RATIOS_SYMBOL,
        RATIO_HINT, STAGE_ONE, STAGE_TWO, SUM_HINT, TRACE_SYMBOL, ZERO_COLUMN_NAME,
    },
    ensure,
    field::{mul, pow, FieldElement, COSET, GENERATOR},
    pilout_check::{self, binary_operation, scalar, BinaryOp, Binding, ExpectedSymbol, Geometry},
    placement_check,
    polynomial::{constant, difference, product, sum, variable, Interpreter, Polynomial, Var},
    proto as pb, r1cs, Context, Result, P,
};
use pb::{operand::Operand, SymbolType};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Successful independent validation of a packed artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct PackedCheckReport {
    /// Always true: a failed check returns an error instead.
    pub accepted: bool,
    /// Name of the checked relation.
    pub relation: String,
    /// Number of equations in the source circuit.
    pub source_equations: usize,
    /// Number of auxiliary wires.
    pub auxiliaries: usize,
    /// Number of trace rows.
    pub rows: usize,
    /// Number of stage-one columns.
    pub width: usize,
    /// Whether the PILout contains a permutation argument.
    pub requires_permutation: bool,
    /// Always false: the check is not a formal proof.
    pub formally_verified: bool,
}

/// Check packed PILout bytes against their source and certificate.
pub fn check(
    source: &[u8],
    pilout_bytes: &[u8],
    certificate: &PackedCertificate,
) -> Result<PackedCheckReport> {
    let binding = Binding {
        version: certificate.version,
        relation: &certificate.relation,
        source_sha256: &certificate.source_sha256,
        pilout_sha256: &certificate.pilout_sha256,
    };
    pilout_check::check_binding(&binding, PACKED_RELATION, source, pilout_bytes)?;
    let circuit = r1cs::read(source)?;
    let checked = placement_check::check(
        &circuit,
        &certificate.rewritten_circuit,
        &certificate.layout,
    )?;
    let layout = &certificate.layout;
    let permutation = PermutationArgument::new(&checked.occurrences);

    let pilout = pilout_check::decode(pilout_bytes)?;
    let air = pilout_check::check_envelope(
        &pilout,
        &Geometry {
            rows: layout.rows as u32,
            stage_widths: [layout.width as u32, permutation.stage_two_width()],
            publics: circuit.publics,
        },
    )?;

    let mut fixed = FixedColumns::new(air, layout.rows);
    let permutation_columns = if layout.requires_permutation {
        fixed.add_permutation_columns(layout, &checked.occurrences, &permutation)?
    } else {
        0
    };
    for s in &layout.selectors {
        fixed.add(format!("E{}", s.id), &indicator(layout.rows, &s.rows))?;
    }
    let publics = PublicSelectors::add(&mut fixed, layout)?;
    ensure(
        fixed.names.len() == air.fixed_cols.len(),
        "unexpected fixed column",
    )?;

    pilout_check::check_symbols(
        &pilout,
        expected_symbols(layout, &fixed.names, circuit.publics, &permutation),
    )?;

    let main = main_polynomials(layout, permutation_columns, &publics)?;
    let roots =
        pilout_check::constraint_roots(air, &constraint_labels(layout, &publics, &permutation))?;
    check_main_constraints(air, layout, circuit.publics, &roots[..main.len()], &main)?;

    let mut graph = ExpressionGraph::default();
    let expression_ids = graph.import(
        air,
        layout,
        &permutation,
        circuit.publics,
        fixed.names.len(),
    )?;
    let expected_hints = if layout.requires_permutation {
        check_permutation_constraints(
            &mut graph,
            &expression_ids,
            &permutation,
            &roots[main.len()..],
        )?
    } else {
        vec![zero_hint(&mut graph)]
    };
    check_hints(
        &pilout,
        &expected_hints,
        &mut graph,
        &OperandContext {
            expression_ids: &expression_ids,
            layout,
            permutation: &permutation,
            publics: circuit.publics,
            fixed_columns: fixed.names.len(),
        },
    )?;

    Ok(PackedCheckReport {
        accepted: true,
        relation: certificate.relation.clone(),
        source_equations: circuit.equations.len(),
        auxiliaries: certificate.rewritten_circuit.auxiliaries.len(),
        rows: layout.rows,
        width: layout.width,
        requires_permutation: layout.requires_permutation,
        formally_verified: false,
    })
}

/// Labels of all constraints, in PILout order.
fn constraint_labels(
    layout: &Layout,
    publics: &PublicSelectors,
    permutation: &PermutationArgument,
) -> Vec<String> {
    let mut labels = vec![];
    if !layout.requires_permutation {
        labels.push(label::ZERO_COLUMN);
    }
    labels.extend(layout.gates.iter().map(|_| label::PACKED_GATE));
    labels.extend(publics.columns.iter().map(|_| label::PACKED_PUBLIC));
    if layout.requires_permutation {
        labels.extend((0..permutation.ratios()).map(|_| label::PERMUTATION_RATIO));
        labels.extend([label::PERMUTATION_RECURRENCE, label::PERMUTATION_CLOSURE]);
    }
    labels.into_iter().map(String::from).collect()
}

/// The columns in the permutation argument (SPEC.md, "Permutation argument"): those holding
/// a cell of a wire with several cells, in increasing order. Empty without a
/// permutation argument.
struct PermutationArgument {
    columns: Vec<usize>,
}
impl PermutationArgument {
    fn new(occurrences: &BTreeMap<u32, Vec<(usize, usize)>>) -> Self {
        let columns: BTreeSet<usize> = occurrences
            .values()
            .filter(|cells| cells.len() > 1)
            .flatten()
            .map(|(_, column)| *column)
            .collect();
        PermutationArgument {
            columns: columns.into_iter().collect(),
        }
    }
    /// Number of permutation ratio columns, from the formula in SPEC.md rather than
    /// the compiler's loop.
    fn ratios(&self) -> u32 {
        (self.columns.len().saturating_sub(1) / (MAX_DEGREE - 1)) as u32
    }
    /// Stage-two width: the running product (or `stage2_zero`) plus the ratio columns.
    fn stage_two_width(&self) -> u32 {
        1 + self.ratios()
    }
    /// Index of the fixed column `FIRST`, after `ID` and one `S` column per
    /// permutation-argument column.
    fn first_column(&self) -> u32 {
        self.columns.len() as u32 + 1
    }
}

/// Column values that are 1 on the rows `on` and 0 elsewhere.
fn indicator(rows: usize, on: &[usize]) -> Vec<u64> {
    let on: BTreeSet<_> = on.iter().copied().collect();
    (0..rows).map(|r| u64::from(on.contains(&r))).collect()
}

/// Expected fixed columns, compared against the actual PILout as they are added.
struct FixedColumns<'a> {
    air: &'a pb::Air,
    rows: usize,
    names: Vec<String>,
}
impl<'a> FixedColumns<'a> {
    fn new(air: &'a pb::Air, rows: usize) -> Self {
        Self {
            air,
            rows,
            names: vec![],
        }
    }
    /// Require the next actual fixed column to hold exactly `values`; returns its index.
    fn add(&mut self, name: String, values: &[u64]) -> Result<u32> {
        let id = self.names.len();
        ensure(
            id < self.air.fixed_cols.len(),
            format_args!("fixed column {name} is missing"),
        )?;
        ensure(
            self.air.fixed_cols[id].values.len() == self.rows,
            format_args!("fixed column {name} must have one value per row"),
        )?;
        for (row, (actual, expected)) in self.air.fixed_cols[id]
            .values
            .iter()
            .zip(values)
            .enumerate()
        {
            ensure(
                scalar(actual)? == *expected,
                format_args!("fixed column {name} differs at row {row}"),
            )?;
        }
        self.names.push(name);
        Ok(id as u32)
    }
    /// The permutation-argument columns: `ID` (the row domain `ω^r`), one permuted
    /// label column `S{col}` per permutation column in increasing order, and `FIRST`.
    /// Returns the number of columns added.
    fn add_permutation_columns(
        &mut self,
        layout: &Layout,
        occurrences: &BTreeMap<u32, Vec<(usize, usize)>>,
        permutation: &PermutationArgument,
    ) -> Result<usize> {
        let (n, width) = (layout.rows, layout.width);
        let omega = pow(GENERATOR, (P - 1) / n as u64);
        let domain: Vec<_> = (0..n).map(|r| pow(omega, r as u64)).collect();
        let labels: Vec<_> = (0..n)
            .flat_map(|r| (0..width).map(move |col| (r, col)))
            .map(|(r, col)| mul(domain[r], pow(COSET, col as u64)))
            .collect();
        ensure(
            labels.iter().collect::<BTreeSet<_>>().len() == labels.len(),
            "permutation labels must be distinct",
        )?;
        // The cells of each wire form one cycle: each holds the label of the
        // previous cell. Unused cells keep their own label.
        let mut connections = labels.clone();
        for locations in occurrences.values() {
            let mut cells: Vec<_> = locations.iter().map(|(r, col)| r * width + col).collect();
            cells.sort_unstable();
            for (i, cell) in cells.iter().enumerate() {
                connections[*cell] = labels[cells[(i + cells.len() - 1) % cells.len()]];
            }
        }
        self.add("ID".into(), &domain)?;
        for col in &permutation.columns {
            self.add(
                format!("S{col}"),
                &(0..n)
                    .map(|r| connections[r * width + col])
                    .collect::<Vec<_>>(),
            )?;
        }
        self.add(FIRST_COLUMN.into(), &indicator(n, &[0]))?;
        Ok(permutation.columns.len() + 2)
    }
}

/// Fixed columns for public bindings (SPEC.md, "Public values"): a binding's
/// row is selected by `PH · PL`, and its column has a `PA` column that is 1 on
/// all its public rows.
struct PublicSelectors {
    /// GateType size: `ceil(sqrt(q))` for `q` distinct rows holding public cells.
    side: usize,
    /// Position `i` of each public row among all public rows.
    row_index: BTreeMap<usize, usize>,
    /// Fixed-column index of each `PH` column.
    high: Vec<u32>,
    /// Fixed-column index of each `PL` column.
    low: Vec<u32>,
    /// Bound columns in first-binding order.
    columns: Vec<PublicColumn>,
}
struct PublicColumn {
    col: usize,
    /// Index of the column's `PA` indicator.
    selector: u32,
    /// `(row, public index)` pairs bound in this column.
    bindings: Vec<(usize, u32)>,
}
impl PublicSelectors {
    fn add(fixed: &mut FixedColumns, layout: &Layout) -> Result<Self> {
        let rows: Vec<_> = layout
            .public_bindings
            .iter()
            .map(|b| b.row)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut selectors = Self {
            side: 0,
            row_index: rows.iter().enumerate().map(|(i, r)| (*r, i)).collect(),
            high: vec![],
            low: vec![],
            columns: vec![],
        };
        if rows.is_empty() {
            return Ok(selectors);
        }
        let mut side = rows.len().isqrt();
        if side * side < rows.len() {
            side += 1;
        }
        selectors.side = side;
        let group_indicator = |select: &dyn Fn(usize) -> bool| -> Vec<u64> {
            (0..layout.rows)
                .map(|r| {
                    let index = selectors.row_index.get(&r);
                    u64::from(index.is_some_and(|i| select(*i)))
                })
                .collect()
        };
        let mut high = vec![];
        for h in 0..rows.len().div_ceil(side) {
            high.push(fixed.add(format!("PH{h}"), &group_indicator(&|i| i / side == h))?);
        }
        let mut low = vec![];
        for l in 0..side {
            low.push(fixed.add(format!("PL{l}"), &group_indicator(&|i| i % side == l))?);
        }
        let mut order = vec![];
        for b in &layout.public_bindings {
            if !order.contains(&b.col) {
                order.push(b.col);
            }
        }
        for col in order {
            let bindings: Vec<_> = layout
                .public_bindings
                .iter()
                .filter(|b| b.col == col)
                .map(|b| (b.row, b.public))
                .collect();
            let on: Vec<_> = bindings.iter().map(|(row, _)| *row).collect();
            let pa = fixed.add(format!("PA{col}"), &indicator(layout.rows, &on))?;
            selectors.columns.push(PublicColumn {
                col,
                selector: pa,
                bindings,
            });
        }
        selectors.high = high;
        selectors.low = low;
        Ok(selectors)
    }
}

/// Every symbol of a packed artifact; fixed columns are named `Kernel.{name}`.
fn expected_symbols(
    layout: &Layout,
    fixed_names: &[String],
    publics: u32,
    permutation: &PermutationArgument,
) -> BTreeMap<String, ExpectedSymbol> {
    let product = if layout.requires_permutation {
        PRODUCT_COLUMN_NAME
    } else {
        ZERO_COLUMN_NAME
    };
    let mut symbols = BTreeMap::from([
        (
            TRACE_SYMBOL.to_string(),
            ExpectedSymbol::air(
                SymbolType::WitnessCol,
                0,
                STAGE_ONE,
                vec![layout.width as u32],
            ),
        ),
        (
            product.to_string(),
            ExpectedSymbol::air(SymbolType::WitnessCol, PRODUCT_COLUMN, STAGE_TWO, vec![]),
        ),
        (
            CHALLENGE_NAMES[ALPHA as usize].to_string(),
            ExpectedSymbol::challenge(ALPHA, STAGE_TWO),
        ),
        (
            CHALLENGE_NAMES[GAMMA as usize].to_string(),
            ExpectedSymbol::challenge(GAMMA, STAGE_TWO),
        ),
    ]);
    let ratios = permutation.ratios();
    if ratios > 0 {
        symbols.insert(
            RATIOS_SYMBOL.into(),
            ExpectedSymbol::air(SymbolType::WitnessCol, 1, STAGE_TWO, vec![ratios]),
        );
    }
    for (i, name) in fixed_names.iter().enumerate() {
        symbols.insert(
            format!("{AIR_NAME}.{name}"),
            ExpectedSymbol::air(SymbolType::FixedCol, i as u32, 0, vec![]),
        );
    }
    if publics > 0 {
        symbols.insert(PUBLICS_SYMBOL.into(), ExpectedSymbol::publics(publics));
    }
    symbols
}

/// Expected gate and public-binding constraint polynomials, in PILout order.
/// Without a permutation argument, the `stage2_zero` constraint comes first.
fn main_polynomials(
    layout: &Layout,
    permutation_columns: usize,
    publics: &PublicSelectors,
) -> Result<Vec<Polynomial>> {
    let mut expected = if layout.requires_permutation {
        vec![]
    } else {
        vec![variable(Var::product())]
    };
    for gate in &layout.gates {
        let terms = &layout.gate_types[gate.gate_type].terms;
        let a = gate_polynomial(&terms[0], &gate.columns)?;
        let b = gate_polynomial(&terms[1], &gate.columns)?;
        let c = gate_polynomial(&terms[2], &gate.columns)?;
        let residual = difference(&product(&a, &b)?, &c);
        let selector = variable(Var::Fixed((permutation_columns + gate.selector) as u32));
        expected.push(product(&selector, &residual)?);
    }
    for column in &publics.columns {
        let mut bound = constant(0);
        for (row, public) in &column.bindings {
            let i = publics.row_index[row];
            let row_selector = product(
                &variable(Var::Fixed(publics.high[i / publics.side])),
                &variable(Var::Fixed(publics.low[i % publics.side])),
            )?;
            bound = sum(
                &bound,
                &product(&row_selector, &variable(Var::Public(*public)))?,
            );
        }
        let cell = product(
            &variable(Var::Fixed(column.selector)),
            &variable(Var::trace(column.col as u32)),
        )?;
        expected.push(difference(&cell, &bound));
    }
    Ok(expected)
}

/// CanonicalGate terms with gate type position `i` read from stage-one column `columns[i]`.
fn gate_polynomial(terms: &[(i32, FieldElement)], columns: &[usize]) -> Result<Polynomial> {
    let mut result = constant(0);
    for (position, k) in terms {
        let x = if *position < 0 {
            constant(1)
        } else {
            variable(Var::trace(columns[*position as usize] as u32))
        };
        result = sum(&result, &product(&constant(k.0), &x)?);
    }
    Ok(result)
}

/// Compare each gate and public constraint with its expected polynomial.
fn check_main_constraints(
    air: &pb::Air,
    layout: &Layout,
    publics: u32,
    roots: &[usize],
    expected: &[Polynomial],
) -> Result<()> {
    let end = roots.iter().copied().max().map_or(0, |i| i + 1);
    let mut interpreter = Interpreter::new(air, layout.width as u32, publics);
    interpreter.evaluate_until(end)?;
    for (index, (root, expected)) in roots.iter().zip(expected).enumerate() {
        ensure(
            interpreter.values.get(*root) == Some(expected),
            format_args!("constraint {index} differs from the expected gate or public constraint"),
        )?;
    }
    Ok(())
}

/// An expression operand that is not another expression.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Leaf {
    Constant(u64),
    Witness {
        stage: u32,
        column: u32,
        offset: i32,
    },
    Fixed {
        column: u32,
        offset: i32,
    },
    Public(u32),
    /// Stage-two challenge index.
    Challenge(u32),
}
impl Leaf {
    fn trace(column: u32) -> Self {
        Leaf::Witness {
            stage: STAGE_ONE,
            column,
            offset: 0,
        }
    }
    fn stage_two(column: u32, offset: i32) -> Self {
        Leaf::Witness {
            stage: STAGE_TWO,
            column,
            offset,
        }
    }
    fn fixed(column: u32) -> Self {
        Leaf::Fixed { column, offset: 0 }
    }
}
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Node {
    Leaf(Leaf),
    Op(BinaryOp, usize, usize),
}

/// What an operand may refer to.
struct OperandContext<'a> {
    /// Graph id of each AIR expression.
    expression_ids: &'a [usize],
    layout: &'a Layout,
    permutation: &'a PermutationArgument,
    publics: u32,
    fixed_columns: usize,
}

/// Expression graph in which identical expressions get the same id, so two
/// expressions have the same operations on the same operands exactly when
/// their ids are equal. Copy constraints and hints are compared this way.
#[derive(Default)]
struct ExpressionGraph {
    ids: BTreeMap<Node, usize>,
}
impl ExpressionGraph {
    fn intern(&mut self, node: Node) -> usize {
        let next = self.ids.len();
        *self.ids.entry(node).or_insert(next)
    }
    fn leaf(&mut self, leaf: Leaf) -> usize {
        self.intern(Node::Leaf(leaf))
    }
    fn node(&mut self, op: BinaryOp, a: usize, b: usize) -> usize {
        self.intern(Node::Op(op, a, b))
    }
    fn add(&mut self, a: usize, b: usize) -> usize {
        self.node(BinaryOp::Add, a, b)
    }
    fn sub(&mut self, a: usize, b: usize) -> usize {
        self.node(BinaryOp::Sub, a, b)
    }
    fn mul(&mut self, a: usize, b: usize) -> usize {
        self.node(BinaryOp::Mul, a, b)
    }
    fn constant(&mut self, x: u64) -> usize {
        self.leaf(Leaf::Constant(x))
    }
    /// `1 · f₀ · f₁ · …`, multiplied left to right as the compiler does.
    fn product(&mut self, factors: &[usize]) -> usize {
        let one = self.constant(1);
        factors.iter().fold(one, |p, f| self.mul(p, *f))
    }

    /// Check and add every AIR expression; returns the id of each one.
    fn import(
        &mut self,
        air: &pb::Air,
        layout: &Layout,
        permutation: &PermutationArgument,
        publics: u32,
        fixed_columns: usize,
    ) -> Result<Vec<usize>> {
        let mut expression_ids = vec![];
        for (index, expression) in air.expressions.iter().enumerate() {
            let (op, lhs, rhs) =
                binary_operation(expression).context(format!("expression {index}"))?;
            let context = OperandContext {
                expression_ids: &expression_ids,
                layout,
                permutation,
                publics,
                fixed_columns,
            };
            let lhs = self
                .operand(lhs, &context)
                .context(format!("expression {index}, lhs"))?;
            let rhs = self
                .operand(rhs, &context)
                .context(format!("expression {index}, rhs"))?;
            expression_ids.push(self.node(op, lhs, rhs));
        }
        Ok(expression_ids)
    }

    /// Check one operand against the certified geometry and return its id.
    fn operand(&mut self, o: &pb::Operand, context: &OperandContext) -> Result<usize> {
        let layout = context.layout;
        let leaf = match o.operand.as_ref().ok_or("missing operand")? {
            Operand::Constant(x) => Leaf::Constant(scalar(&x.value)?),
            Operand::WitnessCol(x) => {
                let stage_one =
                    x.stage == STAGE_ONE && x.col_idx < layout.width as u32 && x.row_offset == 0;
                // Only the running product may be read at the previous row.
                let stage_two = x.stage == STAGE_TWO
                    && x.col_idx < context.permutation.stage_two_width()
                    && (x.row_offset == 0
                        || (layout.requires_permutation
                            && x.col_idx == PRODUCT_COLUMN
                            && x.row_offset == -1));
                ensure(stage_one || stage_two, "witness column out of range")?;
                Leaf::Witness {
                    stage: x.stage,
                    column: x.col_idx,
                    offset: x.row_offset,
                }
            }
            Operand::FixedCol(x) => {
                // Only FIRST may be read at the next row: FIRST[r + 1] is the
                // LAST selector, 1 on the last row.
                ensure(
                    x.idx < context.fixed_columns as u32
                        && (x.row_offset == 0
                            || (layout.requires_permutation
                                && x.idx == context.permutation.first_column()
                                && x.row_offset == 1)),
                    "fixed column out of range or with a row offset",
                )?;
                Leaf::Fixed {
                    column: x.idx,
                    offset: x.row_offset,
                }
            }
            Operand::PublicValue(x) => {
                ensure(x.idx < context.publics, "public value out of range")?;
                Leaf::Public(x.idx)
            }
            Operand::Challenge(x) => {
                ensure(
                    x.stage == STAGE_TWO && (x.idx == ALPHA || x.idx == GAMMA),
                    "challenge out of range",
                )?;
                Leaf::Challenge(x.idx)
            }
            Operand::Expression(x) => {
                return Ok(*context
                    .expression_ids
                    .get(x.idx as usize)
                    .ok_or("expression refers to itself or a later expression")?)
            }
            Operand::ProofValue(_)
            | Operand::AirGroupValue(_)
            | Operand::PeriodicCol(_)
            | Operand::AirValue(_)
            | Operand::CustomCol(_) => return Err("unsupported operand type".into()),
        };
        Ok(self.leaf(leaf))
    }
}

/// Hint name and the expected expression id of each named field.
type ExpectedHint = (&'static str, BTreeMap<&'static str, usize>);

/// `z + 0`, the `reference` field of the running-product or `stage2_zero` hint.
fn reference_field(graph: &mut ExpressionGraph) -> usize {
    let z = graph.leaf(Leaf::stage_two(PRODUCT_COLUMN, 0));
    let zero = graph.constant(0);
    graph.add(z, zero)
}

/// The hint of the `stage2_zero` column, the only stage-two column without a
/// permutation argument.
fn zero_hint(graph: &mut ExpressionGraph) -> ExpectedHint {
    let reference = reference_field(graph);
    let alpha = graph.leaf(Leaf::Challenge(ALPHA));
    // Written as α - α: zero, but an expression over a stage-two challenge.
    let challenge_zero = graph.sub(alpha, alpha);
    let (zero, one) = (graph.constant(0), graph.constant(1));
    (
        SUM_HINT,
        BTreeMap::from([
            ("reference", reference),
            ("numerator_air", challenge_zero),
            ("denominator_air", one),
            ("numerator_direct", zero),
            ("denominator_direct", one),
        ]),
    )
}

/// Rebuild the ratio constraints, the running-product recurrence and the
/// closure constraint (SPEC.md, "Permutation argument"), compare them with the actual
/// constraints, and return the hints they require.
fn check_permutation_constraints(
    graph: &mut ExpressionGraph,
    expression_ids: &[usize],
    permutation: &PermutationArgument,
    roots: &[usize],
) -> Result<Vec<ExpectedHint>> {
    let ratios = permutation.ratios() as usize;
    let (numerators, denominators) = permutation_factors(graph, &permutation.columns);
    let mut hints = vec![];
    let (numerator, denominator) = check_ratios(
        graph,
        expression_ids,
        &roots[..ratios],
        &numerators,
        &denominators,
        &mut hints,
    )?;
    check_running_product(
        graph,
        expression_ids,
        permutation,
        &roots[ratios..],
        numerator,
        denominator,
    )?;
    let reference = reference_field(graph);
    let one = graph.constant(1);
    hints.push((
        PRODUCT_HINT,
        BTreeMap::from([
            ("reference", reference),
            ("numerator_air", numerator),
            ("denominator_air", denominator),
            ("numerator_direct", one),
            ("denominator_direct", one),
            ("result", one),
        ]),
    ));
    Ok(hints)
}

/// For each permutation column, the numerator factor with the permuted label column
/// and the denominator factor with the cell's own label.
fn permutation_factors(graph: &mut ExpressionGraph, columns: &[usize]) -> (Vec<usize>, Vec<usize>) {
    let alpha = graph.leaf(Leaf::Challenge(ALPHA));
    let gamma = graph.leaf(Leaf::Challenge(GAMMA));
    let id = graph.leaf(Leaf::fixed(0));
    let (mut numerators, mut denominators) = (vec![], vec![]);
    for (index, col) in columns.iter().enumerate() {
        let value = graph.leaf(Leaf::trace(*col as u32));
        let shift_constant = graph.constant(pow(COSET, *col as u64));
        let own_label = graph.mul(shift_constant, id);
        let own_label = graph.mul(alpha, own_label);
        let own = graph.add(value, own_label);
        denominators.push(graph.add(own, gamma));
        let permuted_column = graph.leaf(Leaf::fixed(index as u32 + 1));
        let permuted_label = graph.mul(alpha, permuted_column);
        let permuted = graph.add(value, permuted_label);
        numerators.push(graph.add(permuted, gamma));
    }
    (numerators, denominators)
}

/// Compare each ratio constraint with its rebuilt form and add its hint. The
/// first ratio takes up to `MAX_DEGREE` numerator and `MAX_DEGREE - 1`
/// denominator factors; each later one takes the previous ratio and up to
/// `MAX_DEGREE - 1` of each. Returns the numerator and denominator left for
/// the running product.
fn check_ratios(
    graph: &mut ExpressionGraph,
    expression_ids: &[usize],
    roots: &[usize],
    numerators: &[usize],
    denominators: &[usize],
    hints: &mut Vec<ExpectedHint>,
) -> Result<(usize, usize)> {
    let factors = numerators.len();
    // The running numerator and the next unused numerator and denominator factor.
    let (mut numerator, mut next_num, mut next_den) = (graph.constant(1), 0, 0);
    for (i, root) in roots.iter().enumerate() {
        let num_end = factors.min(MAX_DEGREE + (MAX_DEGREE - 1) * i);
        let den_end = factors.min((MAX_DEGREE - 1) * (i + 1));
        for factor in &numerators[next_num..num_end] {
            numerator = graph.mul(numerator, *factor);
        }
        let denominator = graph.product(&denominators[next_den..den_end]);
        let ratio = graph.leaf(Leaf::stage_two(i as u32 + 1, 0));
        let lhs = graph.mul(ratio, denominator);
        let residual = graph.sub(lhs, numerator);
        ensure(
            expression_ids.get(*root) == Some(&residual),
            format_args!("permutation ratio constraint {i} differs from the expected one"),
        )?;
        hints.push((
            RATIO_HINT,
            BTreeMap::from([
                ("reference", ratio),
                ("numerator", numerator),
                ("denominator", denominator),
            ]),
        ));
        numerator = ratio;
        next_num = num_end;
        next_den = den_end;
    }
    for factor in &numerators[next_num..] {
        numerator = graph.mul(numerator, *factor);
    }
    let denominator = graph.product(&denominators[next_den..]);
    Ok((numerator, denominator))
}

/// Compare the recurrence `z · den = z[-1] · num` and the closure
/// `LAST · (z - 1)` with the two constraints in `roots`.
fn check_running_product(
    graph: &mut ExpressionGraph,
    expression_ids: &[usize],
    permutation: &PermutationArgument,
    roots: &[usize],
    numerator: usize,
    denominator: usize,
) -> Result<()> {
    let z = graph.leaf(Leaf::stage_two(PRODUCT_COLUMN, 0));
    let previous = graph.leaf(Leaf::stage_two(PRODUCT_COLUMN, -1));
    let lhs = graph.mul(z, denominator);
    let rhs = graph.mul(previous, numerator);
    let recurrence = graph.sub(lhs, rhs);
    let last = graph.leaf(Leaf::Fixed {
        column: permutation.first_column(),
        offset: 1,
    });
    let one = graph.constant(1);
    let z_minus_one = graph.sub(z, one);
    let closure = graph.mul(last, z_minus_one);
    ensure(
        expression_ids.get(roots[0]) == Some(&recurrence),
        "permutation recurrence differs from the expected one",
    )?;
    ensure(
        expression_ids.get(roots[1]) == Some(&closure),
        "permutation closure differs from the expected one",
    )
}

/// Compare every hint, in order, with the expected hints.
fn check_hints(
    pilout: &pb::PilOut,
    expected: &[ExpectedHint],
    graph: &mut ExpressionGraph,
    context: &OperandContext,
) -> Result<()> {
    let names: Vec<_> = expected.iter().map(|(name, _)| *name).collect();
    let actual = pilout_check::hint_fields(pilout, &names)?;
    for (fields, (name, wanted)) in actual.iter().zip(expected) {
        pilout_check::check_field_names(name, fields, wanted)?;
        for (field, operand) in fields {
            ensure(
                wanted.get(field) == Some(&graph.operand(operand, context)?),
                format_args!("hint '{name}': field '{field}' differs from the expected expression"),
            )?;
        }
    }
    Ok(())
}
