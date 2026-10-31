//! `lower-trackmania-materials`: every `trackmania.material` whose shader has a glTF meaning gets a `pbr.material` on
//! its own entity, by its shader (its template material's file name):
//!
//! - `TDiff*` and `PDiff_Spec_Nrm*` (the bulk of the stadium: roads, platforms, stands, signs): the `Diffuse` slot as
//!   the base colour, the `Normal` slot as the normal map.
//! - the grass (`PDiff PDiff PA * Grass*`) and generated soil (`SoilGen21`): the `Blend1` slot as the base colour. The
//!   other blend layers, the blend mask and the mowing stripes are not lowered.
//! - `SoilFixToGen21` (dirt meeting road): `SoilFix` as the base colour; `SoilFix` (dirt road): `Soil`.
//! - `TSelfI TDiffA PX2` (screens and adverts): `Advert` as the base colour, unlit.
//! - `Sea`: core water ([`MaterialUsage::Water`]), a still tint: its waves, reflection and refraction are not lowered.
//!
//! Additive glows (`TAdd*`, `TSelfI Add`: core has no additive blending), the fake shadows under objects
//! (`ShadowSkirt`) and the grass's blade fins (`VDep Fence`: half-metre upright strips in a lattice over every grass
//! square, which the game fades by the angle they are seen at; as plain cut-outs they draw as pale lines over the
//! grass from above) get none, nor does a shader not named here: each is counted with its shader, and the primitives drawn
//! with it lose their material (counted), which a program leaves out (`omit-materialless-primitives`) or replaces. In every lowered material the specular map, the cube reflection, the occlusion and the
//! lighting atlas are not lowered (counted): the surface is dielectric (metallic 0) at roughness 0.6, the pass's choice.
//!
//! Base colour textures are taken as sRGB colour and normal maps as DXT5nm (X in alpha, Y in green, Z rebuilt) with
//! green down (Direct3D's), which is inferred from the channels (red 0 and blue near 255 on every stadium normal map
//! looked at) and the game's API, not checked against a lit reference.
//!
//! # Projected slots
//!
//! A projected shader (one whose first token starts with `P`, and `SoilGen*` and `SoilFixToGen*`) samples a slot with a
//! stored transform at coordinates projected from the position, not at the mesh's texture coordinates: the grass's
//! `Blend1` repeats every 5 m (scale 0.2), the dirt's every 14.8 m. The projection is taken as the world position's X and
//! Z (inferred: the grass's own texture coordinates are its block-local position over the square, which a stored scale
//! of 0.2 cannot tile seamlessly), and is lowered as texture coordinate set [`PROJECTED_SET`] holding each vertex's X and
//! Z in its prototype's space, in the game's frame; the slot samples it with its transform, made the world's
//! (top-left) one by `in_world`.
//!
//! The prototype is shared by blocks in four directions and at every square, so the world projection is made exact up to
//! a period: the scale is rounded to a whole number of repeats per square (the grass's 6.4 to 6, a 5.33 m repeat;
//! counted), which makes a block's square offset a whole number of repeats, and a block turned `d` steps samples a copy
//! of the material turned with it, given to the block as variant `d` of its prototype's `core.material_variants` (and of
//! each of its `core.lods` forms).
use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, Shading, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, Component, Entity, Instance, Lods, MaterialUsage, MaterialVariants, Mesh, Name, NormalBasis,
    NormalConvention, NormalGreen, NormalX, NormalZ, Resource, Semantic, Stream, TexRole, Texture, World,
};

use crate::FRAME;
use crate::components::{TrackmaniaBlock, TrackmaniaMap, TrackmaniaMaterial, TrackmaniaTextureSlot};

/// The texture coordinate set the projected slots sample: the game stores sets 0 to 2.
pub const PROJECTED_SET: u8 = 3;

/// Roughness of every lowered material (the game's specular model is not lowered).
pub const ROUGHNESS: f32 = 0.6;

/// The stadium's normal maps' encoding (see the module documentation).
pub const NORMALS: NormalConvention = NormalConvention {
    green: NormalGreen::Down,
    z: NormalZ::Rebuild,
    x: NormalX::Alpha,
    basis: NormalBasis::TangentBitangent,
};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerTrackmaniaMaterials;

