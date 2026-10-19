//! GTA's water selected and lowered: `gta-select-water` clips the placed water ([`crate::read::water`]) to the
//! region, and `lower-gta-water` makes the surfaces drawn meshes.
//!
//! `gta-select-water{x0,y0,x1,y1,clip}` (a selection pass): each placed water surface clipped to the region
//! (`clip=region`, the default), none kept (`clip=none`: a level whose water comes from elsewhere, counted), or to the
//! region and as far past it as the placements `gta-select-region` kept reach
//! (`clip=kept`: its `gta.region_extent`, which this pass then removes; the region rules keep a placement whose box
//! meets the region, so the seabed reaches past it); a surface with nothing left is left out with its placement.
//! Every placed surface is counted once, kept or outside; surfaces whose placement another state left out go too,
//! counted. It removes `gta.water`, counting the calming and wave quads of every file read (they are not placed).
//!
//! `lower-gta-water` (after loading): each `gta.water_surface` a flat upward mesh in the core frame with one water
//! material: the earlier exporter's blended colour ([`crate::passes::lower::WATER`], as water shaders get), roughness from the
//! tuning's specular falloff taken as the Blinn-Phong exponent (the water shader is not read: approximated), marked
//! `core.material_usage` water (what the water file states), and
//! its `core.collision`: the same outline as triangles at the water's height, every one on surface `WATER` (class
//! water, from the surface table), so a writer that knows water surfaces makes it water to fall into, not ground.
//! An invisible quad gets neither (counted); the corner values and the depth and stencil flags are not drawn:
//! counted. The surfaces are counted by height: the sea (z = 0) and above it.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    Collision, Entity, Instance, MaterialUsage, Mesh, Name, Primitive, Resource, Semantic, Stream, Surfaces,
    Topology, World,
};

use crate::components::GTA;
use crate::components::water::{GtaWater, GtaWaterSurface, area};
use crate::passes::rect::GroundRect;
use crate::passes::select::RegionExtent;

/// A convex polygon clipped to the rectangle `[x0, y0, x1, y1]` (Sutherland-Hodgman); empty when nothing is inside.
pub fn clip(p: &[[f32; 2]], r: [f32; 4]) -> Vec<[f32; 2]> {
    let mut out = p.to_vec();
    // each edge of the rectangle: axis, bound, keep the side above (true) or below
    for (axis, bound, above) in [
        (0, r[0], true),
        (0, r[2], false),
        (1, r[1], true),
        (1, r[3], false),
    ] {
        let inside = |v: [f32; 2]| if above { v[axis] >= bound } else { v[axis] <= bound };
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let (a, b) = (input[i], input[(i + 1) % input.len()]);
            if inside(a) {
                out.push(a);
            }
            if inside(a) != inside(b) {
                let t = (bound - a[axis]) / (b[axis] - a[axis]);
                out.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
            }
        }
        if out.is_empty() {
            break;
        }
    }
    if out.len() < 3 || area(&out).abs() < 1e-6 {
        Vec::new()
    } else {
        out
    }
}

/// What `gta-select-water` clips the placed water to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Clip {
    /// `clip=region` (the default): the region, or every surface whole without one.
    #[default]
    Region,
    /// `clip=kept`: the region and as far as `gta.region_extent` reaches.
    Kept,
    /// `clip=none`: no water at all, every placed surface left out (a level whose water comes from elsewhere, such
    /// as a far view drawn over the city's own sea).
    None,
}

/// `gta-select-water{x0,y0,x1,y1,clip}` (see the module documentation), made by [`SelectGtaWater::new`], which
/// checks that `clip=kept` has its region.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectGtaWater {
    region: Option<GroundRect>,
    clip: Clip,
}

impl SelectGtaWater {
    /// The surfaces clipped by `clip` to `region` (`None`: every surface whole): an error for [`Clip::Kept`] without a
    /// region.
    pub fn new(region: Option<GroundRect>, clip: Clip) -> std::result::Result<SelectGtaWater, String> {
        if clip == Clip::Kept && region.is_none() {
            return Err("clip=kept needs the region (x0, y0, x1, y1)".into());
        }
        Ok(SelectGtaWater { region, clip })
    }

    pub fn region(&self) -> Option<GroundRect> {
        self.region
    }

    pub fn clip(&self) -> Clip {
        self.clip
    }

