//! `gta-vertex-colours`: GTA's vertex colours given glTF's meaning. Vertex colour 0 kept where it fades the surface (as
//! white with GTA's alpha), vertex colour 1 kept where it weighs a layered material, both dropped elsewhere (glTF
//! multiplies COLOR_0 into the base colour; GTA's red and green are ambient occlusion and its blue a palette column,
//! none of them a colour).
use std::collections::BTreeSet;

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, LayeredMaterial, Semantic, Stream, World};

use crate::components::tint::GtaTinted;
use crate::passes::lower::{classes, for_each_mesh};
use crate::passes::shader_class::Class;

/// `gta-vertex-colours`: on a primitive whose shader fades by vertex alpha
/// ([`crate::passes::shader_class::ShaderClass::vertex_alpha`]) and is blended (alpha, decal, glass), with some vertex alpha below 255, COLOR_0
/// becomes (255, 255, 255, alpha); elsewhere it is dropped, except where `gta-tint` wrote the tint into it (kept,
/// its alpha only where it fades). COLOR_1 stays on primitives drawn with a layered material
/// (its layer weights) and is dropped elsewhere. Terrain primitives whose material is neither layered nor lowered are
/// left as they are (`gta-terrain-layers` has not run). Counted: primitives whose COLOR_0 red or green (GTA's per-vertex ambient
/// occlusion) is not all 255, which no PBR channel carries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GtaVertexColours;

impl Pass for GtaVertexColours {
    fn name(&self) -> &'static str {
        "gta-vertex-colours"
    }
    // no `requires`: it runs on every loaded part and leaves one without its input (a part holding only a water
    // surface, a cell whose shaders all draw nothing) as it is
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "gta.sources"]
    }
    fn approximates(&self) -> &'static str {
        "per-vertex ambient occlusion (vertex colour 0 red and green) not carried"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let classes = classes(w)?;
        let layered: BTreeSet<Entity> = w.iter::<LayeredMaterial>().map(|(e, _)| e).collect();
        let lowered: BTreeSet<Entity> = w.iter::<PbrMaterial>().map(|(e, _)| e).collect();
        let tinted: BTreeSet<Entity> = w.resource::<GtaTinted>().map_or(BTreeSet::new(), |t| t.0.clone());
        for_each_mesh(w, |mesh| {
            for mut p in mesh.primitives_mut() {
                let Some(m) = p.material else { continue };
                let Some(c) = classes.get(&m) else { continue };
                if c.class == Class::Terrain && !layered.contains(&m) && !lowered.contains(&m) {
                    log.count("terrain primitives left (gta-terrain-layers has not run)", 1);
                    continue;
                }
                match p.streams_mut().remove(&Semantic::Color(0)) {
                    Some(Stream::U8x4(v)) => {
                        if v.iter().any(|c| c[0] != 255 || c[1] != 255) {
                            log.approximate("primitives with per-vertex ambient occlusion not carried", 1);
                        }
                        let fades = c.vertex_alpha
                            && matches!(c.class, Class::Alpha | Class::Decal | Class::Glass)
                            && v.iter().any(|c| c[3] < 255);
                        if fades {
                            let white = v.iter().map(|c| [255, 255, 255, c[3]]).collect();
                            p.streams_mut().insert(Semantic::Color(0), Stream::U8x4(white));
                            log.count("COLOR_0 kept as vertex alpha", 1);
                        } else {
                            log.count("COLOR_0 dropped", 1);
                        }
                    }
                    // gta-tint's colour: kept, with the alpha only where it fades the surface
                    Some(Stream::U16x4(mut v)) if tinted.contains(&m) => {
                        let fades = c.vertex_alpha
                            && matches!(c.class, Class::Alpha | Class::Decal | Class::Glass)
                            && v.iter().any(|c| c[3] < 65535);
                        if !fades {
                            for c in &mut v {
                                c[3] = 65535;
                            }
                        }
                        p.streams_mut().insert(Semantic::Color(0), Stream::U16x4(v));
                        log.count("COLOR_0 kept as the tint", 1);
                    }
                    Some(_) => return Err("COLOR_0 not a u8 x 4 stream".into()),
                    None => {}
                }
                if !layered.contains(&m) && p.streams_mut().remove(&Semantic::Color(1)).is_some() {
                    log.count("COLOR_1 dropped", 1);
                }
            }
            Ok(None)
        })
    }
}
