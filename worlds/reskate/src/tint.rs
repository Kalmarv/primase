//! `lower-tint-reskate`: per-vertex tints drawn as skate. can draw them.
//!
//! A material multiplies its base colour by `COLOR_0` (glTF), and a reader's tint pass puts a placement's tint there
//! (GTA's `gta-tint`: each vertex's palette colour, column from the vertex, row from the placement). skate.'s shaders
//! have no per-vertex colour, only a material's `tint`. So each primitive with a colour other than white is split by
//! colour, as the reference exporter did it (gta-skate's MeshBuild: a triangle takes its first vertex's palette
//! column): a triangle goes with its first vertex's colour, at full precision, and each colour's triangles become a
//! primitive of their own drawn with a copy of the material tinted by that colour (the material's own tint multiplied
//! in, in linear light), one copy per (material, colour); the primitive's material variants ([`MaterialVariants`])
//! get tinted copies alike. A material whose source does not take `COLOR_0` (`core.vertex_color` `ignored`) is not
//! tinted, counted: a primitive no material of which takes it is left whole, and one drawn also with a material that
//! takes it is split for that one. Per-primitive components follow the split (`World::rebuild_primitives`). Only a
//! triangle list is split; a primitive of another topology keeps its material, counted.
//! A primitive spreading its triangles over more than [`MAX_COLOURS`] colours (vegetation and clutter gradients) keeps
//! one tint, their mean weighted by triangles: counted, with its triangles. `COLOR_0`'s alpha (a fade) is left for
//! `lower-vertex-alpha-reskate`.
//!
//! Option `copies=N` (a load-crash bisect: the amount of new materials apart from the split): at most N tinted
//! material copies are made, in the pass's order; a tinted primitive past them is split the same but keeps its
//! material, counted.

use std::collections::BTreeMap;

use crate::dialect::SkateMaterial;
use primase_core::pass::{Pass, PassLog};
use primase_core::{Entity, MaterialVariants, Mesh, Name, Primitive, Semantic, Stream, Topology, World};

/// Colours a primitive splits into at most; past it, one mean tint. 32 keeps palette gradients of 9 to 23 colours
/// (GTA's trees and palms) exact while bounding the material copies a primitive can make.
pub const MAX_COLOURS: usize = 32;

/// A primitive's `COLOR_0` colour per vertex, linear 0..1 as exact u16 steps (`None`: none, or white everywhere).
fn colours(p: &Primitive) -> Result<Option<Vec<[u16; 3]>>, String> {
    let c: Vec<[u16; 3]> = match p.streams.get(&Semantic::Color(0)) {
        None => return Ok(None),
        Some(Stream::U8x4(v)) => v
            .iter()
            .map(|c| [c[0], c[1], c[2]].map(|x| u16::from(x) * 257))
            .collect(),
        Some(Stream::U16x4(v)) => v.iter().map(|c| [c[0], c[1], c[2]]).collect(),
        Some(Stream::F32x4(v)) => v
            .iter()
            .map(|c| [c[0], c[1], c[2]].map(|x| (x.clamp(0.0, 1.0) * 65535.0).round() as u16))
            .collect(),
        Some(Stream::F32x3(v)) => v
            .iter()
            .map(|c| c.map(|x| (x.clamp(0.0, 1.0) * 65535.0).round() as u16))
            .collect(),
        Some(_) => return Err("COLOR_0 is not an RGB(A) stream".into()),
    };
    Ok(c.iter().any(|v| v != &[u16::MAX; 3]).then_some(c))
}

/// A colour as a material tint, sRGB bytes.
fn srgb(c: [f64; 3]) -> [u8; 3] {
    c.map(primase_image::linear_to_srgb8)
}

