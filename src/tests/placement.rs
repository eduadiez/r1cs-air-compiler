//! Placement check: helper networks and layouts against their source.
use super::{assert_limit, assert_rejected, packed::config, r1cs_bytes};
use crate::{
    certificate::{Group, Helper, Layout, Network, Placement, Selector, Slot},
    field::{add, FieldElement},
    layout::{self, LayoutConfig, Strategy},
    network, packed, placement_check, r1cs, P,
};

/// The xor fixture packed with its layout: helpers, shared shapes and a copy
/// argument at width 8.
fn xor_plan() -> (r1cs::Circuit, Network, Layout) {
    let source = include_bytes!("../../fixtures/xor/kernel.r1cs");
    let settings: LayoutConfig =
        serde_json::from_slice(include_bytes!("../../fixtures/xor/layout.json")).unwrap();
    let artifact = packed::compile(source, &settings).unwrap();
    let circuit = r1cs::read(source).unwrap();
    let certificate = artifact.certificate;
    (circuit, certificate.network, certificate.layout)
}

/// Apply `change` to the xor network and layout and require the placement
/// check to reject the result with `expected`.
fn rejects(expected: &str, change: impl Fn(&mut Network, &mut Layout)) {
    let (circuit, mut net, mut plan) = xor_plan();
    assert!(placement_check::check(&circuit, &net, &plan).is_ok());
    change(&mut net, &mut plan);
    assert_rejected(placement_check::check(&circuit, &net, &plan), expected);
}

/// Like `rejects`, but require a size limit error.
fn exceeds(expected: &str, change: impl Fn(&mut Network, &mut Layout)) {
    let (circuit, mut net, mut plan) = xor_plan();
    change(&mut net, &mut plan);
    assert_limit(placement_check::check(&circuit, &net, &plan), expected);
}

#[test]
fn network_and_layout_dimensions_are_checked() {
    let counts = "network wire or public counts differ from the source";
    rejects(counts, |n, _| n.original_wires += 1);
    rejects(counts, |n, _| n.publics += 1);
    rejects(counts, |n, _| n.wires += 1);
    rejects("width must be in 3..=1024", |_, l| l.width = 2);
    rejects("width must be in 3..=1024", |_, l| l.width = 1025);
    rejects("minRows must be a power of two", |_, l| l.min_rows = 32);
    rejects("minRows must be a power of two", |_, l| l.min_rows = 96);
    let rows = "rows must be a power of two in minRows..=65536";
    rejects(rows, |_, l| l.rows = 96);
    rejects(rows, |_, l| {
        l.rows = 131_072;
        l.min_rows = 131_072;
    });
    rejects(rows, |_, l| l.min_rows = 128);
    exceeds("layout exceeds 4000000 cells", |_, l| {
        l.width = 128;
        l.rows = 65_536;
        l.min_rows = 65_536;
    });
    rejects("rows must be usedRows rounded up", |_, l| l.rows = 128);
    rejects("usedRows must count the rows up to the last one", |_, l| {
        l.used_rows += 1
    });
    rejects("arithmeticRows must be the last row", |_, l| {
        l.arithmetic_rows += 1
    });
    rejects("one converted equation per source equation", |n, _| {
        n.converted.pop();
    });
    rejects("requiresCopy must be set exactly", |_, l| {
        l.requires_copy = false
    });
}

#[test]
fn helpers_are_checked() {
    rejects(
        "helper 0: ids must follow the source wires in order",
        |n, _| n.helpers[0].id += 1,
    );
    rejects("helper 0: needs at least two terms", |n, _| {
        n.helpers[0].terms.truncate(1);
    });
    rejects("helper 0: first coefficient must be 1", |n, _| {
        n.helpers[0].terms[0].1 = FieldElement(2)
    });
    rejects("helper 0: terms must not use wire 0", |n, _| {
        n.helpers[0].terms[0].0 = 0
    });
    let (_, network, _) = xor_plan();
    let first_helper = network.helpers[0].id;
    rejects(
        &format!("helper 0: wire {first_helper} out of range"),
        |n, _| {
            let id = n.helpers[0].id;
            n.helpers[0].terms.push((id, FieldElement(1)));
        },
    );
    rejects("helper 0: wires must be in increasing order", |n, _| {
        n.helpers[0].terms.reverse()
    });
}

