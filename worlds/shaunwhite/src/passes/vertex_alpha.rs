//! `lower-shaunwhite-vertex-alpha`: `COLOR_0` of a blended or alpha-tested Material as the alpha it is.
//!
//! The shipped generic shader (`ekgeneric.fx`, `FctDiffuseColor`) reads a vertex colour as two things: its **red** is
//! the weight that interpolates the diffuse texture towards the texture times the Material's tint,
//! `lerp(texture, texture * tint, COLOR_0.r)` (README, restored material layers), and its **alpha** is the vertex alpha
//! of the blend or test. Its green and blue are not read. Core can multiply `COLOR_0` into a material's base colour or
//! ignore it, not take its alpha alone, so [`crate::passes::LowerShaunwhiteMaterial`] leaves a blended or alpha-tested
//! Material with a vertex alpha below one drawing with `COLOR_0` as a multiplier, its red, green and blue then
//! multiplying the colour as a colour: a vertex red of 0 (the weight that leaves the texture untinted) draws black.
//!
//! This pass rewrites the stream to what the multiplier should be: red, green and blue 1 and the stored alpha, in the
//! stream's own type, on every primitive that only such Materials and Materials that ignore `COLOR_0`
//! ([`primase_core::VertexColor::Ignored`]) draw. The Material's base colour (the tint) multiplies the texture as
//! `texture * tint`, which is the equation at a red of 1. Where the stored red is not 1 and the tint is not white the
//! game shows less of the tint; the pass says which Materials differ:
//!
//! - **red 1 on every vertex** (or the Material's primitives have no `COLOR_0`, which the shader reads as white), or
//!   a white tint: exact.
//! - **red 0 on every vertex**: the tint is never applied by the shader; the Material's base colour red, green and
//!   blue become 1 (its alpha stays): exact.
//! - otherwise (the red varies, or is between): the tint is applied in full: an approximation, counted and named.
//!
//! A primitive that a Material reading `COLOR_0` as a colour also draws (its own or a variant's) is left as stored,
//! counted: its stream is that Material's, and the Material then keeps the multiplier. Materials marked
//! `core.material_usage` (water, vegetation) are not touched. The pass changes meshes, so their file links go, and it
//! is idempotent: a stream it wrote is red, green and blue 1.
//!
//! Which Materials: those [`crate::passes::LowerShaunwhiteMaterial`] left reading `COLOR_0` (blended or alpha-tested
//! `pbr.material`, no `core.vertex_color`, not layered).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    Component, Entity, LayeredMaterial, MaterialUsage, MaterialVariants, Mesh, MeshFile, Primitive, Semantic,
    Stream, VertexColor, World, uses_color0,
};

/// A vertex red this close to 1 (or 0) is 1 (or 0): the stream's own quantisation (8 bits: 1/255).
const FULL: f32 = 0.998;
const NONE: f32 = 0.002;
/// A tint channel this close to 1 is white.
const WHITE: f32 = 0.999;

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct LowerShaunwhiteVertexAlpha;

pub const LOWER_SHAUNWHITE_VERTEX_ALPHA: PassSpec = PassSpec {
    name: "lower-shaunwhite-vertex-alpha",
    options: &[],
    make: |_| Ok(Box::new(LowerShaunwhiteVertexAlpha)),
};

/// The Materials `lower-shaunwhite-material` left drawing with `COLOR_0` as a multiplier.
fn candidates(w: &World) -> BTreeSet<Entity> {
    w.iter::<PbrMaterial>()
        .filter(|(e, p)| {
            p.alpha_mode != AlphaMode::Opaque
                && !w.has::<VertexColor>(*e)
                && !w.has::<LayeredMaterial>(*e)
                && !w.has::<MaterialUsage>(*e)
        })
        .map(|(e, _)| e)
        .collect()
}

