//! GTA's water as the reader keeps it ([`attach_water`]): every water file, each quad placed as a
//! `gta.water_surface`, the rest of the file as a `gta.water`.
//!
//! The data: the game holds one water file at a time ([`WATER_FILES`]), the world map's
//! (`common.rpf\data\levels\gta5\water.xml`) or, while a script loads the island, Cayo Perico's
//! (`water_heistisland.xml`, with its own tuning). Both are read; which one holds is the state axis `gta.water`
//! (default the world map's), and each file's placed surfaces carry its condition, so `resolve-state` picks the file
//! like any other state. A file: water quads, axis-aligned rectangles in GTA ground coordinates at a height `z` (the
//! sea at 0, lakes, rivers and pools above), each the whole rectangle (type 0) or one of its four corner triangles
//! (types 1-4, by the corners (minX, minY), (maxX, minY), (minX, maxY), (maxX, maxY) as 0-3: 1 = 0 1 2, 2 = 0 3 2,
//! 3 = 1 3 2, 4 = 0 1 3), with a per-corner value `a1`-`a4`, invisibility, depth and stencil flags; calming quads (a
//! dampening of the waves) and wave quads (amplitude and direction). The tuning file (`common.rpf\data\watertune.xml`,
//! the island's `watertune_heistisland.xml`) gives, among others, the specular falloff of the water shader.
//!
//! The reader places every quad as authored: a prototype holding its outline and the quad as stored
//! (`gta.water_surface`), placed once with an identity transform; the file's calming and wave quads and its path stay
//! in a `gta.water` entity of their own.
//!
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{Instance, Name, Source, Transform, World};

use crate::components::water::{CalmingQuad, GtaWater, GtaWaterSurface, WaterQuad, WaveQuad, outline};
use crate::read::archive::archive_set::Install;
use crate::read::archive::keys::Keys;
use crate::{Result, invalid};

/// The world map's water data and the water tuning, as the install stores them.
pub const WATER_FILE: &str = r"common.rpf\data\levels\gta5\water.xml";
pub const TUNE_FILE: &str = r"common.rpf\data\watertune.xml";

/// The water files the game holds one at a time: the `gta.water` axis's value name, the water data and its tuning.
/// The first is the world map's (required; the axis's default); the others are read when the install has them.
pub const WATER_FILES: [(&str, &str, &str); 2] = [
    ("world", WATER_FILE, TUNE_FILE),
    (
        "heistisland",
        r"update\update.rpf\common\data\levels\gta5\water_heistisland.xml",
        r"update\update.rpf\common\data\watertune_heistisland.xml",
    ),
];

/// A water file as stored, with its tuning's specular falloff.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterFile {
    pub path: String,
    pub quads: Vec<WaterQuad>,
    pub calming: Vec<CalmingQuad>,
    pub waves: Vec<WaveQuad>,
    /// The tuning's `SpecularFalloff`, when the tuning file has one.
    pub specular_falloff: Option<f32>,
}

fn err(detail: impl ToString) -> crate::Error {
    invalid("water", 0, detail)
}

/// Each water file of [`WATER_FILES`] the install has (the world map's is required) with its value name on
/// `gta.water` and its tuning's specular falloff, as stored.
pub fn read_water(install: &Install, keys: &Keys) -> Result<Vec<(&'static str, WaterFile)>> {
    let mut out = Vec::new();
    for (i, (value, path, tune_path)) in WATER_FILES.into_iter().enumerate() {
        let Some(bytes) = install.read(path, keys)? else {
            if i == 0 {
                return Err(err(format!("{path} not found")));
            }
            continue;
        };
        let mut w = parse_water(&bytes)?;
        w.path = path.to_string();
        if let Some(tune) = install.read(tune_path, keys)? {
            w.specular_falloff = specular_falloff(&tune, tune_path)?;
        }
        out.push((value, w));
    }
    Ok(out)
}