/// `lower-trackmania-materials`' declaration ([`primase_core::pass::PassRegistry::add`]): no options.
pub const LOWER_TRACKMANIA_MATERIALS: PassSpec = PassSpec {
    name: "lower-trackmania-materials",
    options: &[],
    make: |_| Ok(Box::new(LowerTrackmaniaMaterials)),
};

/// What a shader lowers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    /// Lit, the base colour from the slot named, with the `Normal` slot when `normal`.
    Lit { base: &'static str, normal: bool },
    /// Unlit.
    Screen { base: &'static str },
    /// Core water.
    Water,
}

/// The look of `shader` (a template material's file name), `None` for one not lowered.
fn look(shader: &str) -> Option<Look> {
    let first = shader.split(' ').next().unwrap_or(shader);
    Some(match shader {
        _ if first.starts_with("TDiff") || first.starts_with("PDiff_") => Look::Lit {
            base: "Diffuse",
            normal: true,
        },
        _ if first == "PDiff" && shader.contains("Grass") => Look::Lit {
            base: "Blend1",
            normal: false,
        },
        _ if shader.starts_with("SoilGen") => Look::Lit {
            base: "Blend1",
            normal: false,
        },
        _ if shader.starts_with("SoilFixToGen") => Look::Lit {
            base: "SoilFix",
            normal: false,
        },
        "SoilFix" => Look::Lit {
            base: "Soil",
            normal: false,
        },
        "TSelfI TDiffA PX2" => Look::Screen { base: "Advert" },
        "Sea" => Look::Water,
        _ => return None,
    })
}

/// Whether `shader` samples its transformed slots at projected coordinates.
fn projects(shader: &str) -> bool {
    shader.starts_with('P') || shader.starts_with("SoilGen") || shader.starts_with("SoilFixToGen")
}

/// Whether `slot` states a texture coordinate transform.
fn transformed(slot: &TrackmaniaTextureSlot) -> bool {
    slot.scale != [1.0, 1.0] || slot.translation != [0.0, 0.0] || slot.rotation != 0.0
}

/// A slot's transform `A` (`uv' = t + R(r) s uv`, glTF's order, in the game's bottom-left convention) as the world's,
/// whose texture coordinates are the game's with v as 1 - v (`F`), so that the world samples at `F(A(uv))`. A projected
/// set holds the game's (x, z) as they are: its transform is `F A`, scale (su, -sv), rotation -r and offset
/// (tu, 1 - tv). A mesh's set is already `F` of the game's: its transform is `F A F`, scale s, rotation -r and offset
/// (tu - sv sin r, 1 - tv - sv cos r).
fn in_world(scale: [f32; 2], offset: [f32; 2], rotation: f32, projected: bool) -> UvTransform {
    if projected {
        UvTransform {
            offset: [offset[0], 1.0 - offset[1]],
            scale: [scale[0], -scale[1]],
            rotation: -rotation,
        }
    } else {
        let (sin, cos) = rotation.sin_cos();
        UvTransform {
            offset: [offset[0] - scale[1] * sin, 1.0 - offset[1] - scale[1] * cos],
            scale,
            rotation: -rotation,
        }
    }
}

/// A slot sampled as lowered: its texture, set and transform.
#[derive(Clone, Copy, Debug)]
struct Sampled {
    texture: Entity,
    projected: bool,
    transform: UvTransform,
    /// The projected scale was rounded to a whole number of repeats per square.
    rounded: bool,
}

impl Sampled {
    /// `slot` of a material drawn with `shader`, `None` when its image is not read. A projected slot's scale is rounded
    /// to a whole number of repeats per `square` metres.
    fn new(slot: &TrackmaniaTextureSlot, shader: &str, square: f32) -> Option<Sampled> {
        let texture = slot.texture?;
        let projected = projects(shader) && transformed(slot);
        let mut scale = slot.scale;
        let mut rounded = false;
        if projected {
            for s in &mut scale {
                let repeats = (s.abs() * square).round().max(1.0);
                let whole = s.signum() * repeats / square;
                rounded |= whole != *s;
                *s = whole;
            }
        }
        Some(Sampled {
            texture,
            projected,
            transform: in_world(scale, slot.translation, slot.rotation.to_radians(), projected),
            rounded,
        })
    }

    /// The texture reference sampling it on a block turned `turns` steps.
    fn reference(&self, turns: u8) -> TextureRef {
        let mut transform = self.transform;
        if self.projected && turns != 0 {
            // a turn takes the projected (x, z) to R(90 deg * turns) (x, z), which the scale then follows: through a
            // scale whose two signs agree the turn passes unchanged, through one mirroring a single axis reversed
            let quarter = f32::from(turns) * std::f32::consts::FRAC_PI_2;
            let same = transform.scale[0].signum() == transform.scale[1].signum();
            transform.rotation += if same { quarter } else { -quarter };
        }
        TextureRef {
            texcoord: if self.projected { PROJECTED_SET } else { 0 },
            transform: (transform != UvTransform::IDENTITY).then_some(transform),
            ..TextureRef::new(self.texture)
        }
    }
}

/// A material's lowering: the stored entity, its PBR form, and its base colour's projection when it has one.
struct Lowered {
    pbr: PbrMaterial,
    base: Option<Sampled>,
    normal: Option<Sampled>,
}

impl Lowered {
    /// The material for a block turned `turns` steps.
    fn turned(&self, turns: u8) -> PbrMaterial {
        let mut pbr = self.pbr.clone();
        pbr.base_color_texture = self.base.map(|s| s.reference(turns));
        pbr.normal_texture = self.normal.map(|s| s.reference(turns));
        pbr
    }

    fn projected(&self) -> bool {
        self.base.is_some_and(|s| s.projected) || self.normal.is_some_and(|s| s.projected)
    }
}

impl Pass for LowerTrackmaniaMaterials {
    fn name(&self) -> &'static str {
        "lower-trackmania-materials"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn requires(&self) -> &[&'static str] {
        &[TrackmaniaMaterial::NAME, TrackmaniaMap::NAME]
    }
    /// The materials' `pbr.material` and water's `core.material_usage`; the turned copies (each with `core.name`) and
    /// the prototypes' and forms' `core.material_variants`.
    fn provides(&self) -> &[&'static str] {
        &[
            PbrMaterial::NAME,
            MaterialUsage::NAME,
            MaterialVariants::NAME,
            Name::NAME,
        ]
    }
    /// The projected set added to the meshes; the colour space and role of the textures sampled; a turned block's
    /// variant.
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, Texture::NAME, Instance::NAME, MaterialVariants::NAME]
    }
    fn approximates(&self) -> &'static str {
        "specular, reflection, occlusion and lighting slots not lowered (dielectric at roughness 0.6); a projected \
         texture's repeat rounded to a whole number per square; the grass's and soil's blend layers, mask and stripes \
         not lowered; water a still tint"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let square = w.resource::<TrackmaniaMap>().ok_or("no trackmania.map")?.square[0];
        let materials: Vec<(Entity, TrackmaniaMaterial)> = w
            .iter::<TrackmaniaMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        let mut lowered: BTreeMap<Entity, Lowered> = BTreeMap::new();
        let mut unlowered: BTreeMap<Entity, String> = BTreeMap::new();
        for (e, m) in materials {
            let shader = m.shader_name().unwrap_or("").to_owned();
            let Some(look) = look(&shader) else {
                log.count(&format!("materials not lowered (shader {shader})"), 1);
                unlowered.insert(e, shader);
                continue;
            };
            log.count(&format!("materials lowered (shader {shader})"), 1);
            let slot = |name: &str| m.slot(name).and_then(|s| Sampled::new(s, &shader, square));
            let base_slot = match look {
                Look::Lit { base, .. } | Look::Screen { base } => Some(base),
                Look::Water => None,
            };
            let base = base_slot.and_then(slot);
            if let Some(name) = base_slot
                && base.is_none()
            {
                log.approximate_on(
                    e,
                    &format!("materials whose {name} image is not read: drawn white"),
                    1,
                );
            }
            let normal = match look {
                Look::Lit { normal: true, .. } => slot("Normal"),
                _ => None,
            };
            if base.is_some_and(|s| s.rounded) {
                log.approximate_on(
                    e,
                    "projected base colours' repeat rounded to a whole number per square",
                    1,
                );
            }
            for s in &m.slots {
                let used = base_slot == Some(s.slot.as_str()) || (normal.is_some() && s.slot == "Normal");
                if !used && s.texture.is_some() {
                    log.omit(&format!("{} slots not lowered", s.slot), 1);
                }
            }
            let pbr = match look {
                Look::Lit { .. } => PbrMaterial {
                    metallic: 0.0,
                    roughness: ROUGHNESS,
                    ..PbrMaterial::default()
                },
                Look::Screen { .. } => PbrMaterial {
                    metallic: 0.0,
                    roughness: ROUGHNESS,
                    shading: Shading::Unlit,
                    ..PbrMaterial::default()
                },
                Look::Water => {
                    w.insert(e, MaterialUsage::Water);
                    PbrMaterial {
                        base_color: [0.1, 0.25, 0.3, 1.0],
                        metallic: 0.0,
                        roughness: 0.05,
                        ..PbrMaterial::default()
                    }
                }
            };
            for (s, role) in [(base, TexRole::Color), (normal, TexRole::Normal)] {
                let Some(s) = s else { continue };
                if let Some(t) = w.get_mut::<Texture>(s.texture) {
                    t.role = role;
                    match role {
                        TexRole::Normal => {
                            t.color_space = ColorSpace::Linear;
                            t.normal = Some(NORMALS);
                        }
                        _ => t.color_space = ColorSpace::Srgb,
                    }
                }
            }
            let l = Lowered { pbr, base, normal };
            w.insert(e, l.turned(0));
            lowered.insert(e, l);
        }
        detach(w, &unlowered, log);
        project(w, &lowered, log);
        turn(w, &lowered, log);
        Ok(())
    }
}

