//! The camera-relative form of a vertex program's clip position, read from its value graph ([`clip_rows`]).
//!
//! A Frostbite vertex program places a vertex with a constant buffer's words: each lane `k` of its clip position is four
//! terms, the three coordinates of the world position minus a three-word vector, each times a word of the lane's row, and
//! the row's last word (`row[k][0] * (W.x - c.x) + row[k][1] * (W.y - c.y) + row[k][2] * (W.z - c.z) + row[k][3]`). The
//! sixteen row words are consecutive, four to a lane, and the three words of `c` too, in the same buffer. That is a
//! view-projection of a world whose origin is the camera, and `c` is the camera's position: [`ViewParameter`] has both.
//!
//! What was checked, on the 3,341 vertex programs of one install (a read-only census; the shapes below are word offsets
//! of the constant buffer at register 4, which has a different size and layout in each pass):
//! - the structural test here accepts 2,883 of them: 2,779 with the rows at word 980 and `c` at 1060 (a buffer of 4,552
//!   bytes), 88 with the rows at 1684 and `c` at 1820 (7,804 bytes), 16 with the rows at 1684 and `c` at 1764 (7,368 bytes);
//! - each was run on the CPU executor with the rows and `c` set to other values: 2,845 give the form (the clip position
//!   changes by `row[k] * (W - c) + row[k][3]` for the world position `W` of a run with identity rows and `c` = 0), none
//!   contradicts it, and 38 are undecided (a run with the inputs the check gave was not finite);
//! - the test rejects what is not that form: programs whose world position is itself made of the words of `c` (63 of
//!   those that read the same words), lanes of a different shape (a lane that is one term, five or seven terms) and
//!   programs that read no word of the buffer for their position. Their words stay unknown.
//!
//! Where the words sit is the program's, so the roles are stated per program, never from a fixed offset.
//!
//! [`ViewParameter`]: primase_core::shader::ViewParameter

use std::collections::BTreeSet;

use primase_core::shader::{
    ConstantBinding, ConstantRole, D3dClass, Interface, MatrixLines, ResourceId, ResourceKind, Shader,
    ShaderFormat, Stage, SystemValue, ViewParameter,
};
use iridia::flow::{At, BinaryOperator, Node, NodeId, Op, Values};

/// Where a vertex program's clip position reads its matrix rows and its camera position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipRows {
    /// The first word of the sixteen row words: lane `k`'s `x`, `y`, `z` coefficients and constant are the words
    /// `rows + 4 * k` to `rows + 4 * k + 3`.
    pub rows: u32,
    /// The first of the three words of the camera position the program subtracts from its world position.
    pub camera: u32,
}

impl ClipRows {
    /// The bytes of the sixteen row words, from `rows * 4`.
    pub const ROWS_BYTES: u32 = 64;
    /// The bytes of the camera position, from `camera * 4`.
    pub const CAMERA_BYTES: u32 = 12;
}

/// The terms of a tree of additions.
fn terms(values: &Values, id: NodeId, out: &mut Vec<NodeId>) {
    if let Node::Op {
        op: Op::Binary(BinaryOperator::Add),
        operands,
    } = values.node(id)
    {
        for &o in operands {
            terms(values, o, out);
        }
    } else {
        out.push(id);
    }
}

/// The two operands of a binary operation.
fn pair(values: &Values, id: NodeId, wanted: BinaryOperator) -> Option<(NodeId, NodeId)> {
    match values.node(id) {
        Node::Op {
            op: Op::Binary(op),
            operands,
        } if *op == wanted && operands.len() == 2 => Some((operands[0], operands[1])),
        _ => None,
    }
}

/// The word of `buffer` a node is.
fn word(values: &Values, id: NodeId, buffer: ResourceId) -> Option<u32> {
    match values.node(id) {
        Node::Global {
            resource: Some(r),
            at: At::Component(c),
        } if *r == buffer => Some(*c),
        _ => None,
    }
}

/// The globals a node is made of.
fn globals_of(values: &Values, id: NodeId, seen: &mut BTreeSet<NodeId>, out: &mut Vec<NodeId>) {
    if !seen.insert(id) {
        return;
    }
    if matches!(values.node(id), Node::Global { .. }) {
        out.push(id);
    }
    for o in values.operands(id) {
        globals_of(values, o, seen, out);
    }
}

