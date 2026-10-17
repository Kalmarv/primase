//! GTA's water: the reader's side ([`read_water`]: the level's water data, raw, as `gta.water`) and
//! `lower-gta-water`, which makes its water quads drawn surfaces.
//!
//! The data (`common.rpf\data\levels\gta5\water.xml`, the world map's; Cayo Perico's `water_heistisland.xml` replaces
//! it only while a script loads the island, and is not read): water quads, axis-aligned rectangles in GTA ground
//! coordinates at a height `z` (the sea at 0, lakes, rivers and pools above), each the whole rectangle (type 0) or one
//! of its four corner triangles (types 1-4, by the corners (minX, minY), (maxX, minY), (minX, maxY), (maxX, maxY) as
//! 0-3: 1 = 0 1 2, 2 = 0 3 2, 3 = 1 3 2, 4 = 0 1 3), with a per-corner value `a1`-`a4`, invisibility and depth flags;
//! calming quads (a dampening of the waves) and wave quads (amplitude and direction). The tuning file
//! (`common.rpf\data\watertune.xml`) gives, among others, the specular falloff of the water shader.
//!
//! `lower-gta-water{x0,y0,x1,y1}`: every visible water quad (or its triangle), clipped to the region, as a prototype
//! with a flat upward mesh in the core frame and a placement with an identity transform, all with one water material:
//! the earlier exporter's blended colour ([`crate::lower::WATER`], as water shaders get), roughness from the tuning's
//! specular falloff taken as the Blinn-Phong exponent (the water shader is not read: approximated). Without a region
//! every quad is drawn whole. Calming and wave quads, the corner values and the depth flags have no glTF meaning and
//! stay in `gta.water` (counted). The pass removes `gta.water`.
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    Component, Entity, Instance, JsonError, Mesh, Name, Primitive, Semantic, Source, Stream, Topology,
    Transform, World,
};

use crate::archive_set::Install;
use crate::keys::Keys;
use crate::region::GTA;
use crate::{Result, invalid};

/// The world map's water data and the water tuning, as the install stores them.
pub const WATER_FILE: &str = r"common.rpf\data\levels\gta5\water.xml";
pub const TUNE_FILE: &str = r"common.rpf\data\watertune.xml";

/// A water quad, as stored (GTA ground coordinates, metres).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct WaterQuad {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub z: f32,
    /// 0: the whole rectangle; 1-4: one of its corner triangles (see the module documentation).
    pub kind: u8,
    pub invisible: bool,
    pub limited_depth: bool,
    /// `a1`-`a4`, by corner.
    pub corners: [f32; 4],
    pub no_stencil: bool,
}

/// A calming quad: where waves are damped.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct CalmingQuad {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub dampening: f32,
}

/// A wave quad: a wave's amplitude and direction over a rectangle.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct WaveQuad {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub amplitude: f32,
    pub direction: [f32; 2],
}

/// The level's water data, raw, and the tuning's specular falloff.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct GtaWater {
    pub path: String,
    pub quads: Vec<WaterQuad>,
    pub calming: Vec<CalmingQuad>,
    pub waves: Vec<WaveQuad>,
    /// The tuning's `SpecularFalloff`, when the tuning file has one.
    pub specular_falloff: Option<f32>,
}

impl Component for GtaWater {
    const NAME: &'static str = "gta.water";
    /// A summary: the file and how many quads of each kind.
    fn to_json(&self) -> std::result::Result<Option<serde_json::Value>, JsonError> {
        Ok(Some(serde_json::json!({
            "path": self.path,
            "water_quads": self.quads.len(),
            "calming_quads": self.calming.len(),
            "wave_quads": self.waves.len(),
            "specular_falloff": self.specular_falloff,
        })))
    }
}

fn err(detail: impl ToString) -> crate::Error {
    invalid("water", 0, detail)
}

