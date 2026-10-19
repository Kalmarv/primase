//! `prepare-reskate-water{extent=none|all|region|content|x0,y0,x1,y1, margin=M, ocean=stated|all}`: the skate water
//! surfaces a map writes, and which water materials draw as Studio's ocean.
//!
//! - **Extent** (required): `none` leaves every water surface out, `all` keeps every one as given; `region` keeps
//!   those that touch the scene's stated region (`skate.scene` region; an error without one), `content` those that
//!   touch the map's content as the writer measures it (the ground bounds of every placement's position, its
//!   prototype's box corners and the world collision, whole metres out), `x0,y0,x1,y1` those that touch that ground
//!   rectangle; each kept outline is clipped to
//!   the rectangle grown by `margin` metres on every side, and what is left with less than 1 m² is left out
//!   (counted). The writer used to do this itself over the region, else the content, with a 400 m margin. Outlines stay convex: a
//!   convex outline clipped by a rectangle is convex; one the source gives not convex is kept and reported (the
//!   writer draws an outline as a fan from its first corner).
//! - **Ocean** (`stated` unless given): `all` makes every water-class skate material that states nothing draw as
//!   Studio's ocean (`ocean: true`); a material that states `ocean` keeps it.
//!
//! Marker snaps (`snap-reskate-markers`) see the water outlines as they are when the snap runs: run it before this
//! pass to snap against the uncut water, as the writer did.
//!
//! The water is the map's: a part of a conversion in several is refused. Without this pass the writer writes every
//! water surface it is given, as given (clipped only to the map's extent limit), and no material as ocean unless it
//! says so.

use std::path::Path;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Entity, World};

use crate::build::{bounds, writer_placements};
use crate::dialect::{MaterialClass, SkateMaterial, SkateScene, SkateWater};
use crate::scene::{Scene, WaterQuad};

/// Which water surfaces [`PrepareReskateWater`] keeps, and where it clips them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaterExtent {
    /// None: every water surface left out.
    None,
    /// Every water surface, as given.
    All,
    /// Those touching the scene's stated region, clipped to it grown by `margin` metres.
    Region { margin: f32 },
    /// Those touching the map's content (placements and world collision) as the writer measures it, clipped to its
    /// ground bounds grown by `margin` metres.
    Content { margin: f32 },
    /// Those touching ground rectangle `[x0, y0, x1, y1]`, clipped to it grown by `margin` metres.
    Rect { rect: [f32; 4], margin: f32 },
}

/// Which water materials [`PrepareReskateWater`] makes draw as Studio's ocean.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OceanMaterials {
    /// Those that say so (`ocean: true`).
    #[default]
    Stated,
    /// Every water-class material that states nothing, too.
    All,
}

/// See the module documentation. Made by [`PrepareReskateWater::new`]; from options by the same call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrepareReskateWater {
    extent: WaterExtent,
    ocean: OceanMaterials,
}

impl PrepareReskateWater {
    /// An error for a margin that is not a finite number of metres, 0 or more, or a rectangle that is not finite
    /// with x0 < x1 and y0 < y1.
    pub fn new(extent: WaterExtent, ocean: OceanMaterials) -> Result<PrepareReskateWater, String> {
        let margin = match extent {
            WaterExtent::None | WaterExtent::All => 0.0,
            WaterExtent::Region { margin } | WaterExtent::Content { margin } => margin,
            WaterExtent::Rect { rect, margin } => {
                let [x0, y0, x1, y1] = rect;
                if !rect.iter().all(|v| v.is_finite()) || x0 >= x1 || y0 >= y1 {
                    return Err(format!(
                        "rectangle {rect:?}: x0, y0, x1, y1 finite, with x0 < x1 and y0 < y1"
                    ));
                }
                margin
            }
        };
        if !(margin.is_finite() && margin >= 0.0) {
            return Err(format!("margin {margin} is not a number of metres, 0 or more"));
        }
        Ok(PrepareReskateWater { extent, ocean })
    }

    pub fn extent(&self) -> WaterExtent {
        self.extent
    }

    pub fn ocean(&self) -> OceanMaterials {
        self.ocean
    }