/// The rows and the camera words `buffer` gives the clip position of the program `values` is the graph of, when every
/// lane has exactly the form of the module documentation: four terms, one product of a word of the row `a + 4k + j` and
/// `W_j - c_j` for each coordinate `j` (the same `W_j` in every lane, none made of a word the rows or the camera use) and
/// the row's last word, alone or times one. `Err` says what the graph does instead.
pub fn clip_rows(values: &Values, interface: &Interface, buffer: ResourceId) -> Result<ClipRows, String> {
    let position = interface
        .outputs
        .iter()
        .position(|o| o.system == Some(SystemValue::ClipPosition))
        .ok_or("the program writes no clip position")?;
    let one = |n: NodeId| matches!(values.node(n), Node::Literal(l) if l.0 == 1.0);
    let mut found: Option<ClipRows> = None;
    let mut world: [Option<NodeId>; 3] = [None; 3];
    let mut used: BTreeSet<u32> = BTreeSet::new();
    for k in 0..4u32 {
        let lane = values
            .output(position, k as u8)
            .ok_or_else(|| format!("clip lane {k} is not written"))?;
        let mut sum = Vec::new();
        terms(values, lane, &mut sum);
        if sum.len() != 4 {
            return Err(format!("clip lane {k} is {} terms, not four", sum.len()));
        }
        // each term is the lane's constant word or a coefficient word times (a world coordinate - a camera word)
        let mut constant: Option<u32> = None;
        let mut products: Vec<(u32, NodeId, u32)> = Vec::new();
        for term in sum {
            if let Some(w) = word(values, term, buffer) {
                if constant.replace(w).is_some() {
                    return Err(format!("clip lane {k} has two constant terms"));
                }
                continue;
            }
            let (x, y) = pair(values, term, BinaryOperator::Multiply)
                .ok_or_else(|| format!("clip lane {k} has a term that is no product and no word"))?;
            let (coefficient, other) = if word(values, x, buffer).is_some() {
                (x, y)
            } else {
                (y, x)
            };
            let coefficient = word(values, coefficient, buffer)
                .ok_or_else(|| format!("clip lane {k} has a product without a word of the buffer"))?;
            if one(other) {
                if constant.replace(coefficient).is_some() {
                    return Err(format!("clip lane {k} has two constant terms"));
                }
                continue;
            }
            let (coordinate, camera) = pair(values, other, BinaryOperator::Subtract)
                .and_then(|(w, c)| Some((w, word(values, c, buffer)?)))
                .ok_or_else(|| {
                    format!("clip lane {k} has a product that is not a word times (world - camera word)")
                })?;
            products.push((coefficient, coordinate, camera));
        }
        let constant = constant.ok_or_else(|| format!("clip lane {k} has no constant term"))?;
        if products.len() != 3 {
            return Err(format!(
                "clip lane {k} has {} products, not three",
                products.len()
            ));
        }
        let rows = constant.wrapping_sub(3).wrapping_sub(4 * k);
        let mut camera: [Option<u32>; 3] = [None; 3];
        for (coefficient, coordinate, c) in products {
            let j = coefficient.wrapping_sub(rows.wrapping_add(4 * k));
            if j >= 3 || camera[j as usize].is_some() {
                return Err(format!(
                    "clip lane {k}'s words are not one row of four from the first row"
                ));
            }
            let j = j as usize;
            match world[j] {
                Some(known) if known != coordinate => {
                    return Err(format!(
                        "clip lane {k} takes another world coordinate {j} than lane 0"
                    ));
                }
                _ => world[j] = Some(coordinate),
            }
            camera[j] = Some(c);
        }
        let [Some(c0), Some(c1), Some(c2)] = camera else {
            return Err(format!("clip lane {k} has no camera word for a coordinate"));
        };
        if c1 != c0 + 1 || c2 != c0 + 2 {
            return Err(format!("clip lane {k}'s camera words are not three in a row"));
        }
        let lane_form = ClipRows { rows, camera: c0 };
        match found {
            Some(first) if first != lane_form => {
                return Err(format!(
                    "clip lane {k} reads other rows or another camera than lane 0"
                ));
            }
            _ => found = Some(lane_form),
        }
        used.extend((0..4).map(|i| rows + 4 * k + i));
        used.extend([c0, c1, c2]);
    }
    // the world position is not made of the words that place it
    for coordinate in world.into_iter().flatten() {
        let (mut seen, mut globals) = (BTreeSet::new(), Vec::new());
        globals_of(values, coordinate, &mut seen, &mut globals);
        if let Some(w) = globals
            .into_iter()
            .find_map(|g| word(values, g, buffer).filter(|w| used.contains(w)))
        {
            return Err(format!(
                "the world position is made of the word {w}, which the clip position's rows or camera use"
            ));
        }
    }
    found.ok_or_else(|| "the program has no clip lane".into())
}

/// The constant buffer at register 4, space 0 of an interface.
pub(crate) fn pass_buffer(interface: &Interface) -> Option<ResourceId> {
    interface
        .resources
        .iter()
        .find(|r| {
            matches!(r.kind, ResourceKind::ConstantBuffer { .. })
                && r.provenance
                    .d3d
                    .is_some_and(|d| d.class == D3dClass::ConstantBuffer && d.register == 4 && d.space == 0)
        })
        .map(|r| r.id)
}