    pub fn from_options(o: &Options) -> std::result::Result<SelectGtaWater, String> {
        for k in o.keys() {
            if !["x0", "y0", "x1", "y1", "clip"].contains(&k.as_str()) {
                return Err(format!("unknown option {k}"));
            }
        }
        let clip = match o.get("clip").map(String::as_str) {
            None | Some("region") => Clip::Region,
            Some("kept") => Clip::Kept,
            Some("none") => Clip::None,
            Some(v) => return Err(format!("clip: {v} is none of region, kept and none")),
        };
        SelectGtaWater::new(GroundRect::from_options(o)?, clip)
    }
}

impl Pass for SelectGtaWater {
    fn name(&self) -> &'static str {
        "gta-select-water"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        if let Some(r) = self.region {
            r.insert_options(&mut o);
        }
        match self.clip {
            Clip::Region => {}
            Clip::Kept => {
                o.insert("clip".into(), "kept".into());
            }
            Clip::None => {
                o.insert("clip".into(), "none".into());
            }
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        if self.clip == Clip::Kept {
            &["gta.water", RegionExtent::NAME]
        } else {
            &["gta.water"]
        }
    }
    fn modifies(&self) -> &[&'static str] {
        &["gta.water_surface"]
    }
    fn removes(&self) -> &[&'static str] {
        if self.clip == Clip::Kept {
            &["gta.water", RegionExtent::NAME]
        } else {
            &["gta.water"]
        }
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let waters: Vec<(Entity, GtaWater)> = w.iter::<GtaWater>().map(|(e, g)| (e, g.clone())).collect();
        for (e, water) in waters {
            w.remove::<GtaWater>(e);
            log.count("calming quads not drawn", water.calming.len() as u64);
            log.count("wave quads not drawn", water.waves.len() as u64);
        }
        // the region, and with clip=kept as far as what is drawn
        let clip_to = match (self.region.map(GroundRect::get), self.clip) {
            (Some(r), Clip::Kept) => {
                let extent = w
                    .remove_resource::<RegionExtent>()
                    .ok_or("clip=kept: no gta.region_extent")?;
                Some(extent.0.map_or(r, |e| {
                    [r[0].min(e[0]), r[1].min(e[1]), r[2].max(e[2]), r[3].max(e[3])]
                }))
            }
            (r, _) => r,
        };
        let placements: Vec<(Entity, Entity)> = w
            .iter::<Instance>()
            .filter(|(_, i)| w.has::<GtaWaterSurface>(i.prototype))
            .map(|(p, i)| (p, i.prototype))
            .collect();
        let placed: BTreeSet<Entity> = placements.iter().map(|&(_, proto)| proto).collect();
        // a surface whose placement another state left out (resolve-state) goes with it
        let unplaced: Vec<Entity> = w
            .iter::<GtaWaterSurface>()
            .map(|(e, _)| e)
            .filter(|e| !placed.contains(e))
            .collect();
        log.count(
            "water surfaces left out: placement left out by its state",
            unplaced.len() as u64,
        );
        for e in unplaced {
            // a prototype: its placement went with its state
            w.despawn(e);
        }
        if self.clip == Clip::None {
            for (p, proto) in placements {
                primase_core::ledger::despawn(w, p, log, "water surfaces left out: clip=none");
                w.despawn(proto);
            }
            return Ok(());
        }
        let Some(r) = clip_to else {
            log.count("water surfaces kept", placements.len() as u64);
            return Ok(());
        };
        for (p, proto) in placements {
            let Some(s) = w.get_mut::<GtaWaterSurface>(proto) else {
                continue;
            };
            s.outline = clip(&s.outline, r);
            if s.outline.is_empty() {
                primase_core::ledger::despawn(w, p, log, "water surfaces left out: outside the region");
                w.despawn(proto);
            } else {
                log.count("water surfaces kept", 1);
            }
        }
        Ok(())
    }
}

/// `lower-gta-water` (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerGtaWater;

/// The water surface material (see the module documentation).
pub fn water_material(specular_falloff: Option<f32>) -> PbrMaterial {
    let srgb = |b: u8| primase_image::srgb_to_linear(f64::from(b) / 255.0) as f32;
    let w = crate::passes::lower::WATER;
    PbrMaterial {
        base_color: [srgb(w[0]), srgb(w[1]), srgb(w[2]), f32::from(w[3]) / 255.0],
        metallic: 0.0,
        // a Blinn-Phong exponent n as GGX: alpha = sqrt(2 / (n + 2)), roughness sqrt(alpha)
        roughness: specular_falloff.map_or(1.0, |n| (2.0 / (n.max(0.0) + 2.0)).powf(0.25)),
        alpha_mode: AlphaMode::Blend,
        ..PbrMaterial::default()
    }
}