#[test]
fn converted_equations_are_checked() {
    let coefficient = "converted equation 0: coefficient of wire";
    rejects(coefficient, |n, _| n.converted[0][2][0].1 = FieldElement(0));
    rejects(coefficient, |n, _| n.converted[0][2][0].1 = FieldElement(P));
    let (_, network, _) = xor_plan();
    let past_last = network.wires;
    rejects(
        &format!("converted equation 0: wire {past_last} out of range"),
        |n, _| {
            let wire = n.wires;
            n.converted[0][2].push((wire, FieldElement(1)));
        },
    );
    // Changes that keep the relation but break canonical form.
    let with_spare_wire = |n: &Network| {
        (0..n.converted.len())
            .find(|i| {
                n.converted[*i][2]
                    .last()
                    .is_some_and(|(wire, _)| *wire + 1 < n.wires)
            })
            .unwrap()
    };
    rejects("coefficient of wire", |n, _| {
        let i = with_spare_wire(n);
        let wire = n.wires - 1;
        n.converted[i][2].push((wire, FieldElement(0)));
    });
    rejects("coefficient of wire", |n, _| {
        let i = with_spare_wire(n);
        let wire = n.wires - 1;
        n.converted[i][2].push((wire, FieldElement(P)));
    });
    rejects("wires must be in increasing order", |n, _| {
        let i = with_spare_wire(n);
        let (wire, k) = n.converted[i][2].pop().unwrap();
        let rest = add(k.0, P - 2);
        n.converted[i][2].extend([(wire, FieldElement(2)), (wire, FieldElement(rest))]);
    });
}

#[test]
fn changed_converted_equation_is_rejected() {
    let source = r1cs::read(&r1cs_bytes([5, 1, 1, 2], &[[1, 2, 3], [1, 3, 4]])).unwrap();
    let mut net = network::convert(&source, &config(3).options).unwrap();
    // Change one coefficient; the equation count is unchanged.
    net.converted[0][2][0].1 .0 = 2;
    let plan = layout::pack(&net, &config(3)).unwrap();
    assert_rejected(
        placement_check::check(&source, &net, &plan),
        "converted equation 0 differs from its source equation",
    );
}

#[test]
fn shapes_are_checked() {
    rejects("shape 0: id out of order", |_, l| l.groups[0].id = 1);
    rejects("shape 0: wider than the layout", |_, l| {
        l.groups[0].width = l.width + 1
    });
    rejects("shape 0: position out of range", |_, l| {
        l.groups[0].terms[2][0].0 = -2
    });
    rejects("shape 0: position out of range", |_, l| {
        let width = l.groups[0].width as i32;
        l.groups[0].terms[2].last_mut().unwrap().0 = width;
    });
    // A zero or P coefficient at an unused position leaves the relation intact.
    let with_spare_position = |l: &Layout| {
        l.groups
            .iter()
            .position(|g| {
                g.terms[2]
                    .last()
                    .is_some_and(|(i, _)| *i + 1 < g.width as i32)
            })
            .unwrap()
    };
    for k in [0, P] {
        rejects("coefficient must be in 1..P", |_, l| {
            let g = with_spare_position(l);
            let position = l.groups[g].width as i32 - 1;
            l.groups[g].terms[2].push((position, FieldElement(k)));
        });
    }
    rejects("shape 0: coefficient must be in 1..P", |_, l| {
        l.groups[0].terms[2][0].1 = FieldElement(0)
    });
    rejects("shape 0: positions must be in increasing order", |_, l| {
        let (position, k) = l.groups[0].terms[2].pop().unwrap();
        let rest = add(k.0, P - 2);
        l.groups[0].terms[2].extend([(position, FieldElement(2)), (position, FieldElement(rest))]);
    });
    rejects("shape 0: positions must be in increasing order", |_, l| {
        let first = l.groups[0].terms[2][0];
        l.groups[0].terms[2].push(first);
    });
}

#[test]
fn selectors_and_slots_are_checked() {
    rejects("selector 0: id out of order", |_, l| l.selectors[0].id = 9);
    rejects("selector 0: no active rows", |_, l| {
        l.selectors[0].rows.clear()
    });
    rejects("selector 0: rows must be in increasing order", |_, l| {
        let row = l.selectors[0].rows[0];
        l.selectors[0].rows.push(row);
    });
    rejects("selector 0: row beyond arithmeticRows", |_, l| {
        let row = l.arithmetic_rows;
        l.selectors[0].rows.push(row);
    });
    rejects("slot 0: unknown shape", |_, l| {
        l.slots[0].shape = l.groups.len()
    });
    rejects("slot 0: unknown selector", |_, l| {
        l.slots[0].selector = l.selectors.len()
    });
    rejects("slot 0: rows differ from its selector's rows", |_, l| {
        l.slots[0].rows.push(63)
    });
    rejects(
        "slot 0: column count differs from its shape width",
        |_, l| {
            l.slots[0].columns.pop();
        },
    );
    rejects("slot 0: column out of range", |_, l| {
        l.slots[0].columns[0] = l.width
    });
    rejects("slot 0: repeated column", |_, l| {
        l.slots[0].columns[1] = l.slots[0].columns[0];
    });
    rejects("duplicate of an earlier slot", |_, l| {
        let slot = l.slots[0].clone();
        l.slots.push(slot);
    });
}

