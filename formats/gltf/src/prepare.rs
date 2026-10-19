//! Preparing a world for glTF where the format needs something the source does not state: `prepare-gltf-materials`
//! (a material for material entities without `pbr.material`) and `fill-missing-texcoords` (texture coordinates a
//! primitive lacks for a set its material samples). Each fills in an appearance the source does not give, so a
//! program chooses it, with the values it states; the writer itself invents neither.
use std::collections::BTreeSet;

use primase_core::layered::{Channel, LayeredMaterial, Source};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{Component, Entity, Mesh, Semantic, Stream, World};

/// A factor from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct UnitValue(f32);

impl UnitValue {
    /// `value`: an error unless it is from 0 to 1.
    pub fn new(value: f32) -> Result<UnitValue, String> {
        if (0. ..=1.).contains(&value) {
            Ok(UnitValue(value))
        } else {
            Err(format!("{value} is not a number from 0 to 1"))
        }
    }

    fn parse(key: &str, text: &str) -> Result<UnitValue, String> {
        let value = text
            .parse::<f32>()
            .map_err(|_| format!("{key}={text} is not a number"))?;
        UnitValue::new(value).map_err(|e| format!("{key}: {e}"))
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

/// `prepare-gltf-materials{base-color=R/G/B/A,metallic=M,roughness=R}` (all three required: the program states the
/// look): every material entity a primitive draws with that has neither `pbr.material` nor `core.layered` (nor is a
/// layer of one) gets a `pbr.material` of those factors, untextured, opaque and single-sided, counted as an
/// approximation. The writer writes a material entity without one as no material at all (the reader's default).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrepareGltfMaterials {
    /// Linear base colour and alpha.
    pub base_color: [UnitValue; 4],
    pub metallic: UnitValue,
    pub roughness: UnitValue,
}

impl PrepareGltfMaterials {
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let (mut base_color, mut metallic, mut roughness) = (None, None, None);
        for (k, v) in o {
            match k.as_str() {
                "base-color" => {
                    let parts = v
                        .split('/')
                        .map(|x| UnitValue::parse(k, x))
                        .collect::<Result<Vec<_>, _>>()?;
                    base_color = Some(
                        <[UnitValue; 4]>::try_from(parts)
                            .map_err(|_| format!("base-color={v} is not four values R/G/B/A"))?,
                    );
                }
                "metallic" => metallic = Some(UnitValue::parse(k, v)?),
                "roughness" => roughness = Some(UnitValue::parse(k, v)?),
                _ => return Err(format!("unknown option {k}")),
            }
        }
        let missing = |what: &str| format!("{what} is required: the program states the material it fills in");
        Ok(PrepareGltfMaterials {
            base_color: base_color.ok_or_else(|| missing("base-color"))?,
            metallic: metallic.ok_or_else(|| missing("metallic"))?,
            roughness: roughness.ok_or_else(|| missing("roughness"))?,
        })
    }
}

/// The material entities primitives draw with, their variants' included.
fn drawn_materials(w: &World) -> BTreeSet<Entity> {
    let mut out: BTreeSet<Entity> = w
        .iter::<Mesh>()
        .flat_map(|(_, m)| m.primitives().iter().filter_map(|p| p.material))
        .collect();
    for (_, v) in w.iter::<primase_core::MaterialVariants>() {
        out.extend(v.0.iter().flat_map(|m| m.values().copied()));
    }
    // a layered material's layers are drawn through it
    let layers: Vec<Entity> = out
        .iter()
        .filter_map(|e| w.get::<LayeredMaterial>(*e))
        .flat_map(|l| l.layers.iter().flatten().map(|layer| layer.material))
        .collect();
    out.extend(layers);
    out
}