/// The rows and camera words of a vertex shader's clip position and the constant buffer (at register 4) they are words of,
/// in the shader's own interface: the program lifted, its value graph read ([`clip_rows`]).
pub fn shader_clip_rows(shader: &Shader) -> Result<(ResourceId, ClipRows), String> {
    if shader.interface.stage != Stage::Vertex {
        return Err("the shader is not a vertex program".into());
    }
    if shader.format != ShaderFormat::Dxil {
        return Err("the program is not DXIL".into());
    }
    let bound = pass_buffer(&shader.interface).ok_or("the program has no constant buffer at register 4")?;
    let program =
        primase_dxil::lift(&shader.bytecode).map_err(|e| format!("the program does not lift: {e}"))?;
    // the lifted program's own resource for the same binding
    let buffer =
        pass_buffer(&program.interface).ok_or("the lifted program has no constant buffer at register 4")?;
    let values = Values::of_program(&program).map_err(|e| format!("the value graph: {e}"))?;
    clip_rows(&values, &program.interface, buffer).map(|rows| (bound, rows))
}

/// States the roles of `rows` over the constants of the buffer `resource`: the sixteen row words
/// ([`ViewParameter::CameraRelativeViewProjection`], its lines its rows, as [`clip_rows`] found each lane of the clip
/// position to be one line's four words times the position) and the three camera words
/// ([`ViewParameter::CameraPosition`]), each cut out of the `Unknown` constants that cover those bytes. `false` when a
/// byte of either is not covered by `Unknown` constants alone (nothing is changed then).
pub fn state_clip_roles(constants: &mut Vec<ConstantBinding>, resource: ResourceId, rows: ClipRows) -> bool {
    let wanted = [
        (
            rows.rows * 4,
            ClipRows::ROWS_BYTES,
            ViewParameter::CameraRelativeViewProjection,
            Some(MatrixLines::Rows),
        ),
        (
            rows.camera * 4,
            ClipRows::CAMERA_BYTES,
            ViewParameter::CameraPosition,
            None,
        ),
    ];
    let mut cut = constants.clone();
    for (offset, size, role, lines) in wanted {
        if !cut_out(
            &mut cut,
            resource,
            (offset, size),
            ConstantRole::View(role),
            lines,
        ) {
            return false;
        }
    }
    cut.sort_by_key(|c| (c.resource.0, c.offset));
    *constants = cut;
    true
}

/// Replaces the bytes `offset..offset + size` (`at`) of the buffer `resource` in `constants` by one constant of `role`
/// (a matrix with its `lines`), the `Unknown` constants that cover them cut to what lies outside (a piece keeps the
/// reason, not the parameter's name).
pub(crate) fn cut_out(
    constants: &mut Vec<ConstantBinding>,
    resource: ResourceId,
    (offset, size): (u32, u32),
    role: ConstantRole,
    lines: Option<MatrixLines>,
) -> bool {
    let end = offset + size;
    constants.sort_by_key(|c| (c.resource.0, c.offset));
    let covering: Vec<usize> = (0..constants.len())
        .filter(|&i| {
            let c = &constants[i];
            c.resource == resource && c.offset < end && c.offset + c.size > offset
        })
        .collect();
    let all_unknown = covering
        .iter()
        .all(|&i| matches!(constants[i].role, ConstantRole::Unknown { .. }));
    let first = covering.first().map(|&i| constants[i].offset);
    let last = covering.last().map(|&i| constants[i].offset + constants[i].size);
    let gapless = covering
        .windows(2)
        .all(|p| constants[p[0]].offset + constants[p[0]].size == constants[p[1]].offset);
    if covering.is_empty()
        || !all_unknown
        || !gapless
        || first.is_none_or(|f| f > offset)
        || last.is_none_or(|l| l < end)
    {
        return false;
    }
    let mut pieces = Vec::new();
    for &i in &covering {
        let c = &constants[i];
        let piece = |from: u32, to: u32| ConstantBinding {
            resource,
            offset: from,
            size: to - from,
            name: None,
            role: c.role.clone(),
            lines: None,
        };
        if c.offset < offset {
            pieces.push(piece(c.offset, offset));
        }
        if c.offset + c.size > end {
            pieces.push(piece(end, c.offset + c.size));
        }
    }
    for &i in covering.iter().rev() {
        constants.remove(i);
    }
    constants.extend(pieces);
    constants.push(ConstantBinding {
        resource,
        offset,
        size,
        name: None,
        role,
        lines,
    });
    true
}
