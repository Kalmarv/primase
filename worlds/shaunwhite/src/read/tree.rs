//! A RealTreeMesh (`0xCD0D35E6`): the game's urban trees, a skeleton of branches and a set of leaves that its RealTree
//! library (Ubisoft's techgroup `RealTreeLib`: the trunk shader expands each node to a ring of vertices, the leaf shader
//! each leaf to a card) draws; the object stores no triangles. In level 01 there are three: `00_Veg_UrbanTree01`,
//! `00_Veg_UrbanTree01_Thin` and `00_Veg_UrbanTreeSmall01`, shown by 36 placements.
//!
//! The body (little-endian) is a managed header (`[01][id][class]`), the GraphicObject's Category (u32) and Generated
//! (u8), then the SubMeshes array: a u32 count (2) and per SubRealTreeMesh (class `0xFE8CCC18`) `[pointer 0][managed
//! 1][id][class]`, the Material id, a u32 0, the kind (0 the trunk, 1 the leaves), and a count of f32 distances (the
//! levels of detail: 6 or 5). Five zero bytes, the u32 `0x5E5E7E30`, the blob's size (u32), 154, 12345, 0 and more
//! fields follow, and the blob is the last `size` bytes of the body: a native memory image whose pointers are its
//! base address plus an offset in it. The base differs by level (`0x72300000` in level 01, `0x71040000` in 02,
//! `0x6FFA0000` in 03, `0x734D0000` or `0x73210000` in 04, `0x73000000` in 06, `0x757C0000` in the shared forge) and the
//! image stores it: three header words, at 0x154, 0x158 and 0x280, point at the offsets 0x3B0, 0x390 and 0x3D0 of the
//! image itself, in all 18 distinct trees of the game's cells (nine forges). [`Blob::new`] takes the base from them and refuses a blob whose
//! three disagree.
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
//!
//! A leaf is a card of four vertices. Its record `C` (92 bytes): two opposite vectors (3 x f32 each, not read: the
//! shader's far view), a byte for its kind (0 to 3: an entry of the kind table), a byte 100 (the leaf shader's "do not
//! turn to face the camera" scale code), two bytes not read, a permutation of 0 to 3 in four bytes (the UV corner of
//! each vertex), a byte 0, a size in metres (f32), 0, a scale (f32: 1 in `00_Veg_UrbanTree01_Thin`, 1.5 in
//! `00_Veg_UrbanTree01`, 0.76 to 1.4 in `00_Veg_UrbanTreeSmall01`) and four vectors (3 x f32 each), the vertices'
//! offsets from the leaf's centre in units of the size: the shader computes `centre + size x vector` (`RegenScaling x
//! CtrToVertexDist x (CtrToVertexDir x 2 - 1)` with the scale at 1 in the near view). In every tree the four points
//! are coplanar (median tetrahedron volume 0.001 to 0.003 of the scale cubed over all leaves; `v0` and `v2` lie on
//! one ray from the centre, `v2` at the scale times `v0`'s distance), so a card is a quad; its order round the quad is
//! found from the angles about its centre. Record `E`: its centre at 4. A leaf with no size, a scale outside 0.5 to 2
//! (the last leaf of each tree: 0.05, 0.25) or a vector longer than 2 (16 of `00_Veg_UrbanTreeSmall01`'s: corners of
//! a cube and one of 6.9 or 9.0, a clamped value) or a centre more than its reach (its longest vector and 0.5 m) outside
//! the box of the nodes (the second-last leaf of two trees, at the origin) is not a card and is counted.
//! The kind table, in the blob's header from 0x300: per kind three or four UV corners (u, v) of the leaf atlas, then
//! three 0.502 (the vertex colour), then a code (5 or 100 to 104): a kind with a code of 100 or more has three
//! corners (the fourth completes the parallelogram), the others four. In `00_Veg_TreeLeavesA`'s atlas (256 x 256)
//! kinds 0 to 2 are the big branch (u 0 to 0.8, v 0 to 1) and kind 3 a bush (u 0.81 to 0.98, v 0.80 to 1).
//!
//! The leaves are the *restored* world's: the leaf shader (`leaves.fx`) clips every pixel where the world's influence is
//! 0, and the game before the Ministry's influence is lifted shows the sibling Mesh `00_Veg_RealTree_UrbanTree`, a
//! metal tree with light strips, instead. [`Tree::cards`] makes them with both faces.
//!
//! **The leaves' shading normal.** The leaf shader (`leaves.fx`) lights a leaf by no normal of its own: its vertex
//! normal is `normalize(position - DensityBoundingSphere.xyz)` with the result's z raised to at least 0.6 and the
//! vector normalised again, a crown lit as one rounded, upward-tilted body, the same on both faces of a card. The
//! sphere is the one the blob's header holds at 0x1d4 (centre x y z, then a radius): in all five distinct trees of the
//! game (level 01's three, level 03's palm, level 04's cherry) its centre lies within 0.07 to 0.55 m of the leaves'
//! centroid, 1.5 m in the cherry's (a crown 8 m across), so it is read as the density sphere; its radius is not used.
//! [`Tree::cards`] gives each vertex that normal, the back face's negated (a double-sided material lights a back face
//! with the opposite of the normal its vertices carry, so the two faces shade alike).
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
/// The header words that point at the image itself, and the offset each points at: they give the image's base address.
const SELF_POINTERS: [(usize, u32); 3] = [(0x154, 0x3B0), (0x158, 0x390), (0x280, 0x3D0)];
/// The header offset of the leaves' density sphere: a centre (3 x f32) and a radius.
const CROWN: usize = 0x1d4;
/// The least z of a leaf's shading normal (the leaf shader's `max(0.6, normal.z)`).
const CROWN_NORMAL_Z: f32 = 0.6;
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
    /// The leaf kinds' UV corners.
    pub kinds: Vec<LeafKind>,
    /// The leaves that make a card, in the blob's order.
    pub leaves: Vec<Leaf>,
    /// Leaves with no card (no size, a scale outside 0.5 to 2, a vertex vector longer than 2, or a centre far from every
    /// node).
    pub unusable_leaves: usize,
    /// The centre of the leaves' density sphere, metres, the tree's own frame: the leaf shader's shading normals point
    /// away from it.
    pub crown: [f32; 3],
}