impl Pass for PrepareGltfMaterials {
    fn name(&self) -> &'static str {
        "prepare-gltf-materials"
    }
    fn options(&self) -> Options {
        let [r, g, b, a] = self.base_color.map(UnitValue::get);
        Options::from([
            ("base-color".to_owned(), format!("{r}/{g}/{b}/{a}")),
            ("metallic".to_owned(), self.metallic.get().to_string()),
            ("roughness".to_owned(), self.roughness.get().to_string()),
        ])
    }
    fn provides(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "materials without PBR factors given the program's"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let bare: Vec<Entity> = drawn_materials(w)
            .into_iter()
            .filter(|&e| !w.has::<PbrMaterial>(e) && !w.has::<LayeredMaterial>(e))
            .collect();
        for e in bare {
            w.insert(
                e,
                PbrMaterial {
                    base_color: self.base_color.map(UnitValue::get),
                    metallic: self.metallic.get(),
                    roughness: self.roughness.get(),
                    ..PbrMaterial::default()
                },
            );
            log.approximate("materials without PBR factors given the program's", 1);
        }
        Ok(())
    }
}

/// The texture coordinate sets a material samples, as the writer binds them: each texture slot's set, a layered
/// material's layers' (their own slots, and the set each layer is sampled with) and its weight masks'.
pub fn sampled_texcoords(w: &World, material: Entity) -> BTreeSet<u8> {
    let mut sets = BTreeSet::new();
    let mut slot = |r: &Option<TextureRef>| {
        if let Some(r) = r {
            sets.insert(r.texcoord);
        }
    };
    if let Some(p) = w.get::<PbrMaterial>(material) {
        for r in [
            &p.base_color_texture,
            &p.metallic_roughness_texture,
            &p.occlusion_texture,
            &p.normal_texture,
            &p.emissive_texture,
        ] {
            slot(r);
        }
        if let Some(s) = &p.specular {
            slot(&s.texture);
            slot(&s.color_texture);
        }
    }
    if let Some(l) = w.get::<LayeredMaterial>(material) {
        for layer in l.layers.iter().flatten() {
            sets.extend(sampled_texcoords(w, layer.material));
            sets.insert(layer.texcoord.unwrap_or(0));
        }
        for channel in l.weights.channels.iter().flatten() {
            for source in Channel::sources(channel) {
                if let Source::Mask { texture, .. } = source {
                    sets.insert(texture.texcoord);
                }
            }
        }
    }
    sets
}

/// `fill-missing-texcoords`: a primitive drawn with a material that samples a texture coordinate set it lacks gets
/// that set as zeros (every vertex samples the texture's corner), counted per primitive as an approximation. glTF
/// requires the set and states no default; the writer refuses a primitive without it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FillMissingTexcoords;

impl Pass for FillMissingTexcoords {
    fn name(&self) -> &'static str {
        "fill-missing-texcoords"
    }
    fn requires(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn approximates(&self) -> &'static str {
        "texture coordinates a primitive lacks for a set its material samples, as zeros"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let meshes: Vec<Entity> = w.iter::<Mesh>().map(|(e, _)| e).collect();
        for e in meshes {
            let wanted: Vec<BTreeSet<u8>> = w
                .get::<Mesh>(e)
                .expect("listed")
                .primitives()
                .iter()
                .map(|p| p.material.map(|m| sampled_texcoords(w, m)).unwrap_or_default())
                .collect();
            let mesh = w.get_mut::<Mesh>(e).expect("listed");
            for (mut p, sets) in mesh.primitives_mut().zip(wanted) {
                let count = p.vertex_count();
                let mut filled = false;
                for set in sets {
                    if let std::collections::btree_map::Entry::Vacant(v) =
                        p.streams_mut().entry(Semantic::TexCoord(set))
                    {
                        v.insert(Stream::F32x2(vec![[0.; 2]; count]));
                        filled = true;
                    }
                }
                if filled {
                    log.approximate("primitives given zero texture coordinates", 1);
                }
            }
        }
        Ok(())
    }
}

/// Register this module's passes.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.register_requiring(
        "prepare-gltf-materials",
        |o| Ok(Box::new(PrepareGltfMaterials::from_options(o)?)),
        &[
            ("base-color", "0.8/0.8/0.8/1"),
            ("metallic", "0"),
            ("roughness", "0.85"),
        ],
    );
    reg.register("fill-missing-texcoords", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(FillMissingTexcoords)),
    });
}
