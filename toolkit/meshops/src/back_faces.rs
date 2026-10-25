//! `add-back-faces` (no options): every triangle primitive drawn with a double-sided `pbr.material` gets a back face for
//! each triangle and is then drawn with a single-sided one, for a destination that culls back faces and has no
//! double-sided flag (`pack-texture-atlas` refuses materials that differ in double-sidedness, and `set-material-modes`
//! only makes the flag false, which would lose the back).
//!
//! **Which primitives.** Those with a triangle-list topology whose materials are double-sided: the primitive's own and
//! every per-placement variant of it ([`MaterialVariants`]); they must all agree, else the pass is refused, naming the
//! primitive (the back faces cannot be drawn for one variant and not another). A primitive with no material, one with a
//! single-sided material and one that is not a triangle list are left as they are and counted. A material without a
//! `pbr.material` is refused, naming it: its sidedness is not known.
//!
//! **The copy.** For a primitive of `n` vertices and `t` triangles `(a, b, c)`, the vertices the triangles use are
//! copied once each and appended after the `n` (in the order the triangles first use them), and every triangle is
//! followed by its reverse `(a', c', b')` on the copies, so a front face and its back face stay next to each other (a
//! later split of the primitive by triangles, `group-primitives`, keeps them in one section). A primitive without
//! indices gets indices. Every stream of a copy is the source vertex's, except the shading frame:
//!
//! - **normal** negated;
//! - **tangent**: xyz negated and w negated.
//!
//! The rule is the one a double-sided material is lit by (glTF, and the Khronos sample renderer: a back-facing
//! fragment negates its tangent, bitangent and normal): the back face shades with the opposite of the normal the
//! front's texel gives, `-(x T + y B + z N)`. The surface is the same sheet seen from behind, so its relief is the same
//! relief and its normal is the front's reversed. With the bitangent `B = w (N x T)`, negating `N` and `T` alone would
//! keep `B` (the texel would give `x T + y B - z N`, the front's normal mirrored in the sheet: a different relief);
//! negating `w` as well reverses `B` with them. A tangent's `w` is not otherwise touched, and a normal map is not
//! touched at all. A primitive with no normal stream has none to negate (the destination derives its normal from the
//! winding, which the reverse has already turned); a tangent stream that is not four floats is refused, as is a normal
//! stream that is not three, naming the primitive.
//!
//! Every other stream (positions, texture coordinates, colours, joints, weights) is copied as it is, so a skinned
//! back face deforms as its front does. Per-vertex components of the entity follow through
//! [`primase_core::World::remap_vertices`]: the copies hold their source vertices' data. Morph targets are such a
//! component: a copy's position deltas are its source's and its normal and tangent deltas are negated, so the morphed
//! frame is the negation of the front's morphed frame. Any other per-vertex component that cannot follow, or an opaque
//! one, refuses the pass naming it, before anything changes.
//!
//! **No offset.** The back face lies exactly on the front face, not displaced along the normal. A single-sided
//! destination culls the face turned away from the viewer, and of two coincident triangles of opposite winding exactly
//! one is turned away from any viewpoint, so they never compete for a pixel and cannot fight in depth. A depth
//! pre-pass or shadow map that draws both draws the same depth twice. An offset would only open a visible gap at every
//! edge, so it is not an option.
//!
//! **A triangle that already has its reverse** (a triangle whose three positions, bit for bit, are a rotation of the
//! reverse of another triangle's: a source that stores its own back faces) is not copied, and nor is one with two
//! coincident corners (it draws nothing); both are counted.
//!
//! The materials of the primitives given back faces are made single-sided (each reported). The pass is exact: the
//! surface drawn from either side is what the double-sided material drew. The file link of a changed mesh goes.
//!
//! Reported: primitives given back faces, triangles and vertices added, triangles left (already reversed, or
//! coincident corners), primitives left and why, materials made single-sided.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::morphs::Morphs;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Entity, MaterialVariants, Mesh, MeshFile, Name, Primitive, Semantic, Stream, Topology, World,
};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AddBackFaces;

