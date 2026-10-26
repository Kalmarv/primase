//! A RealTreeMesh (`0xCD0D35E6`): the game's urban trees, a skeleton of branches and a set of leaves that its RealTree
//! library (Ubisoft's techgroup `RealTreeLib`: the trunk shader expands each node to a ring of vertices, the leaf shader
//! each leaf to a card) draws; the object stores no triangles. In level 01 there are three: `00_Veg_UrbanTree01`,
//! `00_Veg_UrbanTree01_Thin` and `00_Veg_UrbanTreeSmall01`, shown by 36 placements.
//!
//! The body (little-endian) is a managed header (`[01][id][class]`), the GraphicObject's Category (u32) and Generated
//! (u8), then the SubMeshes array: a u32 count (2) and per SubRealTreeMesh (class `0xFE8CCC18`) `[pointer 0][managed
//! 1][id][class]`, the Material id, a u32 0, the kind (0 the trunk, 1 the leaves), and a count of f32 distances (the
//! levels of detail: 6 or 5). Five zero bytes, the u32 `0x5E5E7E30`, the blob's size (u32), 154, 12345, 0 and more
//! fields follow, and the blob is the last `size` bytes of the body: a native memory image whose pointers are
//! `0x72300000` plus an offset in it.
//!
//! The blob's header, at 0x100: `[0][N][A][B]`, `[M][..][..][L]`, `[C]`, with `N` nodes, `M` branches (the trunk
//! the first) and `L` leaves; at 0x350 `[0][N][L][0]`, `[0][F][G][0]`, `[0][N][L][0]`, `[R][E][0][0]`; each a pointer or a count.
//! The arrays, each pointed to and in memory in this order (`A`, `B`, `C` back to back; `F`, `G`, `R`, `E` one after
//! another, each within 15 bytes of the last one's end): `A`, `N` records of 80 bytes: a node's position at 20 (3 x f32) and,
//! at 72, a u32 whose low half is its branch's number and whose high half says what it is (0 an end, 2 or 3 where a
//! branch leaves it, 1 a node a branch will leave); `B`, `N` x 32; `C`, `L` x 92 (a leaf: two opposite directions, the
//! half-diagonal, normals); `F`, `N` x 72; `G`, `L` x 24; `R`, `N` x 32 (record `k`: the position of node `k + 1`, a
//! radius at 16 and a direction at 20); `E`, `L` x 32 (a leaf's centre at 4 and its scale at 16).
//!
//! A branch is the nodes with its number, which are consecutive; its first node lies where its parent's node is. Node
//! `k`'s radius is record `k - 1`'s (the render record keeps each segment's far end); node 0 has record 0's. A tip's
//! radius is 0.01 and a branch starts as thick as its parent there. The last record of `00_Veg_UrbanTree01_Thin` has the
//! branch number 65376 and no meaning (its 114 nodes are 113 of 35 branches and this one): a node of a branch past
//! the last is dropped. The trunk's first node is where the stem begins: the trees stand on the pole of their sibling Mesh, so the
//! nodes do not reach the ground in `00_Veg_UrbanTree01_Thin` (the trunk starts at 2.63 m) and
//! `00_Veg_UrbanTree01` (3.5 m), and do in `00_Veg_UrbanTreeSmall01` (-0.26 m).
//!
//! [`Tree::tubes`] builds the trunk and branches as tapering rings of [`RING`] vertices at every node, joined along
//! each branch.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::collections::BTreeMap;
use std::f32::consts::TAU;

use primase_bytes::{Endian, Field, FieldError, View};
use primase_core::{Primitive, Semantic, Stream, Topology};

const LE: Endian = Endian::Little;
pub const REAL_TREE_MESH: u32 = 0xCD0D_35E6;
pub const SUB_REAL_TREE_MESH: u32 = 0xFE8C_CC18;
const BLOB_MARKER: u32 = 0x5E5E_7E30;
/// The offset a blob's pointers count from.
const BASE: u32 = 0x7230_0000;
/// Vertices of a branch's ring.
pub const RING: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Field(#[from] FieldError),
    #[error("RealTreeMesh {field}: {why}")]
    Invalid { field: &'static str, why: String },
}

fn invalid(field: &'static str, why: impl Into<String>) -> Error {
    Error::Invalid {
        field,
        why: why.into(),
    }
}

/// A node of a branch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    /// Metres, the tree's own frame (z up).
    pub position: [f32; 3],
    pub radius: f32,
}

/// A RealTreeMesh as read (the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct Tree {
    /// The trunk's Material id (sub-mesh kind 0).
    pub trunk_material: u32,
    /// The leaves' Material id (sub-mesh kind 1).
    pub leaf_material: u32,
    /// The trunk first, then the other branches in the order of their numbers.
    pub branches: Vec<Vec<Node>>,
    /// Nodes of a branch past the last (dropped).
    pub dropped: usize,
    /// How many leaves the tree has.
    pub leaves: usize,
}

struct Blob<'a> {
    v: View<'a>,
    len: usize,
}

