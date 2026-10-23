//! `lower-fromsoft-materials` (source materials as core PBR) and `drop-fromsoft-texture-transforms`.
use crate::materials::{LayeredMaterial, Material};
use crate::sekiro::components::lowered::UvTiling;
use crate::sekiro::source::LayerInputs;
use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{PbrMaterial, UvTransform};
use primase_core::{Component, DrawOrder, Entity, MaterialUsage, Mesh, VertexColor, World};
use std::collections::BTreeMap;

/// `lower-fromsoft-materials{layers=preserve|preview}`: source materials as core PBR from their bindings and the
/// texture files the reader kept (decoded here, named `<stem>` and `<stem>@normal`): base and normal maps in the
/// source normal convention (`normalize-normals` converts), gloss to roughness, traced metallic constants and maps
/// (unevidenced metallic is dielectric 0, counted), the preview fallback colour without a base texture (counted),
/// texture transforms relative to the tiling a geometry pass applied. Baked materials keep their bindings. What a
/// lowered material's source class states about its surfaces is set with it ([`super::pbr::class_usage`]): water
/// and leaves as their `core.material_usage`, decals as their `core.draw_order`. A material is two-sided exactly where
/// the face sets drawing it do not cull back faces (`fromsoft.face_cull`), split when they differ
/// ([`super::pbr::apply_face_cull`]).
///
/// A layered material no pass baked needs the program's choice: `layers=preserve` leaves it as source data
/// (unlowered, its layer inputs kept, counted); `layers=preview` lowers it from its main-layer preview, a counted
/// approximation, and drops the layer inputs (shader inputs, not vertex colours). Without the option such a material
/// is an error. Materials of a skipped source class, and source materials no primitive draws, are not lowered, counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LowerFromSoftMaterials {
    layers: Option<super::pbr::Layers>,
}

impl LowerFromSoftMaterials {
    /// `layers`: what becomes of a layered material no pass baked; `None`: such a material is an error.
    pub fn new(layers: Option<super::pbr::Layers>) -> Self {
        LowerFromSoftMaterials { layers }
    }
    pub fn layers(&self) -> Option<super::pbr::Layers> {
        self.layers
    }
}

/// `lower-fromsoft-materials`' declaration ([`primase_core::pass::PassRegistry::add`]).
pub const LOWER_FROMSOFT_MATERIALS: PassSpec = PassSpec {
    name: "lower-fromsoft-materials",
    options: &[OptionSpec::optional_choice::<super::pbr::Layers>(
        "layers",
        "a layered material left as source data for a later bake, or lowered from its main layer's preview textures",
    )],
    make: |g| Ok(Box::new(LowerFromSoftMaterials::new(g.optional_choice("layers")))),
};

impl Pass for LowerFromSoftMaterials {
    fn name(&self) -> &'static str {
        "lower-fromsoft-materials"
    }
    fn options(&self) -> Options {
        match self.layers {
            None => Options::new(),
            Some(l) => Options::from([("layers".to_owned(), l.text().to_owned())]),
        }
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        // a material drawn both culled and not is split: its copy has the source material's components
        &[
            "pbr.material",
            "core.texture",
            "core.name",
            MaterialUsage::NAME,
            DrawOrder::NAME,
            Material::NAME,
            LayeredMaterial::NAME,
            UvTiling::NAME,
            VertexColor::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[LayerInputs::NAME]
    }
    /// A lowered source material is named after its preview base map ([`crate::preview`]).
    fn modifies(&self) -> &[&'static str] {
        &["pbr.material", "core.texture", "core.name", Mesh::NAME]
    }
    fn approximates(&self) -> &'static str {
        "unevidenced metallic as dielectric 0; per-light gloss correction omitted; with layers=preview, unbaked layered materials from their main-layer preview; preview fallback colour"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        use crate::sekiro::source::TexturePayloads;
        // lent read-only (a world without the reader's payloads has no texture file to decode): borrowed, not written
        let lowered = world.resource_scope::<TexturePayloads, _>(|world, payloads| {
            super::pbr::lower_drawn(world, payloads.map_or(&BTreeMap::new(), |p| &p.0), self.layers)
        });
        let (lowered, previewed) = lowered.map_err(|e| e.to_string())?;
        // the inputs of the layers a preview stands for: not vertex colours, so they go with the layers; the inputs of
        // materials left as source data stay
        let dropped = LayerInputs::consume(world, |_, _, p| {
            p.material.is_some_and(|m| previewed.contains(&m))
        });
        if dropped > 0 {
            log.approximate(
                "layer input streams dropped with layered materials lowered from previews",
                dropped,
            );
        }
        log.absorb(lowered);
        super::pbr::apply_face_cull(world, log).map_err(|e| e.to_string())
    }
}

/// `drop-fromsoft-texture-transforms`: every texture binding of a lowered FromSoft material (a `pbr.material` on an
/// entity with `fromsoft.material`) loses its transform, so each texture is sampled at the coordinates
/// `lower-fromsoft-geometry{uv-tiling=apply}` scaled by the albedo binding's FLVER tiling, as the earlier exporter
/// sampled them (only the main albedo's tiling scales the mesh UVs; a normal or detail map's own scale is not applied).
/// Sekiro's normal and roughness maps carry their binding's scale relative to the albedo's
/// (`lower-fromsoft-materials`), and a material that mixes them cannot have them baked into one coordinate set, so a
/// target without texture transforms (ReSkate's dialect) needs them gone. Each non-identity transform left out is a
/// counted approximation, by slot; other sources' materials are not this pass's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropFromSoftTextureTransforms;

impl DropFromSoftTextureTransforms {
    /// The pass; it has no settings.
    pub fn new() -> Self {
        DropFromSoftTextureTransforms
    }
}

/// `drop-fromsoft-texture-transforms`' declaration ([`primase_core::pass::PassRegistry::add`]): no options, which transforms it drops is the target's need.
pub const DROP_FROMSOFT_TEXTURE_TRANSFORMS: PassSpec = PassSpec {
    name: "drop-fromsoft-texture-transforms",
    options: &[],
    make: |_| Ok(Box::new(DropFromSoftTextureTransforms::new())),
};

impl Pass for DropFromSoftTextureTransforms {
    fn name(&self) -> &'static str {
        "drop-fromsoft-texture-transforms"
    }
    fn requires(&self) -> &[&'static str] {
        &[PbrMaterial::NAME, Material::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[PbrMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "FromSoft textures sampled at the albedo's tiling, their own transform left out"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<Entity> = world
            .iter::<PbrMaterial>()
            .filter(|(e, _)| world.has::<Material>(*e))
            .map(|(e, _)| e)
            .collect();
        for e in materials {
            let Some(m) = world.get_mut::<PbrMaterial>(e) else {
                continue;
            };
            for (slot, r) in m.slots_mut() {
                if let Some(t) = r.transform.take()
                    && t != UvTransform::IDENTITY
                {
                    log.approximate(
                        &format!(
                            "{} textures sampled at the albedo's tiling, their own transform left out",
                            slot.name()
                        ),
                        1,
                    );
                }
            }
        }
        Ok(())
    }
}