#[test]
fn every_active_slot_row_needs_a_placement() {
    rejects(
        "placements must match the active slot rows exactly",
        |_, l| {
            // Activate selector 0 on one more row that holds equations.
            let selector = &l.selectors[0];
            let row = (0..l.arithmetic_rows)
                .find(|r| !selector.rows.contains(r))
                .expect("a row without selector 0");
            let mut rows = selector.rows.clone();
            rows.push(row);
            rows.sort_unstable();
            l.selectors[0].rows = rows.clone();
            for slot in l.slots.iter_mut().filter(|s| s.selector == 0) {
                slot.rows = rows.clone();
            }
        },
    );
}

#[test]
fn placements_are_checked() {
    let (_, _, plan) = xor_plan();
    let first = plan.placements[0].index;
    rejects("one placement per helper and converted equation", |_, l| {
        l.placements.pop();
    });
    rejects("placement of unknown equation", |_, l| {
        l.placements[0].index = l.placements.len()
    });
    rejects("is placed twice", |_, l| {
        l.placements[1].index = l.placements[0].index
    });
    rejects(&format!("equation {first}: unknown shape"), |_, l| {
        l.placements[0].shape = l.groups.len()
    });
    rejects(
        &format!("equation {first}: row beyond arithmeticRows"),
        |_, l| l.placements[0].row = l.arithmetic_rows,
    );
    let count = format!("equation {first}: wire or column count differs from its shape width");
    rejects(&count, |_, l| {
        l.placements[0].wires.pop();
    });
    rejects(&count, |_, l| {
        l.placements[0].columns.pop();
    });
    rejects(&format!("equation {first}: repeated column"), |_, l| {
        l.placements[0].columns[1] = l.placements[0].columns[0];
    });
    rejects(
        &format!("equation {first}: its shape and wires give a different equation"),
        |_, l| {
            let shape = l.placements[0].shape;
            let k = l.groups[shape].terms[2][0].1 .0;
            l.groups[shape].terms[2][0].1 = FieldElement(if k == 1 { 2 } else { 1 });
        },
    );
    rejects(
        &format!("equation {first} is placed on a row where its slot is not active"),
        |_, l| {
            let placement = &l.placements[0];
            let active: Vec<_> = l
                .slots
                .iter()
                .filter(|s| s.shape == placement.shape && s.columns == placement.columns)
                .flat_map(|s| s.rows.clone())
                .collect();
            let row = (0..l.arithmetic_rows)
                .find(|r| !active.contains(r))
                .expect("a row where the slot is inactive");
            l.placements[0].row = row;
        },
    );
}

#[test]
fn unused_selectors_and_shapes_are_rejected() {
    rejects("no slot uses it", |_, l| {
        let id = l.selectors.len();
        l.selectors.push(Selector { id, rows: vec![0] });
    });
    rejects("no equation uses it", |_, l| {
        let mut unused = l.groups[0].clone();
        unused.id = l.groups.len();
        l.groups.push(unused);
    });
}

#[test]
fn public_bindings_are_checked() {
    let order = "public binding i must bind public value i to wire i + 1";
    rejects("one public binding per public value", |_, l| {
        l.public_bindings.pop();
    });
    rejects(order, |_, l| l.public_bindings[0].public = 1);
    rejects(order, |_, l| l.public_bindings[0].wire = 2);
    rejects("cell (64, ", |_, l| l.public_bindings[0].row = 64);
    rejects(", 8) out of range", |_, l| {
        l.public_bindings[0].col = l.width
    });
}

#[test]
fn a_cell_cannot_hold_two_different_wires() {
    // Move the first public binding onto a cell that holds another wire.
    rejects("would hold two different wires", |_, l| {
        let wire = l.public_bindings[0].wire;
        let (row, column) = l
            .placements
            .iter()
            .flat_map(|p| {
                p.wires
                    .iter()
                    .zip(&p.columns)
                    .map(move |(w, c)| (*w, p.row, *c))
            })
            .find(|(w, _, _)| *w != wire)
            .map(|(_, row, column)| (row, column))
            .unwrap();
        l.public_bindings[0].row = row;
        l.public_bindings[0].col = column;
    });
}