impl Blob<'_> {
    fn word(&self, at: usize, field: &'static str) -> Result<u32, Error> {
        Ok(self.v.u32(at, LE).field(field)?)
    }
    fn count(&self, at: usize, field: &'static str) -> Result<usize, Error> {
        usize::try_from(self.word(at, field)?).map_err(|_| invalid(field, "too large"))
    }
    /// The array pointed to at `at`: its offset, with `n` records of `size` bytes inside the blob.
    fn array(&self, at: usize, n: usize, size: usize, field: &'static str) -> Result<usize, Error> {
        let p = self
            .word(at, field)?
            .checked_sub(BASE)
            .and_then(|o| usize::try_from(o).ok())
            .ok_or_else(|| invalid(field, "a pointer outside the blob's base"))?;
        n.checked_mul(size)
            .and_then(|b| p.checked_add(b))
            .filter(|&e| e <= self.len)
            .ok_or_else(|| {
                invalid(
                    field,
                    format!("{n} records of {size} bytes at {p:#x} leave the blob"),
                )
            })?;
        Ok(p)
    }
    fn float(&self, at: usize, field: &'static str) -> Result<f32, Error> {
        let f = self.v.f32(at, LE).field(field)?;
        if f.is_finite() {
            Ok(f)
        } else {
            Err(invalid(field, "a non-finite float"))
        }
    }
}

impl Tree {
    /// The RealTreeMesh object body `body`.
    pub fn parse(body: &[u8]) -> Result<Tree, Error> {
        let v = View::new(body);
        let word = |at: usize, field: &'static str| -> Result<u32, Error> { Ok(v.u32(at, LE).field(field)?) };
        if word(5, "class")? != REAL_TREE_MESH {
            return Err(invalid("class", "not a RealTreeMesh"));
        }
        // GraphicObject: Category u32 at 9, Generated u8 at 13; the SubMeshes at 14
        let subs = word(14, "SubMeshes count")?;
        let mut at = 18usize;
        let (mut trunk, mut leaf) = (None, None);
        for _ in 0..subs {
            if v.slice(at, 2).field("SubMesh header")? != [0, 1] {
                return Err(invalid("SubMesh header", "not an inline managed object"));
            }
            if word(at.saturating_add(6), "SubMesh class")? != SUB_REAL_TREE_MESH {
                return Err(invalid("SubMesh class", "not a SubRealTreeMesh"));
            }
            let material = word(at.saturating_add(10), "SubMesh material")?;
            let kind = word(at.saturating_add(18), "SubMesh kind")?;
            let n = usize::try_from(word(at.saturating_add(22), "SubMesh distance count")?)
                .map_err(|_| invalid("SubMesh distance count", "too large"))?;
            match kind {
                0 => trunk = Some(material),
                1 => leaf = Some(material),
                k => return Err(invalid("SubMesh kind", format!("{k}, not 0 or 1"))),
            }
            at = at.saturating_add(26).saturating_add(
                n.checked_mul(4)
                    .ok_or_else(|| invalid("SubMesh distances", "too many"))?,
            );
        }
        let (Some(trunk_material), Some(leaf_material)) = (trunk, leaf) else {
            return Err(invalid("SubMeshes", "no trunk and leaf sub-mesh"));
        };
        // five zero bytes, the marker, the blob's size, 154, 12345
        if v.slice(at, 5).field("header gap")? != [0; 5]
            || word(at.saturating_add(5), "marker")? != BLOB_MARKER
            || word(at.saturating_add(13), "header size")? != 154
            || word(at.saturating_add(17), "magic")? != 12345
        {
            return Err(invalid("blob header", "not the RealTree header read"));
        }
        let size = usize::try_from(word(at.saturating_add(9), "blob size")?)
            .map_err(|_| invalid("blob size", "too large"))?;
        let start = body
            .len()
            .checked_sub(size)
            .filter(|&s| s >= at.saturating_add(21))
            .ok_or_else(|| invalid("blob size", format!("{size} bytes of a body of {}", body.len())))?;
        let blob = Blob {
            v: View::new(body.get(start..).unwrap_or_default()),
            len: size,
        };
        let n = blob.count(0x104, "node count")?;
        let m = blob.count(0x110, "branch count")?;
        let l = blob.count(0x11c, "leaf count")?;
        if blob.count(0x354, "node count")? != n || blob.count(0x358, "leaf count")? != l {
            return Err(invalid("counts", "the header's counts disagree"));
        }
        let a = blob.array(0x108, n, 80, "node array")?;
        let b = blob.array(0x10c, n, 32, "node array B")?;
        let c = blob.array(0x120, l, 92, "leaf array")?;
        let f = blob.array(0x364, n, 72, "node array F")?;
        let g = blob.array(0x368, l, 24, "leaf array G")?;
        let r = blob.array(0x380, n, 32, "render node array")?;
        let e = blob.array(0x384, l, 32, "leaf centre array")?;
        // the arrays lie one after another, the later ones padded to 16 bytes
        for (what, got, want) in [
            ("B after A", b, a.saturating_add(n.saturating_mul(80))),
            ("C after B", c, b.saturating_add(n.saturating_mul(32))),
        ] {
            if got != want {
                return Err(invalid("arrays", format!("{what}: {got:#x}, not {want:#x}")));
            }
        }
        for (what, got, from) in [
            ("G after F", g, f.saturating_add(n.saturating_mul(72))),
            ("R after G", r, g.saturating_add(l.saturating_mul(24))),
            ("E after R", e, r.saturating_add(n.saturating_mul(32))),
        ] {
            if got < from || got.saturating_sub(from) > 15 {
                return Err(invalid("arrays", format!("{what}: {got:#x} after {from:#x}")));
            }
        }
        // the nodes, in branches
        let mut nodes = Vec::new();
        for k in 0..n {
            let rec = a.saturating_add(k.saturating_mul(80));
            let mut position = [0f32; 3];
            for (j, p) in position.iter_mut().enumerate() {
                *p = blob.float(
                    rec.saturating_add(20).saturating_add(j.saturating_mul(4)),
                    "node position",
                )?;
            }
            let id = blob.word(rec.saturating_add(72), "node branch")?;
            // radius: the render record before this one, the first node's own
            let rr = r.saturating_add(k.saturating_sub(1).saturating_mul(32));
            let radius = blob.float(rr.saturating_add(16), "node radius")?;
            nodes.push((id & 0xFFFF, Node { position, radius }));
        }
        let mut branches: BTreeMap<u32, Vec<Node>> = BTreeMap::new();
        let mut dropped = 0usize;
        let mut last = None;
        for (id, node) in nodes {
            if usize::try_from(id).map_or(true, |i| i >= m) {
                dropped = dropped.saturating_add(1);
                continue;
            }
            if last.is_some_and(|l| l != id) && branches.contains_key(&id) {
                return Err(invalid("nodes", format!("branch {id} is not consecutive")));
            }
            last = Some(id);
            branches.entry(id).or_default().push(node);
        }
        if branches.len() != m {
            return Err(invalid(
                "branches",
                format!("{} branches, the header says {m}", branches.len()),
            ));
        }
        Ok(Tree {
            trunk_material,
            leaf_material,
            branches: branches.into_values().collect(),
            dropped,
            leaves: l,
        })
    }