/// An sRGB byte in linear light (the IEC 61966-2-1 curve).
fn linear(b: u8) -> f64 {
    let c = f64::from(b) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A colour of a split, linear 0..1.
type Colour = [f64; 3];

/// One primitive's plan: per new primitive, its triangles (`None`: all of them, unsplit) and its colour.
type Split = Vec<(Option<Vec<u32>>, Colour)>;

/// A mesh rebuilt: its entity, the new primitives, each one's source primitive and colour.
type Rebuilt = (Entity, Vec<Primitive>, Vec<usize>, Vec<Option<Colour>>);

/// `lower-tint-reskate` (see the module documentation).
#[derive(Clone, Copy, Debug, Default)]
pub struct LowerTintReskate {
    /// At most this many tinted material copies (`copies=N`); `None`: one per (material, colour).
    pub copies: Option<usize>,
}

impl Pass for LowerTintReskate {
    fn name(&self) -> &'static str {
        "lower-tint-reskate"
    }
    // no `requires`: a part without tints has nothing to lower
    fn provides(&self) -> &[&'static str] {
        &["skate.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh"]
    }
    fn approximates(&self) -> &'static str {
        "a primitive tinted with more than 32 colours keeps their mean (counted with its triangles)"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        // ---- plan: per mesh, per primitive, how it splits
        let mut plans: Vec<(Entity, Vec<Option<Split>>)> = Vec::new();
        for (e, mesh) in world.iter::<Mesh>() {
            let mut any = false;
            let mut per: Vec<Option<Split>> = Vec::with_capacity(mesh.primitives().len());
            for (i, p) in mesh.primitives().iter().enumerate() {
                let drawn = primase_core::color0_drawn(world, e, mesh, i);
                let plan = plan(world, e, p, drawn, log).map_err(|m| format!("{e:?} primitive {i}: {m}"))?;
                any |= plan.is_some();
                per.push(plan);
            }
            if any {
                plans.push((e, per));
            }
        }
        if plans.is_empty() {
            return Ok(());
        }
        // ---- the new primitive lists, and which old primitive each comes from; tried on a copy so a refusal
        // changes nothing
        let mut rebuilt: Vec<Rebuilt> = Vec::new();
        for (e, per) in &plans {
            let old = world.get::<Mesh>(*e).expect("planned");
            let (mut prims, mut from, mut colours) = (Vec::new(), Vec::new(), Vec::new());
            for (i, (p, plan)) in old.primitives().iter().zip(per).enumerate() {
                match plan {
                    None => {
                        prims.push(p.clone());
                        from.push(i);
                        colours.push(None);
                    }
                    Some(split) => {
                        for (tris, colour) in split {
                            let mut q = p.clone();
                            if let Some(t) = tris {
                                q.indices = Some(t.clone());
                            }
                            prims.push(q);
                            from.push(i);
                            colours.push(Some(*colour));
                        }
                    }
                }
            }
            rebuilt.push((*e, prims, from, colours));
        }
        let mut probe = world.subset(&rebuilt.iter().map(|(e, ..)| *e).collect());
        for (e, _, from, _) in &rebuilt {
            probe
                .remap_primitives(*e, from)
                .map_err(|m| format!("{e:?}: {m}"))?;
        }
        drop(probe);
        // ---- apply: material copies per (material, colour), the meshes, their per-primitive data and variants
        let mut made: BTreeMap<(Entity, [u8; 3]), Entity> = BTreeMap::new();
        for (e, mut prims, from, colours) in rebuilt {
            for (p, colour) in prims.iter_mut().zip(&colours) {
                let (Some(colour), Some(m)) = (colour, p.material) else {
                    continue;
                };
                if !primase_core::uses_color0(world, m) {
                    log.count(
                        "tinted primitives whose own material ignores COLOR_0, material kept",
                        1,
                    );
                    continue;
                }
                match self.copy(world, &mut made, m, *colour, log) {
                    Some(c) => {
                        p.material = Some(c);
                        log.count("tinted primitives", 1);
                    }
                    None => log.count("tinted primitives past copies=N, material kept", 1),
                }
            }
            world
                .rebuild_primitives(e, |_| Ok((prims, from)))
                .expect("the same remap succeeded on the copy");
            // the variants followed the split; a tinted primitive's variant materials get tinted copies too
            if let Some(mut v) = world.get::<MaterialVariants>(e).cloned() {
                for (map, colour) in v.0.iter_mut().zip(&colours) {
                    let Some(colour) = colour else { continue };
                    for m in map.values_mut() {
                        if !primase_core::uses_color0(world, *m) {
                            log.count("variant materials ignoring COLOR_0, kept untinted", 1);
                            continue;
                        }
                        match self.copy(world, &mut made, *m, *colour, log) {
                            Some(c) => {
                                *m = c;
                                log.count("tinted variant materials of primitives", 1);
                            }
                            None => log.count("tint lost on a variant (past copies=N)", 1),
                        }
                    }
                }
                world.insert(e, v);
            }
        }
        Ok(())
    }
}