/// Source `1 · w1 = w2` (factor A is the constant wire 0), placed by hand as one
/// equation of the given shape on row 0, columns 0, 1 and 2.
fn constant_factor_plan(group: Group, wires: Vec<u32>) -> (r1cs::Circuit, Network, Layout) {
    let circuit = r1cs::read(&r1cs_bytes([3, 0, 0, 2], &[[0, 1, 2]])).unwrap();
    let network = Network {
        original_wires: 3,
        wires: 3,
        publics: 0,
        helpers: vec![],
        converted: vec![circuit.equations[0].clone()],
    };
    let columns = vec![0, 1, 2];
    let layout = Layout {
        min_rows: 64,
        width: 3,
        rows: 64,
        used_rows: 1,
        arithmetic_rows: 1,
        groups: vec![group],
        placements: vec![Placement {
            index: 0,
            shape: 0,
            wires,
            row: 0,
            columns: columns.clone(),
        }],
        slots: vec![Slot {
            shape: 0,
            columns,
            rows: vec![0],
            selector: 0,
        }],
        selectors: vec![Selector {
            id: 0,
            rows: vec![0],
        }],
        public_bindings: vec![],
        requires_copy: false,
    };
    (circuit, network, layout)
}

#[test]
fn only_nonconstant_source_and_helper_wires_occupy_cells() {
    let one = FieldElement(1);
    // Wire 0 at shape position 0: the shape gives back the equation, but the
    // cell would stand for the constant 1 without any constraint.
    let group = Group {
        id: 0,
        width: 3,
        terms: [vec![(0, one)], vec![(1, one)], vec![(2, one)]],
    };
    let (circuit, network, layout) = constant_factor_plan(group, vec![0, 1, 2]);
    assert_rejected(
        placement_check::check(&circuit, &network, &layout),
        "wire 0 cannot occupy a cell",
    );
    // An unknown wire at a position that no term uses.
    let group = Group {
        id: 0,
        width: 3,
        terms: [vec![(-1, one)], vec![(0, one)], vec![(1, one)]],
    };
    for unknown in [3, 999] {
        let (circuit, network, layout) = constant_factor_plan(group.clone(), vec![1, 2, unknown]);
        assert_rejected(
            placement_check::check(&circuit, &network, &layout),
            &format!("wire {unknown} cannot occupy a cell"),
        );
    }
}

/// Source `w1 · w2 = w3`, with one helper `h = w4` standing in for `w1` in factor A.
fn helper_network(
    terms: Vec<(u32, FieldElement)>,
    a: Vec<(u32, FieldElement)>,
) -> (r1cs::Circuit, Network, Layout) {
    let circuit = r1cs::read(&r1cs_bytes([4, 0, 0, 3], &[[1, 2, 3]])).unwrap();
    let net = Network {
        original_wires: 4,
        wires: 5,
        publics: 0,
        helpers: vec![Helper { id: 4, terms }],
        converted: vec![[a, vec![(2, FieldElement(1))], vec![(3, FieldElement(1))]]],
    };
    let mut settings = config(3);
    settings.strategy = Strategy::Mixed;
    let plan = layout::pack(&net, &settings).unwrap();
    (circuit, net, plan)
}

#[test]
fn helpers_must_be_normalized_even_when_the_relation_holds() {
    let check = |terms, a| {
        let (circuit, net, plan) = helper_network(terms, a);
        placement_check::check(&circuit, &net, &plan)
    };
    let inverse_of_two = P.div_ceil(2); // (P + 1) / 2
                                        // h = w1 + w2, A = h - w2.
    assert!(check(
        vec![(1, FieldElement(1)), (2, FieldElement(1))],
        vec![(2, FieldElement(P - 1)), (4, FieldElement(1))]
    )
    .is_ok());
    // h = w1 alone.
    assert_rejected(
        check(vec![(1, FieldElement(1))], vec![(4, FieldElement(1))]),
        "helper 0: needs at least two terms",
    );
    // h = 1 + w1, A = h - 1.
    assert_rejected(
        check(
            vec![(0, FieldElement(1)), (1, FieldElement(1))],
            vec![(0, FieldElement(P - 1)), (4, FieldElement(1))],
        ),
        "helper 0: terms must not use wire 0",
    );
    // h = 2·w1 + w2, A = (h - w2) / 2.
    assert_rejected(
        check(
            vec![(1, FieldElement(2)), (2, FieldElement(1))],
            vec![
                (2, FieldElement(P - inverse_of_two)),
                (4, FieldElement(inverse_of_two)),
            ],
        ),
        "helper 0: first coefficient must be 1",
    );
}