    /// The trunk and branches as one triangle-list primitive in the tree's own frame: a ring of [`RING`] vertices
    /// of the node's radius around every node, perpendicular to the branch's direction there (the neighbours'
    /// difference), joined to the next node's ring; normals radial; texture coordinates u around the ring, v the
    /// branch's length from its start divided by the ring's circumference at the start (a stand-in: the game's
    /// trunk texture mapping is not read).
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "ring and branch vertex indices: counts of one tree's nodes times the ring, far below u32's range"
    )]
    pub fn tubes(&self, material: Option<primase_core::Entity>) -> Primitive {
        let mut position = Vec::new();
        let mut normal = Vec::new();
        let mut uv = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for branch in &self.branches {
            let first = u32::try_from(position.len()).unwrap_or(u32::MAX);
            let mut along = 0f32;
            for (j, node) in branch.iter().enumerate() {
                let prev = branch.get(j.saturating_sub(1)).unwrap_or(node);
                let next = branch.get(j.saturating_add(1)).unwrap_or(node);
                if j > 0 {
                    along += distance(prev.position, node.position);
                }
                let dir = unit(sub(next.position, prev.position)).unwrap_or([0., 0., 1.]);
                let reference = if dir[0].abs() < 0.9 {
                    [1., 0., 0.]
                } else {
                    [0., 1., 0.]
                };
                let u = unit(cross(dir, reference)).unwrap_or([1., 0., 0.]);
                let v = cross(dir, u);
                let circumference = TAU * branch.first().map_or(0.01, |n| n.radius).max(0.01);
                for s in 0..RING {
                    let angle = TAU * s as f32 / RING as f32;
                    let (sin, cos) = angle.sin_cos();
                    let out = [
                        u[0] * cos + v[0] * sin,
                        u[1] * cos + v[1] * sin,
                        u[2] * cos + v[2] * sin,
                    ];
                    position.push([
                        node.position[0] + out[0] * node.radius,
                        node.position[1] + out[1] * node.radius,
                        node.position[2] + out[2] * node.radius,
                    ]);
                    normal.push(out);
                    uv.push([s as f32 / RING as f32, along / circumference]);
                }
            }
            let ring = u32::try_from(RING).unwrap_or(0);
            for j in 1..u32::try_from(branch.len()).unwrap_or(0) {
                let (r0, r1) = (first + (j - 1) * ring, first + j * ring);
                for s in 0..ring {
                    let t = (s + 1) % ring;
                    indices.extend_from_slice(&[r0 + s, r0 + t, r1 + t, r0 + s, r1 + t, r1 + s]);
                }
            }
        }
        Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([
                (Semantic::Position, Stream::F32x3(position)),
                (Semantic::Normal, Stream::F32x3(normal)),
                (Semantic::TexCoord(0), Stream::F32x2(uv)),
            ]),
            indices: Some(indices),
            material,
        }
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (n > 1e-9).then(|| [v[0] / n, v[1] / n, v[2] / n])
}