/// Take the material off every primitive drawn with one of `unlowered` (material entity to its shader): it has no core
/// meaning, and a program leaves such primitives out (`omit-materialless-primitives`) or gives them one of its own.
fn detach(w: &mut World, unlowered: &BTreeMap<Entity, String>, log: &mut PassLog) {
    let meshes: Vec<Entity> = w
        .iter::<Mesh>()
        .filter(|(_, m)| {
            m.primitives()
                .iter()
                .any(|p| p.material.is_some_and(|m| unlowered.contains_key(&m)))
        })
        .map(|(e, _)| e)
        .collect();
    for e in meshes {
        let Some(mesh) = w.get_mut::<Mesh>(e) else {
            continue;
        };
        for mut p in mesh.primitives_mut() {
            let f = p.fields();
            let Some(shader) = f.material.and_then(|m| unlowered.get(&m)) else {
                continue;
            };
            log.count(
                &format!("primitives drawn with no PBR form, their material taken off (shader {shader})"),
                1,
            );
            *f.material = None;
        }
    }
}

/// Add the projected set to every primitive drawn with a projected material: each vertex's X and Z in the game's frame.
fn project(w: &mut World, lowered: &BTreeMap<Entity, Lowered>, log: &mut PassLog) {
    let meshes: Vec<Entity> = w
        .iter::<Mesh>()
        .filter(|(_, m)| {
            m.primitives().iter().any(|p| {
                p.material
                    .and_then(|m| lowered.get(&m))
                    .is_some_and(Lowered::projected)
            })
        })
        .map(|(e, _)| e)
        .collect();
    for e in meshes {
        let Some(mesh) = w.get_mut::<Mesh>(e) else {
            continue;
        };
        for mut p in mesh.primitives_mut() {
            let f = p.fields();
            if !f
                .material
                .and_then(|m| lowered.get(&m))
                .is_some_and(Lowered::projected)
            {
                continue;
            }
            let Some(Stream::F32x3(positions)) = f.streams.get(&Semantic::Position) else {
                continue;
            };
            let projected: Vec<[f32; 2]> = positions
                .iter()
                .map(|&p| {
                    let [x, _, z] = FRAME.point_to_source(p);
                    [x, z]
                })
                .collect();
            if f.streams
                .insert(Semantic::TexCoord(PROJECTED_SET), Stream::F32x2(projected))
                .is_some()
            {
                log.count("primitives whose stored set 3 the projected set replaced", 1);
            }
            log.count("primitives given the projected set", 1);
        }
    }
}