/// `add-back-faces`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, which primitives get back
/// faces is their materials' double-sidedness and the rest is fixed.
pub const ADD_BACK_FACES: PassSpec = PassSpec {
    name: "add-back-faces",
    options: &[],
    make: |_| Ok(Box::new(AddBackFaces)),
};

/// A primitive's triangles, `indices` as stored: its indices, else its vertices in order.
fn corners(p: &Primitive) -> Vec<u32> {
    match &p.indices {
        Some(ix) => ix.clone(),
        None => (0..p.vertex_count() as u32).collect(),
    }
}

/// A triangle's positions as bits, rotated so the smallest comes first: equal for rotations of one triangle.
fn cycle_key(c: [[u32; 3]; 3]) -> [[u32; 3]; 3] {
    let first = (0..3).min_by_key(|&k| c[k]).expect("three corners");
    [c[first], c[(first + 1) % 3], c[(first + 2) % 3]]
}

/// The materials `e`'s primitive `i` is drawn with (its own and its variants').
fn drawn_with(w: &World, e: Entity, i: usize, p: &Primitive) -> BTreeSet<Entity> {
    let mut ms: BTreeSet<Entity> = p.material.into_iter().collect();
    if let Some(v) = w.get::<MaterialVariants>(e).and_then(|v| v.0.get(i)) {
        ms.extend(v.values().copied());
    }
    ms
}

/// What a primitive is: its entity (by name when it has one) and index.
fn describe(w: &World, e: Entity, i: usize) -> String {
    match w.get::<Name>(e) {
        Some(n) => format!("{e:?} {:?} primitive {i}", n.0),
        None => format!("{e:?} primitive {i}"),
    }
}

/// The back-faced primitive: the streams and indices, the source vertex of each appended vertex and the counts.
struct Backed {
    streams: BTreeMap<Semantic, Stream>,
    indices: Vec<u32>,
    /// New vertex `n + k` is source vertex `gather[k]`.
    gather: Vec<u32>,
    triangles_added: u64,
    already_reversed: u64,
    coincident: u64,
}

/// Append the copies of `gather` to `s`, and negate them (xyz of each, and w of a tangent) when `negate`.
fn append(s: &mut Stream, gather: &[u32], negate: Negate) {
    macro_rules! copy {
        ($v:expr) => {{
            let copies: Vec<_> = gather.iter().map(|&g| $v[g as usize]).collect();
            $v.extend(copies);
        }};
    }
    let n = s.len();
    match s {
        Stream::F32x2(v) => copy!(v),
        Stream::F32x3(v) => {
            copy!(v);
            if matches!(negate, Negate::Normal) {
                for x in &mut v[n..] {
                    *x = x.map(|c| -c);
                }
            }
        }
        Stream::F32x4(v) => {
            copy!(v);
            if matches!(negate, Negate::Tangent) {
                for x in &mut v[n..] {
                    *x = x.map(|c| -c);
                }
            }
        }
        Stream::U8x4(v) => copy!(v),
        Stream::U16x4(v) => copy!(v),
    }
}

#[derive(Clone, Copy)]
enum Negate {
    No,
    Normal,
    Tangent,
}