/// A leaf card (the module documentation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leaf {
    pub center: [f32; 3],
    /// Each vertex's offset from the centre, metres.
    pub offsets: [[f32; 3]; 4],
    /// Index into [`Tree::kinds`].
    pub kind: usize,
    /// The UV corner (0 to 3) of each vertex.
    pub corners: [u8; 4],
}

/// A leaf kind's UV corners in the leaf atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeafKind {
    pub corners: [[f32; 2]; 4],
}

struct Blob<'a> {
    v: View<'a>,
    len: usize,
    /// The address the image was built at: its pointers are this plus an offset.
    base: u32,
}

impl<'a> Blob<'a> {
    /// The blob `v` of `len` bytes, its base the address its own pointers ([`SELF_POINTERS`]) give.
    fn new(v: View<'a>, len: usize) -> Result<Self, Error> {
        let mut base = None;
        for (at, offset) in SELF_POINTERS {
            let b = v
                .u32(at, LE)
                .field("image pointer")?
                .checked_sub(offset)
                .ok_or_else(|| {
                    invalid(
                        "base",
                        format!("the pointer at {at:#x} is below its target {offset:#x}"),
                    )
                })?;
            if base.is_some_and(|first| first != b) {
                return Err(invalid(
                    "base",
                    "the header's pointers to the image itself disagree",
                ));
            }
            base = Some(b);
        }
        let base = base.ok_or_else(|| invalid("base", "no pointer to the image itself"))?;
        Ok(Blob { v, len, base })
    }
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
            .checked_sub(self.base)
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
        let blob = Blob::new(View::new(body.get(start..).unwrap_or_default()), size)?;
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
        // the box of the nodes
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for node in branches.values().flatten() {
            for ((l, h), v) in lo.iter_mut().zip(hi.iter_mut()).zip(node.position) {
                *l = l.min(v);
                *h = h.max(v);
            }
        }
        let kinds = leaf_kinds(&blob)?;
        let crown = [
            blob.float(CROWN, "crown centre")?,
            blob.float(CROWN.saturating_add(4), "crown centre")?,
            blob.float(CROWN.saturating_add(8), "crown centre")?,
        ];
        let mut leaves = Vec::new();
        let mut unusable_leaves = 0usize;
        for k in 0..l {
            let rec = c.saturating_add(k.saturating_mul(92));
            let at = |o: usize| rec.saturating_add(o);
            let vec3 = |o: usize, what: &'static str| -> Result<[f32; 3], Error> {
                Ok([
                    blob.float(at(o), what)?,
                    blob.float(at(o).saturating_add(4), what)?,
                    blob.float(at(o).saturating_add(8), what)?,
                ])
            };
            let size = blob.float(at(32), "leaf size")?;
            let scale = blob.float(at(40), "leaf scale")?;
            let mut vectors = [[0f32; 3]; 4];
            for (i, v) in vectors.iter_mut().enumerate() {
                *v = vec3(44usize.saturating_add(i.saturating_mul(12)), "leaf vertex")?;
            }
            let bytes = blob.v.slice(at(24), 8).field("leaf bytes")?;
            let kind = usize::from(bytes.first().copied().unwrap_or(u8::MAX));
            let mut corners = [0u8; 4];
            for (i, c) in corners.iter_mut().enumerate() {
                *c = bytes.get(3usize.saturating_add(i)).copied().unwrap_or(u8::MAX);
            }
            let mut seen = [false; 4];
            for &c in &corners {
                if let Some(s) = seen.get_mut(usize::from(c)) {
                    *s = true;
                }
            }
            if kind >= kinds.len() || !seen.iter().all(|&s| s) {
                return Err(invalid(
                    "leaf",
                    format!("leaf {k}: kind {kind} of {}, corners {corners:?}", kinds.len()),
                ));
            }
            let longest = vectors
                .iter()
                .map(|v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt())
                .fold(0f32, f32::max);
            if size <= 0. || !(0.5..=2.).contains(&scale) || longest > 2. {
                unusable_leaves = unusable_leaves.saturating_add(1);
                continue;
            }
            let offsets = vectors.map(|v| v.map(|x| x * size));
            let ec = e.saturating_add(k.saturating_mul(32));
            let center = [
                blob.float(ec.saturating_add(4), "leaf centre")?,
                blob.float(ec.saturating_add(8), "leaf centre")?,
                blob.float(ec.saturating_add(12), "leaf centre")?,
            ];
            // a leaf hangs near its branches: the second-last leaf of two trees sits at the origin, 2.6 m under the
            // lowest node, and is a sentinel
            let reach = offsets
                .iter()
                .map(|v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt())
                .fold(0f32, f32::max)
                + 0.5;
            let near = center
                .iter()
                .zip(lo.iter().zip(&hi))
                .all(|(c, (l, h))| *c >= l - reach && *c <= h + reach);
            if !near {
                unusable_leaves = unusable_leaves.saturating_add(1);
                continue;
            }
            leaves.push(Leaf {
                center,
                offsets,
                kind,
                corners,
            });
        }
        Ok(Tree {
            trunk_material,
            leaf_material,
            branches: branches.into_values().collect(),
            dropped,
            kinds,
            leaves,
            unusable_leaves,
            crown,
        })
    }

    /// The leaves as one triangle-list primitive in the tree's own frame: per card four vertices (the centre plus the
    /// offsets, the kind's UV corners by the card's permutation, the crown's shading normal [`Tree::crown_normal`]) and
    /// two triangles taken round the quad, and the same four again reversed with the normal negated, so that a
    /// one-sided material shows both faces and a double-sided one shades them alike.
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "vertex indices: counts of one tree's leaves times eight, far below u32's range"
    )]
    pub fn cards(&self, material: Option<primase_core::Entity>) -> Primitive {
        let mut position = Vec::new();
        let mut normal = Vec::new();
        let mut uv = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for leaf in &self.leaves {
            let kind = self.kinds.get(leaf.kind);
            let o = leaf.offsets;
            // the quad's plane, for the winding: its diagonals' normal
            let n = unit(cross(sub(o[2], o[0]), sub(o[3], o[1]))).unwrap_or([0., 0., 1.]);
            // the vertices round the quad, by the angle about their mean in the plane
            let mean = o.iter().fold([0f32; 3], |a, &v| add3(a, v)).map(|x| x / 4.);
            let u = unit(sub(o[0], mean)).unwrap_or([1., 0., 0.]);
            let w = cross(n, u);
            let mut order = [0usize, 1, 2, 3];
            let angle = |i: usize| {
                let d = sub(o.get(i).copied().unwrap_or(mean), mean);
                (d[0] * w[0] + d[1] * w[1] + d[2] * w[2]).atan2(d[0] * u[0] + d[1] * u[1] + d[2] * u[2])
            };
            order.sort_by(|&a, &b| angle(a).total_cmp(&angle(b)));
            for back in [false, true] {
                let first = u32::try_from(position.len()).unwrap_or(u32::MAX);
                for (i, off) in o.iter().enumerate() {
                    let at = add3(leaf.center, *off);
                    position.push(at);
                    let shade = self.crown_normal(at);
                    normal.push(if back {
                        [-shade[0], -shade[1], -shade[2]]
                    } else {
                        shade
                    });
                    let corner = usize::from(leaf.corners.get(i).copied().unwrap_or(0));
                    uv.push(
                        kind.and_then(|k| k.corners.get(corner))
                            .copied()
                            .unwrap_or([0., 0.]),
                    );
                }
                let q = |j: usize| first + u32::try_from(order.get(j).copied().unwrap_or(0)).unwrap_or(0);
                if back {
                    indices.extend_from_slice(&[q(0), q(2), q(1), q(0), q(3), q(2)]);
                } else {
                    indices.extend_from_slice(&[q(0), q(1), q(2), q(0), q(2), q(3)]);
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

    /// The leaf shader's shading normal at `at` (the tree's own frame): away from the [`crown`](Tree::crown) with its z raised to
    /// [`CROWN_NORMAL_Z`] and the vector normalised again (unit length); straight up at the centre itself.
    pub fn crown_normal(&self, at: [f32; 3]) -> [f32; 3] {
        let mut out = unit(sub(at, self.crown)).unwrap_or([0., 0., 1.]);
        out[2] = out[2].max(CROWN_NORMAL_Z);
        unit(out).unwrap_or([0., 0., 1.])
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

/// The kind table: each run of three words `0x3F008081` (0.502) in the blob's header from 0x300 to 0x4A0 ends one kind's
/// UV corners, which the words before it hold ([`LeafKind`]).
fn leaf_kinds(blob: &Blob<'_>) -> Result<Vec<LeafKind>, Error> {
    const GREY: u32 = 0x3F00_8081;
    let word = |at: usize| blob.word(at, "leaf kind table");
    let mut kinds = Vec::new();
    let mut at = 0x300usize;
    while at < 0x4A0 {
        if word(at)? == GREY && word(at.saturating_add(4))? == GREY && word(at.saturating_add(8))? == GREY {
            let code = blob.float(at.saturating_add(12), "leaf kind code")?;
            let count = if code >= 100. { 6usize } else { 8 };
            let mut uv = Vec::new();
            for k in 0..count {
                let back = count.saturating_sub(k).saturating_mul(4);
                let v = blob.float(at.saturating_sub(back), "leaf kind UV")?;
                if !(0.0..=1.0).contains(&v) {
                    return Err(invalid("leaf kind UV", format!("{v} outside 0 to 1")));
                }
                uv.push(v);
            }
            let pair = |i: usize| {
                let j = i.saturating_mul(2);
                [
                    uv.get(j).copied().unwrap_or(0.),
                    uv.get(j.saturating_add(1)).copied().unwrap_or(0.),
                ]
            };
            let corners = if count == 6 {
                let (c0, c1, c2) = (pair(0), pair(1), pair(2));
                [c0, c1, c2, [c1[0] + c2[0] - c0[0], c1[1] + c2[1] - c0[1]]]
            } else {
                [pair(0), pair(1), pair(2), pair(3)]
            };
            kinds.push(LeafKind { corners });
            at = at.saturating_add(16);
        } else {
            at = at.saturating_add(4);
        }
    }
    if kinds.is_empty() {
        return Err(invalid("leaf kind table", "no kind found"));
    }
    Ok(kinds)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
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