/// Give each block turned `d` steps whose prototype draws a projected material variant `d`, the material turned with it.
fn turn(w: &mut World, lowered: &BTreeMap<Entity, Lowered>, log: &mut PassLog) {
    let blocks: Vec<(Entity, Entity, u8)> = w
        .iter::<TrackmaniaBlock>()
        .filter(|(_, b)| b.direction != 0)
        .filter_map(|(e, b)| Some((e, w.get::<Instance>(e)?.prototype, b.direction)))
        .collect();
    let mut copies: BTreeMap<(Entity, u8), Entity> = BTreeMap::new();
    let mut tabled: BTreeMap<(Entity, u8), bool> = BTreeMap::new();
    for (e, prototype, turns) in blocks {
        let has = match tabled.get(&(prototype, turns)) {
            Some(&has) => has,
            None => {
                let mut meshes = vec![prototype];
                if let Some(lods) = w.get::<Lods>(prototype) {
                    meshes.extend(lods.forms.iter().map(|f| f.form));
                }
                let mut has = false;
                for m in meshes {
                    has |= variant(w, m, turns, lowered, &mut copies, log);
                }
                tabled.insert((prototype, turns), has);
                has
            }
        };
        if has && let Some(i) = w.get_mut::<Instance>(e) {
            i.variant = u32::from(turns);
            log.count("turned blocks drawn with turned projected materials", 1);
        }
    }
}