fn back_faces(p: &Primitive, what: &str) -> Result<Backed, String> {
    let n = p.vertex_count();
    let Some(positions) = p.positions() else {
        return Err(format!("{what}: no float3 positions"));
    };
    if let Some((sem, _)) = p.streams.iter().find(|(_, s)| s.len() != n) {
        return Err(format!("{what}: stream {sem:?} is not {n} vertices long"));
    }
    if n > u32::MAX as usize / 2 {
        return Err(format!(
            "{what}: {n} vertices cannot be doubled in 32-bit indices"
        ));
    }
    for (sem, want) in p.streams.keys().filter_map(|s| match s {
        Semantic::Normal => Some((*s, "three floats")),
        Semantic::Tangent(_) => Some((*s, "four floats")),
        _ => None,
    }) {
        if let Some(s) = p.streams.get(&sem)
            && !matches!(
                (sem, s),
                (Semantic::Normal, Stream::F32x3(_)) | (Semantic::Tangent(_), Stream::F32x4(_))
            )
        {
            return Err(format!("{what}: {sem:?} is not {want}"));
        }
    }
    let source = corners(p);
    if !source.len().is_multiple_of(3) {
        return Err(format!(
            "{what}: {} indices are not whole triangles",
            source.len()
        ));
    }
    if source.iter().any(|&v| v as usize >= n) {
        return Err(format!("{what}: an index past its {n} vertices"));
    }
    let bits = |v: u32| positions[v as usize].map(f32::to_bits);
    // the reverses the source already has: each triangle's key, then its reverse looked up
    let keys: BTreeSet<[[u32; 3]; 3]> = source
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| cycle_key(t.map(bits)))
        .collect();
    let mut copy_of: BTreeMap<u32, u32> = BTreeMap::new();
    let mut gather: Vec<u32> = Vec::new();
    let mut indices: Vec<u32> = Vec::with_capacity(source.len() * 2);
    let (mut added, mut already_reversed, mut coincident) = (0u64, 0u64, 0u64);
    for t in source.as_chunks::<3>().0 {
        indices.extend_from_slice(t);
        let [a, b, c] = t.map(bits);
        if a == b || b == c || c == a {
            coincident += 1;
            continue;
        }
        if keys.contains(&cycle_key([a, c, b])) {
            already_reversed += 1;
            continue;
        }
        let mut back = [0u32; 3];
        for (slot, &v) in back.iter_mut().zip(t) {
            let at = *copy_of.entry(v).or_insert_with(|| {
                gather.push(v);
                (n + gather.len() - 1) as u32
            });
            *slot = at;
        }
        indices.extend_from_slice(&[back[0], back[2], back[1]]);
        added += 1;
    }
    let mut streams = p.streams.clone();
    for (sem, s) in &mut streams {
        let negate = match sem {
            Semantic::Normal => Negate::Normal,
            Semantic::Tangent(_) => Negate::Tangent,
            _ => Negate::No,
        };
        append(s, &gather, negate);
    }
    Ok(Backed {
        streams,
        indices,
        gather,
        triangles_added: added,
        already_reversed,
        coincident,
    })
}

struct Planned {
    entity: Entity,
    primitive: usize,
    vertices_before: usize,
    backed: Backed,
}