/// The world map's water data ([`WATER_FILE`]) and the tuning's specular falloff ([`TUNE_FILE`]), as stored.
pub fn read_water(install: &Install, keys: &Keys) -> Result<GtaWater> {
    let bytes = install
        .read(WATER_FILE, keys)?
        .ok_or_else(|| err(format!("{WATER_FILE} not found")))?;
    let mut w = parse_water(&bytes)?;
    w.path = WATER_FILE.to_string();
    if let Some(tune) = install.read(TUNE_FILE, keys)? {
        let root = crate::xml::parse(&tune, TUNE_FILE)?;
        w.specular_falloff = root.child_value("SpecularFalloff").and_then(|v| v.parse().ok());
    }
    Ok(w)
}

/// Water data from `water.xml` bytes (`path` empty, no tuning).
pub fn parse_water(bytes: &[u8]) -> Result<GtaWater> {
    let root = crate::xml::parse(bytes, "water.xml")?;
    let num = |e: &crate::xml::Element, k: &str| -> Result<f32> {
        e.child_value(k)
            .ok_or_else(|| err(format!("a quad without {k}")))?
            .parse()
            .map_err(|_| err(format!("{k}: not a number")))
    };
    let flag =
        |e: &crate::xml::Element, k: &str| e.child_value(k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let rect = |e: &crate::xml::Element| -> Result<([f32; 2], [f32; 2])> {
        Ok((
            [num(e, "minX")?, num(e, "minY")?],
            [num(e, "maxX")?, num(e, "maxY")?],
        ))
    };
    let section = |name: &str| root.child(name).into_iter().flat_map(|s| s.items());
    let mut quads = Vec::new();
    for q in section("WaterQuads") {
        let (min, max) = rect(q)?;
        let kind = num(q, "Type")? as u8;
        if kind > 4 {
            return Err(err(format!("water quad type {kind}")));
        }
        quads.push(WaterQuad {
            min,
            max,
            z: num(q, "z")?,
            kind,
            invisible: flag(q, "IsInvisible"),
            limited_depth: flag(q, "HasLimitedDepth"),
            corners: [num(q, "a1")?, num(q, "a2")?, num(q, "a3")?, num(q, "a4")?],
            no_stencil: flag(q, "NoStencil"),
        });
    }
    let mut calming = Vec::new();
    for q in section("CalmingQuads") {
        let (min, max) = rect(q)?;
        calming.push(CalmingQuad {
            min,
            max,
            dampening: num(q, "fDampening")?,
        });
    }
    let mut waves = Vec::new();
    for q in section("WaveQuads") {
        let (min, max) = rect(q)?;
        waves.push(WaveQuad {
            min,
            max,
            amplitude: num(q, "Amplitude")?,
            direction: [num(q, "XDirection")?, num(q, "YDirection")?],
        });
    }
    Ok(GtaWater {
        path: String::new(),
        quads,
        calming,
        waves,
        specular_falloff: None,
    })
}

/// A quad's outline (GTA ground coordinates), counter-clockwise seen from above.
pub fn outline(q: &WaterQuad) -> Vec<[f32; 2]> {
    let c = [
        [q.min[0], q.min[1]],
        [q.max[0], q.min[1]],
        [q.min[0], q.max[1]],
        [q.max[0], q.max[1]],
    ];
    let mut p: Vec<[f32; 2]> = match q.kind {
        0 => vec![c[0], c[1], c[3], c[2]],
        1 => vec![c[0], c[1], c[2]],
        2 => vec![c[0], c[3], c[2]],
        3 => vec![c[1], c[3], c[2]],
        _ => vec![c[0], c[1], c[3]],
    };
    if area(&p) < 0.0 {
        p.reverse();
    }
    p
}

/// Twice the signed area of a polygon (positive: counter-clockwise).
fn area(p: &[[f32; 2]]) -> f32 {
    (0..p.len())
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

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

/// `lower-gta-water` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LowerGtaWater {
    /// `[x0, y0, x1, y1]`, GTA ground coordinates; `None`: every quad whole.
    pub region: Option<[f32; 4]>,
}

impl LowerGtaWater {
    pub fn from_options(o: &Options) -> std::result::Result<LowerGtaWater, String> {
        for k in o.keys() {
            if !["x0", "y0", "x1", "y1"].contains(&k.as_str()) {
                return Err(format!("unknown option {k}"));
            }
        }
        let num = |k: &str| -> std::result::Result<Option<f32>, String> {
            o.get(k)
                .map(|v| v.parse().map_err(|_| format!("{k}: not a number")))
                .transpose()
        };
        let region = match [num("x0")?, num("y0")?, num("x1")?, num("y1")?] {
            [Some(a), Some(b), Some(c), Some(d)] => Some([a, b, c, d]),
            [None, None, None, None] => None,
            _ => return Err("x0, y0, x1 and y1 go together".into()),
        };
        Ok(LowerGtaWater { region })
    }
}

/// The water surface material (see the module documentation).
pub fn water_material(specular_falloff: Option<f32>) -> PbrMaterial {
    let srgb = |b: u8| primase_image::srgb_to_linear(f64::from(b) / 255.0) as f32;
    let w = crate::lower::WATER;
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
    fn options(&self) -> Options {
        let mut o = Options::new();
        if let Some([x0, y0, x1, y1]) = self.region {
            for (k, v) in [("x0", x0), ("y0", y0), ("x1", x1), ("y1", y1)] {
                o.insert(k.to_string(), v.to_string());
            }
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["gta.water"]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            "core.mesh",
            "core.instance",
            "core.transform",
            "pbr.material",
            "core.name",
            "core.source",
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &["gta.water"]
    }
    fn approximates(&self) -> &'static str {
        "the water shader as the earlier exporter's blended flat colour, its roughness from the tuning's specular \
         falloff; waves, calming, corner values and depth flags not drawn"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let waters: Vec<(Entity, GtaWater)> = w.iter::<GtaWater>().map(|(e, g)| (e, g.clone())).collect();
        for (e, water) in waters {
            w.remove::<GtaWater>(e);
            let material = w.spawn();
            w.insert(material, water_material(water.specular_falloff));
            w.insert(material, Name("gta_water".into()));
            log.approximate("water drawn as a blended flat colour", 1);
            log.count("calming quads not drawn", water.calming.len() as u64);
            log.count("wave quads not drawn", water.waves.len() as u64);
            let mut drawn = 0;
            for (i, q) in water.quads.iter().enumerate() {
                if q.invisible {
                    log.count("invisible water quads left out", 1);
                    continue;
                }
                let poly = match self.region {
                    Some(r) => clip(&outline(q), r),
                    None => outline(q),
                };
                if poly.is_empty() {
                    continue;
                }
                let positions: Vec<[f32; 3]> = poly.iter().map(|p| GTA.point([p[0], p[1], q.z])).collect();
                let up = GTA.direction([0.0, 0.0, 1.0]);
                let indices: Vec<u32> = (1..poly.len() as u32 - 1).flat_map(|k| [0, k, k + 1]).collect();
                let mut streams = std::collections::BTreeMap::new();
                streams.insert(Semantic::Normal, Stream::F32x3(vec![up; positions.len()]));
                streams.insert(Semantic::Position, Stream::F32x3(positions));
                let proto = w.spawn();
                w.insert(
                    proto,
                    Mesh {
                        primitives: vec![Primitive {
                            topology: Topology::Triangles,
                            streams,
                            indices: Some(indices),
                            material: Some(material),
                        }],
                    },
                );
                w.insert(proto, Name(format!("gta_water_{i}")));
                let placed = w.spawn();
                w.insert(
                    placed,
                    Instance {
                        prototype: proto,
                        variant: 0,
                    },
                );
                w.insert(placed, Transform::IDENTITY);
                w.insert(placed, Source(format!("{}#water_quad_{i}", water.path)));
                drawn += 1;
                if q.z == 0.0 {
                    log.count("water surfaces: sea", 1);
                } else {
                    log.count("water surfaces: above sea level (lakes, rivers, pools)", 1);
                }
            }
            log.count(
                "water quads outside the region",
                (water.quads.len() - drawn) as u64,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn outlines_are_the_rectangle_or_its_corner_triangles_counter_clockwise() {
        assert_eq!(
            outline(&quad(0)),
            vec![[0.0, 0.0], [10.0, 0.0], [10.0, 20.0], [0.0, 20.0]]
        );
        // type 1: corners 0 1 2, the triangle without (maxX, maxY)
        let t = outline(&quad(1));
        assert_eq!(t.len(), 3);
        assert!(!t.contains(&[10.0, 20.0]));
        // type 3: corners 1 3 2, without (minX, minY)
        assert!(!outline(&quad(3)).contains(&[0.0, 0.0]));
        for k in 0..=4 {
            assert!(area(&outline(&quad(k))) > 0.0, "type {k} counter-clockwise");
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

    #[test]
    fn water_xml_reads_its_three_kinds_of_quads() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<WaterData>
  <WaterQuads>
    <Item><minX value="-2" /><maxX value="2" /><minY value="0" /><maxY value="4" /><Type value="2" />
      <IsInvisible value="false" /><HasLimitedDepth value="true" /><z value="1.5" />
      <a1 value="26" /><a2 value="26" /><a3 value="0" /><a4 value="26" /><NoStencil value="false" /></Item>
  </WaterQuads>
  <CalmingQuads>
    <Item><minX value="0" /><maxX value="1" /><minY value="0" /><maxY value="1" /><fDampening value="0.5" /></Item>
  </CalmingQuads>
  <WaveQuads>
    <Item><minX value="0" /><maxX value="1" /><minY value="0" /><maxY value="1" /><Amplitude value="2" />
      <XDirection value="0.5" /><YDirection value="-0.5" /></Item>
  </WaveQuads>
</WaterData>"#;
        let w = parse_water(xml).unwrap();
        assert_eq!(w.quads.len(), 1);
        let q = w.quads[0];
        assert_eq!(
            (q.min, q.max, q.z, q.kind, q.limited_depth),
            ([-2.0, 0.0], [2.0, 4.0], 1.5, 2, true)
        );
        assert_eq!(q.corners, [26.0, 26.0, 0.0, 26.0]);
        assert_eq!(w.calming[0].dampening, 0.5);
        assert_eq!((w.waves[0].amplitude, w.waves[0].direction), (2.0, [0.5, -0.5]));
    }

    #[test]
    fn the_pass_places_one_surface_per_quad_in_the_region() {
        let mut w = World::new();
        let e = w.spawn();
        let mut water = parse_water(
            br#"<WaterData><WaterQuads>
<Item><minX value="0" /><maxX value="10" /><minY value="0" /><maxY value="10" /><Type value="0" />
<IsInvisible value="false" /><HasLimitedDepth value="false" /><z value="0" />
<a1 value="26" /><a2 value="26" /><a3 value="26" /><a4 value="26" /><NoStencil value="false" /></Item>
<Item><minX value="100" /><maxX value="110" /><minY value="0" /><maxY value="10" /><Type value="0" />
<IsInvisible value="false" /><HasLimitedDepth value="false" /><z value="0" />
<a1 value="26" /><a2 value="26" /><a3 value="26" /><a4 value="26" /><NoStencil value="false" /></Item>
</WaterQuads></WaterData>"#,
        )
        .unwrap();
        water.specular_falloff = Some(1118.0);
        w.insert(e, water);
        let mut log = PassLog::default();
        LowerGtaWater {
            region: Some([-5.0, -5.0, 5.0, 5.0]),
        }
        .run(&mut w, &mut log)
        .unwrap();
        assert!(w.get::<GtaWater>(e).is_none());
        let placed: Vec<_> = w.iter::<Instance>().collect();
        assert_eq!(placed.len(), 1);
        let mesh = w.get::<Mesh>(placed[0].1.prototype).unwrap();
        let p = mesh.primitives[0].positions().unwrap();
        // the 5 x 5 corner of the first quad, flat at the sea's height (core y up)
        assert!(p.iter().all(|v| v[1] == 0.0));
        let m = w
            .get::<PbrMaterial>(mesh.primitives[0].material.unwrap())
            .unwrap();
        assert_eq!(m.alpha_mode, AlphaMode::Blend);
        assert!((m.roughness - (2.0f32 / 1120.0).powf(0.25)).abs() < 1e-6);
    }
}