/// A tuning file's `SpecularFalloff`: `None` when it has none, an error when it is not a number.
pub fn specular_falloff(tune: &[u8], path: &str) -> Result<Option<f32>> {
    let root = crate::read::xml::parse(tune, path)?;
    root.child_value("SpecularFalloff")
        .map(|v| {
            v.parse()
                .map_err(|_| err(format!("{path}: SpecularFalloff {v:?} is not a number")))
        })
        .transpose()
}

/// Every water file the install has ([`read_water`]) into the world (see the module documentation): with more than
/// one, the `gta.water` axis (default the world map's); each quad a placed `gta.water_surface` (with its file's
/// condition), the rest of each file a `gta.water`. Returns each file's path with its water, calming and wave quad
/// counts.
pub fn attach_water(w: &mut World, install: &Install, keys: &Keys) -> Result<Vec<(String, [usize; 3])>> {
    let files = read_water(install, keys)?;
    Ok(place_water(w, files))
}

/// [`attach_water`] for files already read.
pub fn place_water(w: &mut World, files: Vec<(&str, WaterFile)>) -> Vec<(String, [usize; 3])> {
    let axis = (files.len() > 1).then(|| {
        let e = w.spawn();
        w.insert(
            e,
            StateAxis {
                key: "gta.water".into(),
                domain: Domain::Enum(files.iter().map(|(v, _)| v.to_string()).collect()),
                default: Some(0),
                scope: Scope::Global,
                class: "water".into(),
                source: "the water file the game holds: the world map's, or Cayo Perico's while a script loads the \
                         island"
                    .into(),
            },
        );
        e
    });
    let mut counts = Vec::new();
    for (i, (value, file)) in files.into_iter().enumerate() {
        counts.push((
            file.path.clone(),
            [file.quads.len(), file.calming.len(), file.waves.len()],
        ));
        let condition = axis.map(|axis| {
            Condition(vec![Clause(vec![Literal {
                axis,
                values: 1u64 << i,
            }])])
        });
        for (k, q) in file.quads.iter().enumerate() {
            let proto = w.spawn();
            w.insert(
                proto,
                GtaWaterSurface {
                    outline: outline(q),
                    quad: *q,
                    specular_falloff: file.specular_falloff,
                },
            );
            // the world map's names as before; another file's carry its value
            w.insert(
                proto,
                Name(if i == 0 {
                    format!("gta_water_{k}")
                } else {
                    format!("gta_water_{value}_{k}")
                }),
            );
            let p = w.spawn();
            w.insert(
                p,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            w.insert(p, Transform::IDENTITY);
            if let Some(c) = &condition {
                w.insert(p, c.clone());
            }
            w.insert(p, Source(format!("{}#water_quad_{k}", file.path)));
        }
        let e = w.spawn();
        w.insert(
            e,
            GtaWater {
                path: file.path,
                calming: file.calming,
                waves: file.waves,
            },
        );
    }
    counts
}

/// Water data from `water.xml` bytes (`path` empty, no tuning).
pub fn parse_water(bytes: &[u8]) -> Result<WaterFile> {
    let root = crate::read::xml::parse(bytes, "water.xml")?;
    let num = |e: &crate::read::xml::Element, k: &str| -> Result<f32> {
        e.child_value(k)
            .ok_or_else(|| err(format!("a quad without {k}")))?
            .parse()
            .map_err(|_| err(format!("{k}: not a number")))
    };
    // a flag left out is false; any value but true or false is an error
    let flag = |e: &crate::read::xml::Element, k: &str| -> Result<bool> {
        match e.child_value(k) {
            None => Ok(false),
            Some(v) if v.eq_ignore_ascii_case("true") => Ok(true),
            Some(v) if v.eq_ignore_ascii_case("false") => Ok(false),
            Some(v) => Err(err(format!("{k}: {v:?} is neither true nor false"))),
        }
    };
    let rect = |e: &crate::read::xml::Element| -> Result<([f32; 2], [f32; 2])> {
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
            invisible: flag(q, "IsInvisible")?,
            limited_depth: flag(q, "HasLimitedDepth")?,
            corners: [num(q, "a1")?, num(q, "a2")?, num(q, "a3")?, num(q, "a4")?],
            no_stencil: flag(q, "NoStencil")?,
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
    Ok(WaterFile {
        path: String::new(),
        quads,
        calming,
        waves,
        specular_falloff: None,
    })
}

/// Water files for tests: a quad as water.xml writes it, a file of such quads.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::components::water::area;

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

    /// A type-0 quad at sea level as water.xml writes it.
    pub(crate) fn quad_xml(x: [i32; 4], invisible: bool, limited_depth: bool, no_stencil: bool) -> String {
        let [x0, x1, y0, y1] = x;
        format!(
            r#"<Item><minX value="{x0}" /><maxX value="{x1}" /><minY value="{y0}" /><maxY value="{y1}" />
<Type value="0" /><IsInvisible value="{invisible}" /><HasLimitedDepth value="{limited_depth}" /><z value="0" />
<a1 value="26" /><a2 value="26" /><a3 value="26" /><a4 value="26" /><NoStencil value="{no_stencil}" /></Item>"#
        )
    }

    pub(crate) fn file(path: &str, quads: &[String]) -> WaterFile {
        let mut f = parse_water(
            format!(
                "<WaterData><WaterQuads>{}</WaterQuads></WaterData>",
                quads.concat()
            )
            .as_bytes(),
        )
        .unwrap();
        f.path = path.into();
        f.specular_falloff = Some(1118.0);
        f
    }

    fn surfaces(w: &World) -> Vec<GtaWaterSurface> {
        w.iter::<Instance>()
            .filter_map(|(_, i)| w.get::<GtaWaterSurface>(i.prototype).cloned())
            .collect()
    }

    /// The reader places every quad whole, the rest of the file apart; one file has no state.
    #[test]
    fn the_reader_places_every_quad_as_stored() {
        let mut w = World::new();
        let counts = place_water(
            &mut w,
            vec![(
                "world",
                file("w.xml", &[quad_xml([0, 10, 0, 10], true, false, false)]),
            )],
        );
        assert_eq!(counts, vec![("w.xml".to_string(), [1, 0, 0])]);
        let s = surfaces(&w);
        assert_eq!(s.len(), 1);
        assert!(s[0].quad.invisible && (area(&s[0].outline) / 2.0 - 100.0).abs() < 1e-3);
        assert_eq!(w.iter::<StateAxis>().count(), 0);
        assert_eq!(w.iter::<Condition>().count(), 0);
        assert_eq!(w.iter::<GtaWater>().count(), 1);
    }

    /// A tuning value that is not a number, or a flag that is neither true nor false, is an error naming it.
    #[test]
    fn malformed_values_are_errors() {
        let tune = |v: &str| format!("<WaterTune><SpecularFalloff value=\"{v}\" /></WaterTune>");
        assert_eq!(
            specular_falloff(tune("1118").as_bytes(), "t.xml").unwrap(),
            Some(1118.0)
        );
        let e = specular_falloff(tune("1,118").as_bytes(), "t.xml")
            .unwrap_err()
            .to_string();
        assert!(e.contains("SpecularFalloff"), "{e}");
        assert_eq!(specular_falloff(b"<WaterTune />", "t.xml").unwrap(), None);
        let bad = quad_xml([0, 1, 0, 1], false, false, false)
            .replace(r#"IsInvisible value="false""#, r#"IsInvisible value="1""#);
        let e = parse_water(format!("<WaterData><WaterQuads>{bad}</WaterQuads></WaterData>").as_bytes())
            .unwrap_err()
            .to_string();
        assert!(e.contains("IsInvisible"), "{e}");
    }
}