/// Every material some drawing of primitive `index` of `mesh` uses: its own and its variants'.
fn drawn_with(w: &World, mesh: Entity, index: usize, p: &Primitive) -> Vec<Entity> {
    p.material
        .into_iter()
        .chain(
            w.get::<MaterialVariants>(mesh)
                .and_then(|v| v.0.get(index))
                .into_iter()
                .flat_map(|v| v.values().copied()),
        )
        .collect()
}

/// The red of each vertex of a `COLOR_0` stream, 0 to 1.
fn reds(stream: &Stream) -> Vec<f32> {
    match stream {
        Stream::F32x4(c) => c.iter().map(|v| v[0]).collect(),
        Stream::U8x4(c) => c.iter().map(|v| f32::from(v[0]) / 255.0).collect(),
        Stream::U16x4(c) => c.iter().map(|v| f32::from(v[0]) / 65535.0).collect(),
        _ => Vec::new(),
    }
}

/// Whether a stream already has red, green and blue 1.
fn is_white(stream: &Stream) -> bool {
    match stream {
        Stream::F32x4(c) => c.iter().all(|v| v[..3].iter().all(|x| *x == 1.0)),
        Stream::U8x4(c) => c.iter().all(|v| v[..3].iter().all(|x| *x == u8::MAX)),
        Stream::U16x4(c) => c.iter().all(|v| v[..3].iter().all(|x| *x == u16::MAX)),
        _ => true,
    }
}

/// The stream with red, green and blue 1 and its alpha.
fn whiten(stream: &mut Stream) {
    match stream {
        Stream::F32x4(c) => c.iter_mut().for_each(|v| v[..3].fill(1.0)),
        Stream::U8x4(c) => c.iter_mut().for_each(|v| v[..3].fill(u8::MAX)),
        Stream::U16x4(c) => c.iter_mut().for_each(|v| v[..3].fill(u16::MAX)),
        _ => {}
    }
}

/// What the vertex reds of a Material's primitives are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Red {
    /// 1 on every vertex (or no stream).
    Full,
    /// 0 on every vertex.
    Zero,
    /// Anything else.
    Between,
}

#[derive(Default)]
struct Seen {
    full: bool,
    zero: bool,
    between: bool,
    /// Some primitive of it is left as stored.
    left: bool,
}

impl Seen {
    fn add(&mut self, reds: &[f32]) {
        for &r in reds {
            if r >= FULL {
                self.full = true;
            } else if r <= NONE {
                self.zero = true;
            } else {
                self.between = true;
            }
        }
    }
    fn red(&self) -> Red {
        match (self.full, self.zero, self.between) {
            (_, false, false) => Red::Full,
            (false, true, false) => Red::Zero,
            _ => Red::Between,
        }
    }
}