/// How primitive `p` of mesh `e` splits (`None`: it keeps its material). `drawn`: some material drawing it, variants
/// included, takes `COLOR_0` ([`primase_core::color0_drawn`]).
fn plan(
    world: &World,
    e: Entity,
    p: &Primitive,
    drawn: bool,
    log: &mut PassLog,
) -> Result<Option<Split>, String> {
    if !p.material.is_some_and(|m| world.has::<SkateMaterial>(m)) {
        return Ok(None);
    }
    let Some(c) = colours(p)? else {
        return Ok(None);
    };
    if !drawn {
        log.count(
            "tinted primitives whose materials ignore COLOR_0 (core.vertex_color), not tinted",
            1,
        );
        return Ok(None);
    }
    if p.topology != Topology::Triangles {
        log.count("tinted primitives not a triangle list, tint not applied", 1);
        return Ok(None);
    }
    let idx = p
        .indices
        .clone()
        .unwrap_or_else(|| (0..p.vertex_count() as u32).collect());
    let mut by: BTreeMap<[u16; 3], Vec<u32>> = BTreeMap::new();
    for t in idx.as_chunks::<3>().0 {
        let first = c
            .get(t[0] as usize)
            .ok_or_else(|| format!("index {} past the {} colours", t[0], c.len()))?;
        by.entry(*first).or_default().extend_from_slice(t);
    }
    let lin = |k: &[u16; 3]| k.map(|x| f64::from(x) / 65535.0);
    Ok(Some(if by.len() > MAX_COLOURS {
        let n: f64 = by.values().map(|t| (t.len() / 3) as f64).sum();
        let mean: Colour = std::array::from_fn(|ch| {
            by.iter()
                .map(|(k, t)| lin(k)[ch] * (t.len() / 3) as f64)
                .sum::<f64>()
                / n
        });
        log.approximate("primitives tinted with more than 32 colours, their mean", 1);
        log.approximate("triangles drawn with a mean tint", n as u64);
        // counted too: a conversion's report lists counts
        log.count("primitives with a mean tint (more than 32 colours)", 1);
        log.count("triangles with a mean tint", n as u64);
        // which: the mesh and material, so the approximation can be found and judged
        let mesh_name = world.get::<Name>(e).map_or("unnamed mesh", |n| n.0.as_str());
        let key = p
            .material
            .and_then(|m| world.get::<SkateMaterial>(m))
            .map_or("", |s| s.key.as_str());
        log.count(
            &format!(
                "triangles with a mean tint ({} colours): {mesh_name} / {key}",
                by.len()
            ),
            n as u64,
        );
        vec![(None, mean)]
    } else if by.len() == 1 {
        vec![(None, lin(by.keys().next().expect("one")))]
    } else {
        log.count("tinted primitives split by colour", 1);
        by.into_iter().map(|(k, t)| (Some(t), lin(&k))).collect()
    }))
}