/// Variant `turns` of the mesh on `e`: each primitive drawn with a projected material takes its copy turned `turns`
/// steps. Whether it has one.
fn variant(
    w: &mut World,
    e: Entity,
    turns: u8,
    lowered: &BTreeMap<Entity, Lowered>,
    copies: &mut BTreeMap<(Entity, u8), Entity>,
    log: &mut PassLog,
) -> bool {
    let Some(materials) = w
        .get::<Mesh>(e)
        .map(|m| m.primitives().iter().map(|p| p.material).collect::<Vec<_>>())
    else {
        return false;
    };
    let mut table = w.get::<MaterialVariants>(e).cloned().unwrap_or_default();
    table.0.resize(materials.len(), BTreeMap::new());
    let mut has = false;
    for (i, m) in materials.iter().enumerate() {
        let Some(m) = *m else { continue };
        let Some(l) = lowered.get(&m).filter(|l| l.projected()) else {
            continue;
        };
        let copy = *copies.entry((m, turns)).or_insert_with(|| {
            let name = w.get::<Name>(m).map_or("", |n| n.0.as_str()).to_owned();
            let c = w.spawn();
            w.insert(c, l.turned(turns));
            w.insert(c, Name(format!("{name} (turned {turns})")));
            log.count("turned copies of projected materials", 1);
            c
        });
        if let Some(slot) = table.0.get_mut(i) {
            slot.insert(u32::from(turns), copy);
            has = true;
        }
    }
    if has {
        w.insert(e, table);
    }
    has
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::in_world;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
    }

    /// Hand-worked: the stadium's projected `PDiff_Spec_Nrm` slots store scale (1/16, -1/16) and offset (0, 1), the
    /// game sampling (x, z) at (x / 16, 1 - z / 16); the world, v as 1 - v, at (x / 16, z / 16).
    #[test]
    fn projected_flip_cancels() {
        let t = in_world([0.0625, -0.0625], [0.0, 1.0], 0.0, true);
        for p in [[0.0, 0.0], [16.0, 32.0], [-5.0, 7.5]] {
            assert!(close(t.apply(p), [p[0] / 16.0, p[1] / 16.0]), "{p:?}");
        }
    }

    /// Hand-worked: scale (2, 3), offset (0.5, 0.25) and a quarter turn take the game's (u, v) to
    /// (0.5 - 3v, 0.25 + 2u); a world coordinate (a, b) is the game's (a, 1 - b), sampled at (-2.5 + 3b, 0.25 + 2a),
    /// which the world reads at (-2.5 + 3b, 0.75 - 2a).
    #[test]
    fn mesh_transform_conjugated() {
        let t = in_world([2.0, 3.0], [0.5, 0.25], std::f32::consts::FRAC_PI_2, false);
        for [a, b] in [[0.0, 0.0], [1.0, 0.0], [0.25, 0.75]] {
            assert!(
                close(t.apply([a, b]), [-2.5 + 3.0 * b, 0.75 - 2.0 * a]),
                "{a} {b}"
            );
        }
        assert_eq!(
            in_world([1.0, 1.0], [0.0, 0.0], 0.0, false),
            primase_core::pbr::UvTransform::IDENTITY
        );
    }
}