impl Pass for LowerShaunwhiteVertexAlpha {
    fn name(&self) -> &'static str {
        "lower-shaunwhite-vertex-alpha"
    }
    fn requires(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, PbrMaterial::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[MeshFile::NAME]
    }
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn approximates(&self) -> &'static str {
        "COLOR_0 of blended or alpha-tested materials becomes its alpha alone, as the shader reads it; the tint multiplies the texture whole, which is exact only where the shader's vertex red is 1 or the tint white (all-zero red drops the tint); a tint with a red between is applied in full"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mine = candidates(w);
        if mine.is_empty() {
            return Ok(());
        }
        // what each Material's drawings read, and which primitives to rewrite
        let mut seen: BTreeMap<Entity, Seen> = BTreeMap::new();
        let mut rewrite: Vec<(Entity, usize)> = Vec::new();
        for (e, mesh) in w.iter::<Mesh>() {
            for (i, p) in mesh.primitives().iter().enumerate() {
                let drawn = drawn_with(w, e, i, p);
                let ours: Vec<Entity> = drawn.iter().copied().filter(|m| mine.contains(m)).collect();
                if ours.is_empty() {
                    continue;
                }
                let stream = p.streams.get(&Semantic::Color(0));
                let shared = drawn.iter().any(|m| !mine.contains(m) && uses_color0(w, *m));
                if shared && stream.is_some() {
                    log.count(
                        "primitives whose COLOR_0 a material reading it as a colour also draws, left as stored",
                        1,
                    );
                    for m in &ours {
                        seen.entry(*m).or_default().left = true;
                    }
                    continue;
                }
                let r = stream.map(reds).unwrap_or_default();
                for m in &ours {
                    let s = seen.entry(*m).or_default();
                    if stream.is_none() {
                        s.full = true;
                    } else {
                        s.add(&r);
                    }
                }
                if stream.is_some_and(|s| !is_white(s)) {
                    rewrite.push((e, i));
                }
            }
        }
        for (e, i) in rewrite.iter().copied().collect::<BTreeSet<_>>() {
            if w.has::<MeshFile>(e) {
                w.remove::<MeshFile>(e);
            }
            w.edit_mesh(e, |mesh| {
                let mut p = mesh
                    .primitive_mut(i)
                    .ok_or_else(|| format!("{e:?} has no primitive {i}"))?;
                if let Some(s) = p.streams_mut().get_mut(&Semantic::Color(0)) {
                    whiten(s);
                }
                Ok(None)
            })?;
            log.count("COLOR_0 streams rewritten to vertex alpha alone", 1);
        }
        for (m, s) in seen {
            let Some(p) = w.get::<PbrMaterial>(m) else {
                continue;
            };
            let white = p.base_color[..3].iter().all(|c| *c >= WHITE);
            let red = s.red();
            if s.left {
                log.approximate_on(
                    m,
                    "COLOR_0 of a primitive another material reads as a colour left as stored",
                    1,
                );
            } else if white || red == Red::Full {
                log.count("materials whose vertex alpha is exact", 1);
            } else if red == Red::Zero {
                if let Some(p) = w.get_mut::<PbrMaterial>(m) {
                    p.base_color[..3].fill(1.0);
                }
                log.count(
                    "materials whose tint the shader's vertex red of 0 removes (tint set to white)",
                    1,
                );
            } else {
                log.approximate_on(
                    m,
                    "tinted materials with a vertex red between 0 and 1 drawn with the tint in full (the shader lerps texture to texture times tint by the red)",
                    1,
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "tests lay hand-made worlds"
)]
mod tests {
    use super::*;
    use primase_core::Topology;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use std::path::PathBuf;

    fn material(w: &mut World, mode: AlphaMode, tint: [f32; 4]) -> Entity {
        let m = w.spawn();
        w.insert(
            m,
            PbrMaterial {
                alpha_mode: mode,
                base_color: tint,
                ..PbrMaterial::default()
            },
        );
        m
    }

    fn mesh(w: &mut World, material: Entity, color: Option<Stream>) -> Entity {
        let mut streams = BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
        )]);
        if let Some(c) = color {
            streams.insert(Semantic::Color(0), c);
        }
        let e = w.spawn();
        w.insert(
            e,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams,
                indices: None,
                material: Some(material),
            }]),
        );
        e
    }

    fn u8s(reds: [u8; 3], alphas: [u8; 3]) -> Stream {
        Stream::U8x4((0..3).map(|i| [reds[i], 9, 200, alphas[i]]).collect())
    }

    fn color(w: &World, e: Entity) -> Stream {
        w.get::<Mesh>(e).unwrap().primitives()[0]
            .streams
            .get(&Semantic::Color(0))
            .unwrap()
            .clone()
    }

    fn run(w: &mut World) -> RunReport {
        let mut registry = PassRegistry::new();
        crate::register(&mut registry);
        let pipeline = registry.parse("lower-shaunwhite-vertex-alpha").unwrap();
        let mut report = RunReport::default();
        PassManager::new(registry)
            .with_pass_checks(true)
            .run(w, &pipeline, &mut report)
            .unwrap();
        report
    }

    /// The stream of each type becomes white with its alpha, a second run changes nothing, and what a white tint or a
    /// red of 1 draws is exact (no approximation named).
    #[test]
    fn red_green_and_blue_become_one_and_the_alpha_stays() {
        let mut w = World::new();
        let white = material(&mut w, AlphaMode::Blend, [1.0; 4]);
        let tinted = material(&mut w, AlphaMode::Mask { cutoff: 0.5 }, [0.5, 0.4, 0.3, 1.0]);
        let u16s = material(&mut w, AlphaMode::Blend, [1.0; 4]);
        let a = mesh(&mut w, white, Some(u8s([10, 20, 30], [0, 128, 255])));
        let b = mesh(&mut w, tinted, Some(u8s([255, 255, 255], [7, 8, 9])));
        let c = mesh(
            &mut w,
            u16s,
            Some(Stream::U16x4(vec![[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]])),
        );
        let f = mesh(&mut w, white, Some(Stream::F32x4(vec![[0.3, 0.2, 0.1, 0.25]; 3])));
        let report = run(&mut w);
        assert_eq!(
            color(&w, a),
            Stream::U8x4(vec![
                [255, 255, 255, 0],
                [255, 255, 255, 128],
                [255, 255, 255, 255]
            ])
        );
        assert_eq!(
            color(&w, b),
            Stream::U8x4(vec![[255, 255, 255, 7], [255, 255, 255, 8], [255, 255, 255, 9]])
        );
        assert_eq!(
            color(&w, c),
            Stream::U16x4(vec![
                [65535, 65535, 65535, 4],
                [65535, 65535, 65535, 8],
                [65535, 65535, 65535, 12]
            ])
        );
        assert_eq!(color(&w, f), Stream::F32x4(vec![[1.0, 1.0, 1.0, 0.25]; 3]));
        let log = &report.steps[0].log;
        assert_eq!(log.counts["COLOR_0 streams rewritten to vertex alpha alone"], 4);
        assert!(log.approximations.is_empty(), "{:?}", log.approximations);
        assert_eq!(
            w.get::<PbrMaterial>(tinted).unwrap().base_color,
            [0.5, 0.4, 0.3, 1.0]
        );
        let before = w.clone().get::<Mesh>(a).cloned();
        let again = run(&mut w);
        assert_eq!(w.get::<Mesh>(a).cloned(), before);
        assert!(
            !again.steps[0]
                .log
                .counts
                .contains_key("COLOR_0 streams rewritten to vertex alpha alone")
        );
    }

    /// A tint the shader's vertex red of 0 never applies is dropped (its alpha stays), a tint with a red between is
    /// applied in full and named, and a material without `COLOR_0` (read as white) is exact.
    #[test]
    fn the_tint_follows_the_shaders_vertex_red() {
        let mut w = World::new();
        let tint = [0.5, 0.4, 0.3, 0.8];
        let zero = material(&mut w, AlphaMode::Blend, tint);
        let between = material(&mut w, AlphaMode::Blend, tint);
        let none = material(&mut w, AlphaMode::Blend, tint);
        let mixed = material(&mut w, AlphaMode::Blend, tint);
        mesh(&mut w, zero, Some(u8s([0, 0, 0], [10, 20, 30])));
        mesh(&mut w, between, Some(u8s([0, 128, 255], [10, 20, 30])));
        mesh(&mut w, none, None);
        mesh(&mut w, mixed, Some(u8s([255, 255, 255], [10, 20, 30])));
        mesh(&mut w, mixed, Some(u8s([0, 0, 0], [10, 20, 30])));
        let report = run(&mut w);
        assert_eq!(
            w.get::<PbrMaterial>(zero).unwrap().base_color,
            [1.0, 1.0, 1.0, 0.8]
        );
        for m in [between, none, mixed] {
            assert_eq!(w.get::<PbrMaterial>(m).unwrap().base_color, tint);
        }
        let log = &report.steps[0].log;
        let named = &log.approximated_on["tinted materials with a vertex red between 0 and 1 drawn with the tint in full (the shader lerps texture to texture times tint by the red)"];
        assert_eq!(named, &BTreeSet::from([between, mixed]));
        assert_eq!(log.counts["materials whose vertex alpha is exact"], 1);
    }

    /// A primitive a material reading `COLOR_0` as a colour also draws (by a variant) is left as stored and named;
    /// one a material that ignores it also draws is rewritten; water and vegetation are not touched.
    #[test]
    fn what_another_material_reads_is_not_rewritten() {
        let mut w = World::new();
        let ours = material(&mut w, AlphaMode::Blend, [1.0; 4]);
        let reader = material(&mut w, AlphaMode::Opaque, [1.0; 4]);
        let ignorer = material(&mut w, AlphaMode::Opaque, [1.0; 4]);
        w.insert(ignorer, VertexColor::Ignored);
        let stored = u8s([10, 20, 30], [1, 2, 3]);
        let shared = mesh(&mut w, ours, Some(stored.clone()));
        w.insert(shared, MaterialVariants(vec![BTreeMap::from([(0, reader)])]));
        let tolerant = mesh(&mut w, ours, Some(stored.clone()));
        w.insert(tolerant, MaterialVariants(vec![BTreeMap::from([(0, ignorer)])]));
        let water = material(&mut w, AlphaMode::Blend, [1.0; 4]);
        w.insert(water, MaterialUsage::Water);
        let leaf = material(&mut w, AlphaMode::Mask { cutoff: 0.5 }, [1.0; 4]);
        w.insert(leaf, MaterialUsage::Foliage);
        let wet = mesh(&mut w, water, Some(stored.clone()));
        let green = mesh(&mut w, leaf, Some(stored.clone()));
        let report = run(&mut w);
        assert_eq!(color(&w, shared), stored);
        assert_eq!(color(&w, wet), stored);
        assert_eq!(color(&w, green), stored);
        assert_eq!(
            color(&w, tolerant),
            Stream::U8x4(vec![[255, 255, 255, 1], [255, 255, 255, 2], [255, 255, 255, 3]])
        );
        let log = &report.steps[0].log;
        assert_eq!(
            log.counts["primitives whose COLOR_0 a material reading it as a colour also draws, left as stored"],
            1
        );
    }

    /// An opaque material, a layered one and one that already ignores `COLOR_0` are not this pass's: nothing changes.
    #[test]
    fn only_the_materials_left_reading_color0_are_taken() {
        let mut w = World::new();
        let opaque = material(&mut w, AlphaMode::Opaque, [1.0; 4]);
        let ignorer = material(&mut w, AlphaMode::Blend, [1.0; 4]);
        w.insert(ignorer, VertexColor::Ignored);
        let stored = u8s([10, 20, 30], [1, 2, 3]);
        let a = mesh(&mut w, opaque, Some(stored.clone()));
        let b = mesh(&mut w, ignorer, Some(stored.clone()));
        let report = run(&mut w);
        assert_eq!((color(&w, a), color(&w, b)), (stored.clone(), stored));
        assert!(report.steps[0].log.counts.is_empty());
    }

    /// The file link of a changed mesh goes; one not changed keeps its link.
    #[test]
    fn a_changed_mesh_loses_its_file_link() {
        let mut w = World::new();
        let m = material(&mut w, AlphaMode::Blend, [1.0; 4]);
        let changed = mesh(&mut w, m, Some(u8s([1, 2, 3], [4, 5, 6])));
        let kept = mesh(&mut w, m, Some(Stream::U8x4(vec![[255, 255, 255, 4]; 3])));
        for e in [changed, kept] {
            w.insert(
                e,
                MeshFile {
                    path: PathBuf::from("a.glb"),
                    mesh: 0,
                    materials: vec![Some(m)],
                },
            );
        }
        run(&mut w);
        assert!(!w.has::<MeshFile>(changed), "changed keeps its link");
        assert!(w.has::<MeshFile>(kept), "unchanged lost its link");
    }
}