impl LowerTintReskate {
    /// Material `m`'s copy tinted by `colour` (its own tint multiplied in, in linear light), made once per
    /// (material, colour as sRGB bytes); `None` past `copies`.
    fn copy(
        &self,
        world: &mut World,
        made: &mut BTreeMap<(Entity, [u8; 3]), Entity>,
        m: Entity,
        colour: Colour,
        log: &mut PassLog,
    ) -> Option<Entity> {
        let rgb = srgb(colour);
        if let Some(&c) = made.get(&(m, rgb)) {
            return Some(c);
        }
        if self.copies.is_some_and(|n| made.len() >= n) {
            return None;
        }
        let mut s = world.get::<SkateMaterial>(m)?.clone();
        let own = s.tint.unwrap_or([255; 3]);
        s.tint = Some(std::array::from_fn(|k| {
            primase_image::linear_to_srgb8(linear(own[k]) * colour[k])
        }));
        s.key = format!("{}~tint{:02x}{:02x}{:02x}", s.key, rgb[0], rgb[1], rgb[2]);
        let c = world.spawn();
        world.copy_components(m, c);
        world.insert(c, s);
        made.insert((m, rgb), c);
        log.count("tinted material copies", 1);
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::MaterialClass;

    fn material(w: &mut World) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            SkateMaterial {
                key: "wall".into(),
                class: MaterialClass::Opaque,
                base: None,
                normal: None,
                mask: None,
                base_color: None,
                tint: None,
                alpha: None,
                cutoff: None,
                two_sided: None,
                domain: None,
                metallic: None,
                roughness: None,
                wrap: None,
                ocean: None,
                cast_shadows: None,
                emissive: None,
                vertex_alpha: None,
                other: Default::default(),
            },
        );
        e
    }

    /// Two triangles over four vertices.
    fn quad(m: Entity, c: Option<Stream>) -> Primitive {
        let mut streams = BTreeMap::from([(
            Semantic::Position,
            Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]]),
        )]);
        if let Some(c) = c {
            streams.insert(Semantic::Color(0), c);
        }
        Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2, 3, 2, 1]),
            material: Some(m),
        }
    }

    /// Per-primitive dialect data, as a reader types it.
    #[derive(Clone, Debug, PartialEq)]
    struct Sources(Vec<&'static str>);
    impl primase_core::Component for Sources {
        const NAME: &'static str = "test.sources";
        fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
            Some(primase_core::remap_list(&self.0, from).map(Sources))
        }
    }

    /// A primitive whose two triangles start at a red and a white vertex splits in two (red tinted, white
    /// tinted white); a one-colour primitive is retinted whole; white and uncoloured ones keep the material; the
    /// dialect's per-primitive data follows. Worked by hand: linear 0.5 is sRGB byte 188.
    #[test]
    fn tinted_primitives_split_by_colour() {
        let mut w = World::new();
        let m = material(&mut w);
        let half = 32768;
        let two = Stream::U16x4(vec![
            [65535, half, half, 65535],
            [65535; 4],
            [65535; 4],
            [half, half, 65535, 65535],
        ]);
        let one = Stream::U16x4(vec![[65535, half, half, 65535]; 4]);
        let mesh = w.spawn();
        w.insert(
            mesh,
            Mesh::new(vec![
                quad(m, Some(two)),
                quad(m, Some(one)),
                quad(m, Some(Stream::U8x4(vec![[255; 4]; 4]))),
                quad(m, None),
            ]),
        );
        w.insert(mesh, Sources(vec!["two", "one", "white", "none"]));
        let mut log = PassLog::default();
        LowerTintReskate::default().run(&mut w, &mut log).unwrap();
        let prims = &w.get::<Mesh>(mesh).unwrap().primitives();
        assert_eq!(prims.len(), 5);
        // the split: triangle 0 starts at red vertex 0, triangle 1 at blue vertex 3 (sorted by colour: blue first)
        assert_eq!(prims[0].indices.as_deref(), Some(&[3, 2, 1][..]));
        assert_eq!(prims[1].indices.as_deref(), Some(&[0, 1, 2][..]));
        let tint = |p: &Primitive| w.get::<SkateMaterial>(p.material.unwrap()).unwrap().tint;
        assert_eq!(
            (tint(&prims[0]), tint(&prims[1]), tint(&prims[2])),
            (
                Some([188, 188, 255]),
                Some([255, 188, 188]),
                Some([255, 188, 188])
            )
        );
        assert_eq!(
            prims[1].material, prims[2].material,
            "one copy per (material, colour)"
        );
        assert_eq!((prims[3].material, prims[4].material), (Some(m), Some(m)));
        assert_eq!(
            w.get::<Sources>(mesh).unwrap().0,
            ["two", "two", "one", "white", "none"]
        );
        assert_eq!(log.counts["tinted primitives split by colour"], 1);
        assert!(log.approximations.is_empty());
    }

    /// `copies=1`: the split is the same, but only the first (material, colour) gets a copy; the other colour's
    /// primitive keeps the material, counted.
    #[test]
    fn copies_caps_the_tinted_materials_not_the_split() {
        let mut w = World::new();
        let m = material(&mut w);
        let half = 32768;
        let two = Stream::U16x4(vec![
            [65535, half, half, 65535],
            [65535; 4],
            [65535; 4],
            [half, half, 65535, 65535],
        ]);
        let mesh = w.spawn();
        w.insert(mesh, Mesh::new(vec![quad(m, Some(two))]));
        let mut log = PassLog::default();
        LowerTintReskate { copies: Some(1) }
            .run(&mut w, &mut log)
            .unwrap();
        let prims = &w.get::<Mesh>(mesh).unwrap().primitives();
        assert_eq!(prims.len(), 2);
        assert_ne!(prims[0].material, Some(m));
        assert_eq!(prims[1].material, Some(m));
        assert_eq!(log.counts["tinted material copies"], 1);
        assert_eq!(log.counts["tinted primitives past copies=N, material kept"], 1);
    }

    /// More than [`MAX_COLOURS`] colours: one mean tint, counted with its triangles.
    #[test]
    fn a_gradient_keeps_a_mean_tint() {
        let mut w = World::new();
        let m = material(&mut w);
        let n = MAX_COLOURS as u16 + 1;
        let positions: Vec<[f32; 3]> = (0..n * 3).map(|i| [f32::from(i), 0.0, 0.0]).collect();
        let colour: Vec<[u16; 4]> = (0..n * 3)
            .map(|i| [65535 - (i / 3) * 1000, 65535, 65535, 65535])
            .collect();
        let mesh = w.spawn();
        w.insert(
            mesh,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([
                    (Semantic::Position, Stream::F32x3(positions)),
                    (Semantic::Color(0), Stream::U16x4(colour)),
                ]),
                indices: None,
                material: Some(m),
            }]),
        );
        let mut log = PassLog::default();
        LowerTintReskate::default().run(&mut w, &mut log).unwrap();
        assert_eq!(w.get::<Mesh>(mesh).unwrap().primitives().len(), 1);
        assert_eq!(
            log.approximations["triangles drawn with a mean tint"],
            u64::from(n)
        );
    }
    /// A material's own tint multiplies in linear light, worked by hand: own sRGB 188 (linear 0.5029) x colour
    /// linear 0.5 = 0.2514, sRGB byte 137 (an sRGB-byte product would give 94). A variant material of the tinted
    /// primitive gets its own tinted copy, and the variants follow the split.
    #[test]
    fn tint_multiplies_in_linear_light_and_variants_get_copies() {
        let mut w = World::new();
        let m = material(&mut w);
        let mut s = w.get::<SkateMaterial>(m).unwrap().clone();
        s.tint = Some([188, 255, 255]);
        w.insert(m, s);
        let variant = material(&mut w);
        let half = 32768;
        let one = Stream::U16x4(vec![[half, 65535, 65535, 65535]; 4]);
        let mesh = w.spawn();
        w.insert(mesh, Mesh::new(vec![quad(m, Some(one))]));
        w.insert(
            mesh,
            primase_core::MaterialVariants(vec![BTreeMap::from([(1, variant)])]),
        );
        let mut log = PassLog::default();
        LowerTintReskate::default().run(&mut w, &mut log).unwrap();
        let p = &w.get::<Mesh>(mesh).unwrap().primitives()[0];
        let tint = w.get::<SkateMaterial>(p.material.unwrap()).unwrap().tint.unwrap();
        assert_eq!(tint, [137, 255, 255]);
        let v = &w.get::<primase_core::MaterialVariants>(mesh).unwrap().0[0];
        let copy = v[&1];
        assert_ne!(copy, variant);
        assert_eq!(w.get::<SkateMaterial>(copy).unwrap().tint, Some([188, 255, 255]));
        assert_eq!(log.counts["tinted variant materials of primitives"], 1);
    }

    /// A material whose source ignores COLOR_0 (`core.vertex_color`) is not tinted; a primitive drawn also with a
    /// variant that takes it is tinted for the variant only, and one no material of which takes it is left whole.
    #[test]
    fn materials_ignoring_vertex_colour_are_not_tinted() {
        let mut w = World::new();
        let ignores = material(&mut w);
        w.insert(ignores, primase_core::VertexColor::Ignored);
        let takes = material(&mut w);
        let red = Stream::U8x4(vec![[255, 0, 0, 255]; 4]);
        let alone = w.spawn();
        w.insert(alone, Mesh::new(vec![quad(ignores, Some(red.clone()))]));
        let mixed = w.spawn();
        w.insert(mixed, Mesh::new(vec![quad(ignores, Some(red))]));
        w.insert(
            mixed,
            primase_core::MaterialVariants(vec![BTreeMap::from([(1, takes)])]),
        );
        let mut log = PassLog::default();
        LowerTintReskate::default().run(&mut w, &mut log).unwrap();
        assert_eq!(
            w.get::<Mesh>(alone).unwrap().primitives()[0].material,
            Some(ignores)
        );
        assert_eq!(
            log.counts["tinted primitives whose materials ignore COLOR_0 (core.vertex_color), not tinted"],
            1
        );
        assert_eq!(
            w.get::<Mesh>(mixed).unwrap().primitives()[0].material,
            Some(ignores)
        );
        assert_eq!(
            log.counts["tinted primitives whose own material ignores COLOR_0, material kept"],
            1
        );
        let copy = w.get::<primase_core::MaterialVariants>(mixed).unwrap().0[0][&1];
        assert_ne!(copy, takes);
        assert_eq!(w.get::<SkateMaterial>(copy).unwrap().tint, Some([255, 0, 0]));
    }

    /// Only a triangle list splits (another topology keeps its material, counted); an index past the colour stream
    /// and a COLOR_0 that is not RGB(A) are refused, the world unchanged.
    #[test]
    fn other_topologies_are_counted_and_bad_input_refused() {
        let mut w = World::new();
        let m = material(&mut w);
        let red = Stream::U8x4(vec![[255, 0, 0, 255]; 4]);
        let mut strip = quad(m, Some(red.clone()));
        strip.topology = Topology::Lines;
        let mesh = w.spawn();
        w.insert(mesh, Mesh::new(vec![strip]));
        let mut log = PassLog::default();
        LowerTintReskate::default().run(&mut w, &mut log).unwrap();
        assert_eq!(
            log.counts["tinted primitives not a triangle list, tint not applied"],
            1
        );
        assert_eq!(w.get::<Mesh>(mesh).unwrap().primitives()[0].material, Some(m));
        for bad in [
            {
                let mut q = quad(m, Some(red.clone()));
                q.indices = Some(vec![9, 1, 0]);
                q
            },
            quad(m, Some(Stream::F32x2(vec![[1.0, 0.0]; 4]))),
        ] {
            let mut w = World::new();
            let m2 = material(&mut w);
            let mut q = bad;
            q.material = Some(m2);
            let mesh = w.spawn();
            w.insert(mesh, Mesh::new(vec![q.clone()]));
            let e = LowerTintReskate::default()
                .run(&mut w, &mut PassLog::default())
                .unwrap_err();
            assert!(e.contains("past the") || e.contains("not an RGB"), "{e}");
            assert_eq!(w.get::<Mesh>(mesh).unwrap().primitives(), &[q][..]);
        }
    }
}