    /// From its options, through [`PrepareReskateWater::new`]: `extent=none`, `extent=all`, `extent=region` or
    /// `extent=content` with `margin`, or `x0,y0,x1,y1` with `margin`; `ocean=stated|all`.
    pub fn from_options(o: &Options) -> Result<PrepareReskateWater, String> {
        const KEYS: [&str; 7] = ["extent", "margin", "x0", "y0", "x1", "y1", "ocean"];
        if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let num = |k: &str| -> Result<Option<f32>, String> {
            o.get(k)
                .map(|v| v.parse().map_err(|_| format!("{k}={v} is not a number")))
                .transpose()
        };
        let margin = || -> Result<f32, String> {
            num("margin")?.ok_or_else(|| {
                "margin is required (metres) with a region, the content or a rectangle".to_owned()
            })
        };
        let rect = [num("x0")?, num("y0")?, num("x1")?, num("y1")?];
        let extent = match (o.get("extent").map(String::as_str), rect) {
            (Some("none"), [None, None, None, None]) if !o.contains_key("margin") => WaterExtent::None,
            (Some("all"), [None, None, None, None]) if !o.contains_key("margin") => WaterExtent::All,
            (Some("region"), [None, None, None, None]) => WaterExtent::Region { margin: margin()? },
            (Some("content"), [None, None, None, None]) => WaterExtent::Content { margin: margin()? },
            (None, [Some(x0), Some(y0), Some(x1), Some(y1)]) => WaterExtent::Rect {
                rect: [x0, y0, x1, y1],
                margin: margin()?,
            },
            (Some(v), _) if !["none", "all", "region", "content"].contains(&v) => {
                return Err(format!(
                    "extent={v}: none, all, region or content (or x0, y0, x1, y1)"
                ));
            }
            _ => {
                return Err(
                    "the extent is required: extent=none, extent=all, extent=region or extent=content with margin, \
                     or x0, y0, x1, y1 with margin"
                        .into(),
                );
            }
        };
        let ocean = match o.get("ocean").map(String::as_str) {
            None | Some("stated") => OceanMaterials::Stated,
            Some("all") => OceanMaterials::All,
            Some(v) => return Err(format!("ocean={v}: stated or all")),
        };
        PrepareReskateWater::new(extent, ocean)
    }
}

/// [`PrepareReskateWater::from_options`] as a pass factory.
pub fn prepare_reskate_water(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateWater::from_options(o)?))
}

/// Sutherland-Hodgman, one step: `p` cut at `k` along x (or y), keeping the side above (or below) it.
pub(crate) fn clip_half(p: &[(f32, f32)], along_y: bool, k: f32, keep_above: bool) -> Vec<(f32, f32)> {
    let mut o = Vec::with_capacity(p.len() + 1);
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        let (mut da, mut db) = (
            (if along_y { a.1 } else { a.0 }) - k,
            (if along_y { b.1 } else { b.0 }) - k,
        );
        if !keep_above {
            da = -da;
            db = -db;
        }
        if da >= 0.0 {
            o.push(a);
        }
        if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
            let t = da / (da - db);
            o.push(if along_y {
                (a.0 + (b.0 - a.0) * t, k)
            } else {
                (k, a.1 + (b.1 - a.1) * t)
            });
        }
    }
    o
}

/// `p` clipped to `[x0, y0, x1, y1]`: x from below, x from above, y from below, y from above.
pub(crate) fn clip_rect(p: &[(f32, f32)], [x0, y0, x1, y1]: [f32; 4]) -> Vec<(f32, f32)> {
    clip_half(
        &clip_half(
            &clip_half(&clip_half(p, false, x0, true), false, x1, false),
            true,
            y0,
            true,
        ),
        true,
        y1,
        false,
    )
}

/// Twice the signed area of outline `p`.
pub(crate) fn signed_area2(p: &[(f32, f32)]) -> f32 {
    let mut a = 0f32;
    for i in 0..p.len() {
        let (q, n) = (p[i], p[(i + 1) % p.len()]);
        a += q.0 * n.1 - n.0 * q.1;
    }
    a
}

/// Whether outline `p` turns one way only (collinear corners allowed).
fn convex(p: &[(f32, f32)]) -> bool {
    let n = p.len();
    let (mut pos, mut neg) = (false, false);
    for i in 0..n {
        let (a, b, c) = (p[i], p[(i + 1) % n], p[(i + 2) % n]);
        let z = (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0);
        pos |= z > 0.0;
        neg |= z < 0.0;
    }
    !(pos && neg)
}