impl Pass for LowerGtaWater {
    fn name(&self) -> &'static str {
        "lower-gta-water"
    }
    // no `requires`: it runs on every loaded part and leaves one without its input (a part holding only a water
    // surface, a cell whose shaders all draw nothing) as it is
    fn provides(&self) -> &[&'static str] {
        &[
            "core.mesh",
            "pbr.material",
            "core.material_usage",
            "core.name",
            "core.collision",
            "core.surfaces",
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &["gta.water_surface"]
    }
    /// The one surface it adds (`WATER`) is classified from the surface table as it is added, so it can run after
    /// `lower-gta-surfaces`.
    fn settles(&self) -> &[&'static str] {
        &["core.surfaces"]
    }
    fn approximates(&self) -> &'static str {
        "the water shader as the earlier exporter's blended flat colour, its roughness from the tuning's specular \
         falloff; waves, calming, corner values and depth and stencil flags not drawn"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let surfaces: Vec<(Entity, GtaWaterSurface)> =
            w.iter::<GtaWaterSurface>().map(|(e, s)| (e, s.clone())).collect();
        if surfaces.is_empty() {
            return Ok(());
        }
        // one material per specular falloff (one per water file)
        let mut materials: BTreeMap<Option<u32>, Entity> = BTreeMap::new();
        // the water's collision surface, classified by the surface table
        let mut table = w.remove_resource::<Surfaces>().unwrap_or_default();
        let water = table.index("WATER").map_err(|e| e.to_string())?;
        if let Some(t) = table.0.get_mut(usize::from(water)) {
            t.class = crate::passes::surfaces::surface_class("WATER");
        }
        for (proto, s) in surfaces {
            w.remove::<GtaWaterSurface>(proto);
            if s.quad.invisible {
                log.count("invisible water quads: no mesh, no collision", 1);
                continue;
            }
            let material = *materials
                .entry(s.specular_falloff.map(f32::to_bits))
                .or_insert_with(|| {
                    let m = w.spawn();
                    w.insert(m, water_material(s.specular_falloff));
                    w.insert(m, MaterialUsage::Water);
                    w.insert(m, Name("gta_water".into()));
                    log.approximate("water drawn as a blended flat colour", 1);
                    m
                });
            // as stored, with no glTF meaning: counted
            log.approximate("water surfaces drawn without their corner values (a1-a4)", 1);
            if s.quad.limited_depth {
                log.approximate("water surfaces drawn without their limited depth", 1);
            }
            if s.quad.no_stencil {
                log.approximate("water surfaces drawn without their no-stencil flag", 1);
            }
            let positions: Vec<[f32; 3]> = s
                .outline
                .iter()
                .map(|p| GTA.point([p[0], p[1], s.quad.z]))
                .collect();
            let up = GTA.direction([0.0, 0.0, 1.0]);
            let indices: Vec<u32> = (1..s.outline.len() as u32 - 1)
                .flat_map(|k| [0, k, k + 1])
                .collect();
            let triangles: Vec<[u32; 3]> = indices.as_chunks::<3>().0.to_vec();
            log.count("water collision triangles", triangles.len() as u64);
            w.insert(
                proto,
                Collision {
                    positions: positions.clone(),
                    surfaces: vec![water; triangles.len()],
                    triangles,
                },
            );
            let mut streams = BTreeMap::new();
            streams.insert(Semantic::Normal, Stream::F32x3(vec![up; positions.len()]));
            streams.insert(Semantic::Position, Stream::F32x3(positions));
            w.insert(
                proto,
                Mesh::new(vec![Primitive {
                    topology: Topology::Triangles,
                    streams,
                    indices: Some(indices),
                    material: Some(material),
                }]),
            );
            log.count(
                if s.quad.z == 0.0 {
                    "water surfaces: sea"
                } else {
                    "water surfaces: above sea level (lakes, rivers, pools)"
                },
                1,
            );
        }
        w.insert_resource(table);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::water::{WaterQuad, outline};
    use crate::read::water::place_water;
    use crate::read::water::tests::{file, quad_xml};
    use primase_core::Component;
    use primase_core::state::{Condition, ResolveState, StateAxis};

    fn quad(kind: u8) -> WaterQuad {
        WaterQuad {
            min: [0.0, 0.0],
            max: [10.0, 20.0],
            z: 5.0,
            kind,
            invisible: false,
            limited_depth: false,
            corners: [26.0; 4],
            no_stencil: false,
        }
    }

    #[test]
    fn clipping_keeps_the_part_inside_the_region() {
        let c = clip(&outline(&quad(0)), [5.0, -5.0, 15.0, 10.0]);
        assert!((area(&c) / 2.0 - 50.0).abs() < 1e-4, "5 x 10 left: {c:?}");
        assert!(clip(&outline(&quad(0)), [20.0, 0.0, 30.0, 10.0]).is_empty());
        // a corner triangle half in the region: the part of triangle 0 1 2 with x <= 5 is a trapezoid of 150
        let t = clip(&outline(&quad(1)), [0.0, 0.0, 5.0, 20.0]);
        assert!((area(&t) / 2.0 - 75.0).abs() < 1e-3, "{t:?}");
    }

    fn select(region: [f32; 4], clip: Option<&str>) -> SelectGtaWater {
        let mut o = Options::new();
        for (k, v) in ["x0", "y0", "x1", "y1"].into_iter().zip(region) {
            o.insert(k.into(), v.to_string());
        }
        if let Some(c) = clip {
            o.insert("clip".into(), c.into());
        }
        SelectGtaWater::from_options(&o).unwrap()
    }

    fn surfaces(w: &World) -> Vec<GtaWaterSurface> {
        w.iter::<Instance>()
            .filter_map(|(_, i)| w.get::<GtaWaterSurface>(i.prototype).cloned())
            .collect()
    }

    /// The options decide the clip: the region alone by default; with clip=kept, also as far as the kept placements
    /// reach (here GTA x 0-20, y 0-10, past the 5 x 5 region), and the extent is used up.
    #[test]
    fn the_clip_is_the_region_or_the_region_and_what_is_kept() {
        for (clip, area_left) in [(None, 25.0), (Some("kept"), 200.0)] {
            let mut w = World::new();
            w.insert_resource(RegionExtent(Some([0.0, 0.0, 20.0, 10.0])));
            place_water(
                &mut w,
                vec![(
                    "world",
                    file("w.xml", &[quad_xml([-50, 50, -50, 50], false, false, false)]),
                )],
            );
            let pass = select([0.0, 0.0, 5.0, 5.0], clip);
            assert_eq!(pass.requires().contains(&RegionExtent::NAME), clip.is_some());
            pass.run(&mut w, &mut PassLog::default()).unwrap();
            let s = surfaces(&w);
            assert_eq!(s.len(), 1);
            assert!(
                (area(&s[0].outline) / 2.0 - area_left).abs() < 1e-3,
                "{clip:?}: {:?}",
                s[0].outline
            );
            assert_eq!(w.resource::<RegionExtent>().is_none(), clip.is_some());
        }
        // clip=none: no water placed, counted
        let mut w = World::new();
        place_water(
            &mut w,
            vec![(
                "world",
                file("w.xml", &[quad_xml([-50, 50, -50, 50], false, false, false)]),
            )],
        );
        let mut log = PassLog::default();
        select([0.0, 0.0, 5.0, 5.0], Some("none"))
            .run(&mut w, &mut log)
            .unwrap();
        assert!(surfaces(&w).is_empty() && w.iter::<GtaWaterSurface>().next().is_none());
        assert_eq!(
            log.drops.get(&(
                primase_core::ledger::Measure::Placements,
                "water surfaces left out: clip=none".into()
            )),
            Some(&1)
        );
        let mut o = Options::new();
        o.insert("clip".into(), "kept".into());
        assert!(
            SelectGtaWater::from_options(&o).is_err(),
            "clip=kept without a region"
        );
        o.insert("clip".into(), "all".into());
        assert!(SelectGtaWater::from_options(&o).is_err());
    }

    /// Two files: each placement carries its file's value on `gta.water`; resolve-state keeps the world map's by
    /// default, and the selection counts every surface once (kept, outside, or left out by its state). Lowering draws
    /// no invisible quad and counts what it does not draw.
    #[test]
    fn the_water_file_is_a_state_and_every_surface_is_counted_once() {
        let mut w = World::new();
        let counts = place_water(
            &mut w,
            vec![
                (
                    "world",
                    file(
                        "w.xml",
                        &[
                            quad_xml([0, 10, 0, 10], false, true, true),
                            quad_xml([0, 10, 0, 10], true, false, false),
                            quad_xml([100, 110, 0, 10], false, false, false),
                        ],
                    ),
                ),
                (
                    "heistisland",
                    file("h.xml", &[quad_xml([0, 10, 0, 10], false, false, false)]),
                ),
            ],
        );
        assert_eq!(counts.len(), 2);
        let axis = w.iter::<StateAxis>().next().unwrap().1.clone();
        assert_eq!(
            (axis.key.as_str(), axis.domain.len(), axis.default),
            ("gta.water", 2, Some(0))
        );
        assert_eq!(w.iter::<Condition>().count(), 4);
        let mut log = PassLog::default();
        ResolveState::default().run(&mut w, &mut log).unwrap();
        assert_eq!(log.counts.get("gta.water=world kept"), Some(&3));
        assert_eq!(log.counts.get("gta.water=world despawned"), Some(&1));
        let mut log = PassLog::default();
        select([-5.0, -5.0, 5.0, 5.0], None)
            .run(&mut w, &mut log)
            .unwrap();
        assert_eq!(log.counts.get("water surfaces kept"), Some(&2));
        assert_eq!(
            log.drops.get(&(
                primase_core::ledger::Measure::Placements,
                "water surfaces left out: outside the region".into()
            )),
            Some(&1)
        );
        assert_eq!(
            log.counts
                .get("water surfaces left out: placement left out by its state"),
            Some(&1)
        );
        assert_eq!(w.iter::<GtaWaterSurface>().count(), 2);
        let s = surfaces(&w);
        assert!(s.iter().any(|s| s.quad.limited_depth && s.quad.no_stencil));
        let json = s[0].to_json().unwrap().unwrap();
        assert_eq!(json["quad"]["corners"][0], 26.0);
        let mut log = PassLog::default();
        LowerGtaWater.run(&mut w, &mut log).unwrap();
        assert_eq!(
            log.counts.get("invisible water quads: no mesh, no collision"),
            Some(&1)
        );
        assert_eq!(log.counts.get("water surfaces: sea"), Some(&1));
        for what in [
            "water surfaces drawn without their corner values (a1-a4)",
            "water surfaces drawn without their limited depth",
            "water surfaces drawn without their no-stencil flag",
        ] {
            assert_eq!(log.approximations.get(what), Some(&1), "{what}");
        }
    }

    /// The selected surface lowered: flat at its height, the water material.
    #[test]
    fn a_selected_surface_is_lowered_flat_with_the_water_material() {
        let mut w = World::new();
        place_water(
            &mut w,
            vec![(
                "world",
                file("w.xml", &[quad_xml([0, 10, 0, 10], false, false, false)]),
            )],
        );
        let mut log = PassLog::default();
        select([-5.0, -5.0, 5.0, 5.0], None)
            .run(&mut w, &mut log)
            .unwrap();
        assert!(w.iter::<GtaWater>().next().is_none());
        let protos: Vec<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        assert_eq!(protos.len(), 1);
        assert!(w.get::<Mesh>(protos[0]).is_none());
        LowerGtaWater.run(&mut w, &mut log).unwrap();
        assert!(w.get::<GtaWaterSurface>(protos[0]).is_none());
        let mesh = w.get::<Mesh>(protos[0]).unwrap();
        let p = mesh.primitives()[0].positions().unwrap();
        // the 5 x 5 corner of the quad, flat at the sea's height (core y up)
        assert!(p.iter().all(|v| v[1] == 0.0));
        let m = w
            .get::<PbrMaterial>(mesh.primitives()[0].material.unwrap())
            .unwrap();
        assert_eq!(m.alpha_mode, AlphaMode::Blend);
        assert!((m.roughness - (2.0f32 / 1120.0).powf(0.25)).abs() < 1e-6);
        assert_eq!(
            w.get::<MaterialUsage>(mesh.primitives()[0].material.unwrap()),
            Some(&MaterialUsage::Water)
        );
        // its collision: the same two triangles at the sea's height, facing up, on surface WATER of class water
        let c = w.get::<Collision>(protos[0]).unwrap();
        assert_eq!(c.triangles.len(), 2);
        assert!(c.positions.iter().all(|v| v[1] == 0.0));
        let [a, b, d] = c.triangles[0].map(|i| c.positions[i as usize]);
        let (u, v) = ([b[0] - a[0], b[2] - a[2]], [d[0] - a[0], d[2] - a[2]]);
        // core y up: the normal's y is the cross product's (z, x) part; positive faces up
        assert!(u[1] * v[0] - u[0] * v[1] > 0.0, "{:?}", c.triangles[0]);
        let table = w.resource::<Surfaces>().unwrap();
        let s = &table.0[usize::from(c.surfaces[0])];
        assert_eq!(
            (s.name.as_str(), s.class),
            ("WATER", Some(primase_core::SurfaceClass::Water))
        );
        assert!(c.surfaces.iter().all(|&x| x == c.surfaces[0]));
    }
}