impl Pass for AddBackFaces {
    fn name(&self) -> &'static str {
        "add-back-faces"
    }
    fn requires(&self) -> &[&'static str] {
        &["core.mesh", "pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "core.morphs", "pbr.material"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.mesh_file"]
    }
    /// The file links it removes are a side effect: the changed meshes no longer match their files.
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if let Some(e) = w.iter::<MeshFile>().map(|(e, _)| e).find(|&e| !w.has::<Mesh>(e)) {
            return Err(format!("{e:?}: geometry still in its file (load it first)"));
        }
        // everything is planned and checked before anything changes
        let mut plan: Vec<Planned> = Vec::new();
        let mut materials: BTreeSet<Entity> = BTreeSet::new();
        for (e, mesh) in w.iter::<Mesh>() {
            for (i, p) in mesh.primitives().iter().enumerate() {
                let what = describe(w, e, i);
                let drawn = drawn_with(w, e, i, p);
                if drawn.is_empty() {
                    log.count("primitives left as they are: no material", 1);
                    continue;
                }
                let mut sides = BTreeSet::new();
                for &m in &drawn {
                    let mat = w.get::<PbrMaterial>(m).ok_or_else(|| {
                        format!("{what}: material {m:?} has no pbr.material, so whether it is double-sided is not known")
                    })?;
                    sides.insert(mat.double_sided);
                }
                if sides.len() > 1 {
                    return Err(format!(
                        "{what}: drawn with double-sided and single-sided materials (its variants differ), so \
                         back faces cannot be drawn for some and not others"
                    ));
                }
                if !sides.contains(&true) {
                    log.count("primitives left as they are: single-sided material", 1);
                    continue;
                }
                if p.topology != Topology::Triangles {
                    log.count("primitives left as they are: not triangles", 1);
                    continue;
                }
                let backed = back_faces(p, &what)?;
                let from: Vec<u32> = (0..p.vertex_count() as u32)
                    .chain(backed.gather.iter().copied())
                    .collect();
                w.check_remap_vertices(e, i, &from)
                    .map_err(|why| format!("{what}: {why}"))?;
                materials.extend(drawn);
                plan.push(Planned {
                    entity: e,
                    primitive: i,
                    vertices_before: p.vertex_count(),
                    backed,
                });
            }
        }
        let mut by_entity: BTreeMap<Entity, Vec<Planned>> = BTreeMap::new();
        for pl in plan {
            by_entity.entry(pl.entity).or_default().push(pl);
        }
        for (e, planned) in by_entity {
            for pl in &planned {
                let from: Vec<u32> = (0..pl.vertices_before as u32)
                    .chain(pl.backed.gather.iter().copied())
                    .collect();
                w.remap_vertices(e, pl.primitive, &from)?;
                if let Some(morphs) = w.get_mut::<Morphs>(e) {
                    negate_morph_directions(morphs, pl.primitive, pl.vertices_before as u32);
                }
            }
            w.edit_mesh(e, |mesh| {
                for pl in &planned {
                    let mut target = mesh
                        .primitive_mut(pl.primitive)
                        .ok_or_else(|| format!("{e:?}: primitive {} vanished", pl.primitive))?;
                    let fields = target.fields();
                    *fields.streams = pl.backed.streams.clone();
                    *fields.indices = Some(pl.backed.indices.clone());
                }
                Ok(None)
            })?;
            w.remove::<MeshFile>(e);
            for pl in planned {
                log.count("primitives given back faces", 1);
                log.count("triangles added", pl.backed.triangles_added);
                log.count("vertices added", pl.backed.gather.len() as u64);
                log.count(
                    "triangles left without a back face: already reversed in the primitive",
                    pl.backed.already_reversed,
                );
                log.count(
                    "triangles left without a back face: coincident corners",
                    pl.backed.coincident,
                );
            }
        }
        for m in materials {
            let name = w.get::<Name>(m).map_or_else(|| format!("{m:?}"), |n| n.0.clone());
            w.get_mut::<PbrMaterial>(m).expect("checked").double_sided = false;
            log.count(&format!("material {name}: made single-sided"), 1);
        }
        Ok(())
    }
}

/// The deltas of the vertices from `first` on in `primitive` (the copies) have their normals and tangents negated: a
/// copy's frame is the negation of its source's, and so is the frame it morphs to.
fn negate_morph_directions(morphs: &mut Morphs, primitive: usize, first: u32) {
    for f in morphs.targets.iter_mut().flat_map(|t| &mut t.frames) {
        let Some(Some(d)) = f.primitives.get_mut(primitive) else {
            continue;
        };
        let entries = [&d.positions, &d.normals, &d.tangents]
            .into_iter()
            .flatten()
            .map(Vec::len)
            .next()
            .unwrap_or(0);
        let copies: Vec<usize> = match &d.indices {
            None => (first as usize..entries).collect(),
            Some(ix) => (0..ix.len()).filter(|&k| ix[k] >= first).collect(),
        };
        for v in [&mut d.normals, &mut d.tangents].into_iter().flatten() {
            for &k in &copies {
                v[k] = v[k].map(|c| -c);
            }
        }
    }
}