impl Pass for PrepareReskateWater {
    fn name(&self) -> &'static str {
        "prepare-reskate-water"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        match self.extent {
            WaterExtent::None => {
                o.insert("extent".into(), "none".into());
            }
            WaterExtent::All => {
                o.insert("extent".into(), "all".into());
            }
            WaterExtent::Region { margin } => {
                o.insert("extent".into(), "region".into());
                o.insert("margin".into(), margin.to_string());
            }
            WaterExtent::Content { margin } => {
                o.insert("extent".into(), "content".into());
                o.insert("margin".into(), margin.to_string());
            }
            WaterExtent::Rect { rect, margin } => {
                for (k, v) in ["x0", "y0", "x1", "y1"].into_iter().zip(rect) {
                    o.insert(k.into(), v.to_string());
                }
                o.insert("margin".into(), margin.to_string());
            }
        }
        o.insert(
            "ocean".into(),
            match self.ocean {
                OceanMaterials::Stated => "stated",
                OceanMaterials::All => "all",
            }
            .into(),
        );
        o
    }
    fn modifies(&self) -> &[&'static str] {
        &[SkateWater::NAME, SkateMaterial::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "a water outline the source gives not convex, drawn as a fan from its first corner"
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        super::require_one_part(w, self.name())?;
        if self.ocean == OceanMaterials::All
            && let Some(mut col) = w.column_mut::<SkateMaterial>()
        {
            for (_, m) in col.iter_mut() {
                if m.class == MaterialClass::Water && m.ocean.is_none() {
                    m.ocean = Some(true);
                    log.count("water materials drawn as Studio's ocean (ocean=all)", 1);
                }
            }
        }
        let water: Vec<Entity> = w.iter::<SkateWater>().map(|(e, _)| e).collect();
        let (r, margin): ([f64; 4], f32) = match self.extent {
            WaterExtent::None => {
                log.count("water surfaces left out: extent=none", water.len() as u64);
                for e in water {
                    w.despawn(e);
                }
                return Ok(());
            }
            WaterExtent::All => {
                for &e in &water {
                    let s = w.get::<SkateWater>(e).expect("listed");
                    let q: Vec<(f32, f32)> = s.corners.iter().map(|c| (c[0], c[1])).collect();
                    if !convex(&q) {
                        log.approximate(
                            "water outlines not convex, drawn as a fan from their first corner",
                            1,
                        );
                    }
                }
                log.count("water surfaces kept", water.len() as u64);
                return Ok(());
            }
            WaterExtent::Region { margin } => {
                let region = w
                    .resource::<SkateScene>()
                    .and_then(|s| s.region)
                    .ok_or("extent=region: the scene states no region (extent=content, or x0, y0, x1, y1)")?;
                (region.map(f64::from), margin)
            }
            WaterExtent::Content { margin } => {
                if water.is_empty() {
                    return Ok(());
                }
                // the writer's measure: the placements as it takes them (positions and box corners) and the soups
                let s = Scene::from_world(w, Path::new("")).map_err(|e| e.to_string())?;
                (bounds(&s, &writer_placements(&s, 200.0)), margin)
            }
            WaterExtent::Rect { rect, margin } => (rect.map(f64::from), margin),
        };
        let grown = [
            r[0] as f32 - margin,
            r[1] as f32 - margin,
            r[2] as f32 + margin,
            r[3] as f32 + margin,
        ];
        for e in water {
            let s = w.get::<SkateWater>(e).expect("listed");
            let height = s.height.unwrap_or(0.0) as f32;
            let q = WaterQuad::of(s.corners.iter().map(|c| (c[0], c[1])).collect(), height);
            let touches = !(f64::from(q.x1) <= r[0]
                || f64::from(q.x0) >= r[2]
                || f64::from(q.y1) <= r[1]
                || f64::from(q.y0) >= r[3]);
            if !touches {
                log.count("water surfaces left out: outside the extent", 1);
                w.despawn(e);
                continue;
            }
            if !convex(&q.corners) {
                log.approximate(
                    "water outlines not convex, drawn as a fan from their first corner",
                    1,
                );
            }
            let cs = clip_rect(&q.corners, grown);
            if cs.len() < 3 || signed_area2(&cs).abs() < 1.0 {
                log.count("water surfaces left out: less than 1 m2 within the margin", 1);
                w.despawn(e);
                continue;
            }
            if cs != q.corners {
                log.count("water surfaces clipped to the extent and its margin", 1);
            }
            let s = w.get_mut::<SkateWater>(e).expect("listed");
            s.corners = cs.into_iter().map(|(x, y)| [x, y]).collect();
            log.count("water surfaces kept", 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water(w: &mut World, corners: &[[f32; 2]]) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            SkateWater {
                corners: corners.to_vec(),
                height: Some(2.0),
                other: Default::default(),
            },
        );
        e
    }

    fn scene(region: Option<[f32; 4]>) -> SkateScene {
        SkateScene {
            name: "t".into(),
            region,
            ..Default::default()
        }
    }

    /// Worked by hand, region 0..100 x 0..100 grown by 10: a square from -50 to 50 is cut to -10..50 (the clip rotates
    /// its corners, as the writer's did); one wholly beyond the region's east edge (east 105..200, inside the margin but
    /// not touching the region) is left out, as the writer left it; a sliver keeping 1.1 m2 within the margin stays, one
    /// keeping 0.2 m2 within a 1 m margin is left out; extent=none leaves all out.
    #[test]
    fn surfaces_touching_the_region_are_clipped_to_its_margin() {
        let mut w = World::new();
        w.insert_resource(scene(Some([0.0, 0.0, 100.0, 100.0])));
        let cut = water(
            &mut w,
            &[[-50.0, -50.0], [50.0, -50.0], [50.0, 50.0], [-50.0, 50.0]],
        );
        let beyond = water(
            &mut w,
            &[[105.0, 0.0], [200.0, 0.0], [200.0, 50.0], [105.0, 50.0]],
        );
        let sliver = water(
            &mut w,
            &[[99.0, 10.0], [110.0, 10.0], [110.0, 10.1], [99.0, 10.1]],
        );
        let mut log = PassLog::default();
        PrepareReskateWater::new(WaterExtent::Region { margin: 10.0 }, OceanMaterials::Stated)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        assert_eq!(
            w.get::<SkateWater>(cut).unwrap().corners,
            // the clip starts the outline where the writer's did
            [[50.0, -10.0], [50.0, 50.0], [-10.0, 50.0], [-10.0, -10.0]]
        );
        assert!(!w.is_alive(beyond));
        assert!(w.is_alive(sliver), "1.1 m2 inside: kept");
        assert_eq!(log.counts["water surfaces left out: outside the extent"], 1);
        let tiny = water(
            &mut w,
            &[[99.0, 20.0], [104.0, 20.0], [104.0, 20.1], [99.0, 20.1]],
        );
        PrepareReskateWater::new(WaterExtent::Region { margin: 1.0 }, OceanMaterials::Stated)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert!(!w.is_alive(tiny), "0.2 m2 within a 1 m margin: left out");
        PrepareReskateWater::new(WaterExtent::None, OceanMaterials::Stated)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert_eq!(w.iter::<SkateWater>().count(), 0);
    }

    /// extent=content measures the map as the writer does: world collision over east 0..512, north 0..256 (no
    /// placements) gives the bounds 0..513 x 0..257 (whole metres out), grown by 10. A strip from east -50 to 50 is cut
    /// at -10; one from east 520 does not touch the bounds and is left out.
    #[test]
    fn the_content_is_measured_as_the_writer_measures_it() {
        let mut w = World::new();
        let mut surfaces = primase_core::Surfaces::default();
        let concrete = surfaces.index("concrete").unwrap();
        w.insert_resource(surfaces);
        let ground = w.spawn();
        let mut c = primase_core::Collision::default();
        c.extend(
            &[
                [0.0, 0.0, 0.0],
                [512.0, 0.0, 0.0],
                [0.0, 0.0, -256.0],
                [512.0, 0.0, -256.0],
            ],
            &[[0, 1, 2], [2, 1, 3]],
            concrete,
            |p| p,
        );
        w.insert(ground, c);
        crate::prepare::raise_skate(
            &mut w,
            &crate::prepare::RaiseOptions {
                name: "content".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(w.resource::<SkateScene>().unwrap().region.is_none());
        let strip = water(
            &mut w,
            &[[-50.0, 100.0], [50.0, 100.0], [50.0, 120.0], [-50.0, 120.0]],
        );
        let east = water(
            &mut w,
            &[[520.0, 0.0], [600.0, 0.0], [600.0, 50.0], [520.0, 50.0]],
        );
        let mut log = PassLog::default();
        PrepareReskateWater::new(WaterExtent::Content { margin: 10.0 }, OceanMaterials::Stated)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        assert_eq!(
            w.get::<SkateWater>(strip).unwrap().corners,
            [[-10.0, 100.0], [50.0, 100.0], [50.0, 120.0], [-10.0, 120.0]]
        );
        assert!(!w.is_alive(east));
        assert_eq!(log.counts["water surfaces left out: outside the extent"], 1);
    }

    /// A rectangle needs no scene region, extent=region does; a non-convex outline is kept and reported.
    #[test]
    fn a_rectangle_or_the_scenes_region_and_non_convex_outlines_reported() {
        let mut w = World::new();
        let ell = water(
            &mut w,
            &[
                [0.0, 0.0],
                [20.0, 0.0],
                [20.0, 10.0],
                [10.0, 10.0],
                [10.0, 20.0],
                [0.0, 20.0],
            ],
        );
        let pass = |extent| PrepareReskateWater::new(extent, OceanMaterials::Stated).unwrap();
        assert!(
            pass(WaterExtent::Region { margin: 0.0 })
                .run(&mut w, &mut PassLog::default())
                .unwrap_err()
                .contains("no region")
        );
        let mut log = PassLog::default();
        pass(WaterExtent::Rect {
            rect: [-5.0, -5.0, 50.0, 50.0],
            margin: 0.0,
        })
        .run(&mut w, &mut log)
        .unwrap();
        assert!(w.is_alive(ell));
        assert_eq!(
            log.approximations["water outlines not convex, drawn as a fan from their first corner"],
            1
        );
    }

    /// ocean=all marks water materials that state nothing; one that states false keeps it; others untouched.
    #[test]
    fn ocean_all_marks_the_water_materials_that_state_nothing() {
        let mut w = World::new();
        let mat = |w: &mut World, class: MaterialClass, ocean: Option<bool>| {
            let e = w.spawn();
            w.insert(
                e,
                SkateMaterial {
                    key: format!("{class:?}{ocean:?}"),
                    class,
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
                    ocean,
                    cast_shadows: None,
                    emissive: None,
                    vertex_alpha: None,
                    other: Default::default(),
                },
            );
            e
        };
        let (a, b, c) = (
            mat(&mut w, MaterialClass::Water, None),
            mat(&mut w, MaterialClass::Water, Some(false)),
            mat(&mut w, MaterialClass::Opaque, None),
        );
        PrepareReskateWater::new(WaterExtent::None, OceanMaterials::All)
            .unwrap()
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        let ocean = |e| w.get::<SkateMaterial>(e).unwrap().ocean;
        assert_eq!((ocean(a), ocean(b), ocean(c)), (Some(true), Some(false), None));
    }

    /// The extent is required, the margin with it; the struct and its options are one.
    #[test]
    fn options_round_trip_and_bad_ones_refused() {
        let o = |kv: &[(&str, &str)]| -> Options {
            kv.iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        for good in [
            o(&[("extent", "none"), ("ocean", "stated")]),
            o(&[("extent", "all"), ("ocean", "all")]),
            o(&[("extent", "region"), ("margin", "400"), ("ocean", "all")]),
            o(&[("extent", "content"), ("margin", "400"), ("ocean", "stated")]),
            o(&[
                ("x0", "0"),
                ("y0", "0"),
                ("x1", "10"),
                ("y1", "10"),
                ("margin", "0"),
                ("ocean", "stated"),
            ]),
        ] {
            assert_eq!(PrepareReskateWater::from_options(&good).unwrap().options(), good);
        }
        for bad in [
            o(&[]),
            o(&[("extent", "region")]),
            o(&[("extent", "region"), ("margin", "-1")]),
            o(&[("extent", "content")]),
            o(&[("extent", "none"), ("margin", "5")]),
            o(&[("extent", "sea")]),
            o(&[
                ("x0", "0"),
                ("y0", "0"),
                ("x1", "0"),
                ("y1", "10"),
                ("margin", "0"),
            ]),
            o(&[("extent", "none"), ("ocean", "some")]),
        ] {
            assert!(PrepareReskateWater::from_options(&bad).is_err(), "{bad:?}");
        }
    }
}
