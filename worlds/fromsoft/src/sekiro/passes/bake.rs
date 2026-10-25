//! `bake-fromsoft-layers`: the layered materials of the placed prototypes baked per weight level.
use super::settings::{PixelCap, Seconds};
use crate::materials::{self, Material};
use crate::sekiro::source::{LayerInputs, PrototypeSource};
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::shader::ShaderBinding;
use primase_core::{Component, Entity, Instance, Lods, Mesh, World};
use std::collections::{BTreeMap, BTreeSet};

/// `bake-fromsoft-layers{max-size=N,sample-time=S}`: bake the static layered materials of the placed lowered
/// prototypes (a `fromsoft.material` with a layered definition, drawn by primitives carrying `fromsoft.layer_inputs`)
/// into core PBR from each material's `core.shader_binding` (`resolve-fromsoft-shaders` first: its program, constants and
/// textures) and the texture files the reader kept. Each material group's triangles are
/// reduced to their mean inputs, clustered into at most three constant levels, and each level's shader evaluated
/// into albedo, normal, gloss (ORM) and HDR emission images at the group's reference size capped at `max-size`, sampled
/// from base mips with bilinear wrapping in a static frame (unit pre-exposure, fixed camera, the global time at
/// `sample-time` seconds, which scrolls some layers); parts are split by level through `World::rebuild_primitives`.
/// Both options are required: the cap is the program's quality choice (also the size of a group without a colour
/// image), the time its choice of the animated layers' frame.
///
/// The layer inputs of the primitives it baked are consumed (their `Color(n)` streams and entries); unplaced
/// prototypes' stay. A level-of-detail form or a placed mesh without a source prototype that carries layer inputs is
/// refused before anything changes: choose a level of detail first (`select-lod`). The source resources are kept,
/// also when it fails.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BakeFromSoftLayers {
    /// The baked images' largest edge.
    max_size: PixelCap,
    /// The seconds of global time the programs that read it are sampled at.
    sample_time: Seconds,
}

impl BakeFromSoftLayers {
    pub fn new(max_size: PixelCap, sample_time: Seconds) -> Self {
        BakeFromSoftLayers {
            max_size,
            sample_time,
        }
    }
    pub fn max_size(&self) -> PixelCap {
        self.max_size
    }
    pub fn sample_time(&self) -> Seconds {
        self.sample_time
    }

    /// From its declared options, through [`BakeFromSoftLayers::new`].
    fn from_given(g: &Given) -> Result<Self, String> {
        Ok(BakeFromSoftLayers::new(
            PixelCap::parse("max-size", g.required("max-size"))?,
            Seconds::parse("sample-time", g.required("sample-time"))?,
        ))
    }
}

/// `bake-fromsoft-layers`' declaration ([`primase_core::pass::PassRegistry::add`]): the baked images' largest edge
/// and the frame of the animated layers are the program's.
pub const BAKE_FROMSOFT_LAYERS: PassSpec = PassSpec {
    name: "bake-fromsoft-layers",
    options: &[
        OptionSpec::required(
            "max-size",
            "512",
            "pixels, 1 to 4096: the baked images' largest edge",
        ),
        OptionSpec::required(
            "sample-time",
            "0",
            "seconds: the frame the animated layers are baked at",
        ),
    ],
    make: |g| Ok(Box::new(BakeFromSoftLayers::from_given(g)?)),
};

impl Pass for BakeFromSoftLayers {
    fn name(&self) -> &'static str {
        "bake-fromsoft-layers"
    }
    fn options(&self) -> Options {
        Options::from([
            ("max-size".to_owned(), self.max_size.to_string()),
            ("sample-time".to_owned(), self.sample_time.to_string()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[Material::NAME, LayerInputs::NAME, ShaderBinding::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            "pbr.material",
            "core.texture",
            "core.name",
            Material::NAME,
            materials::LayeredMaterial::NAME,
        ]
    }
    fn removes(&self) -> &[&'static str] {
        &[LayerInputs::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn approximates(&self) -> &'static str {
        "layer inputs as constant per-triangle levels; base-mip bilinear-wrap sampling; static frame at the sample time"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        use crate::sekiro::source::TexturePayloads;
        // the meshes it bakes: placed source prototypes'; inputs elsewhere are refused (a form) or left (unplaced)
        let placed: BTreeSet<Entity> = world.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let forms: BTreeSet<Entity> = world
            .iter::<Lods>()
            .flat_map(|(_, l)| l.forms.iter().map(|f| f.form))
            .collect();
        let mut unplaced = 0;
        for (e, _) in world.iter::<LayerInputs>() {
            if placed.contains(&e) && world.has::<PrototypeSource>(e) {
                continue;
            }
            if forms.contains(&e) || placed.contains(&e) {
                return Err(format!(
                    "{e:?} carries layer inputs this bake does not bake (a level-of-detail form, or a placed mesh \
                     without fromsoft.prototype): choose a level of detail first (select-lod)"
                ));
            }
            unplaced += 1;
        }
        if unplaced > 0 {
            log.count("unplaced meshes whose layer inputs stay", unplaced);
        }
        // What the bake approximates, recorded once on each material it makes (DESIGN-core-lowering.md 2.8): the pass's
        // own declaration, made specific to this run's size.
        let baked_text = format!(
            "layer materials baked per weight level at up to {} px: layer inputs as constant per-triangle levels; \
             base-mip bilinear-wrap sampling; static frame at the sample time",
            self.max_size.get()
        );
        // lent read-only while the world changes, then put back as they were: borrowed, not written
        let none = BTreeMap::new();
        let result = world.resource_scope::<TexturePayloads, _>(|world, payloads| {
            let bake = super::layers::AreaBake::with_sources(world, payloads.map_or(&none, |p| &p.0))?;
            let prototypes: BTreeMap<String, Entity> = world
                .iter::<PrototypeSource>()
                .map(|(e, s)| (s.id.clone(), e))
                .collect();
            super::bake_assembly::assemble(
                &bake,
                &prototypes,
                self.max_size.get(),
                self.sample_time.get(),
                &baked_text,
                world,
            )
        });
        let (report, baked) = result.map_err(|e| e.to_string())?;
        // counted after the split: a part baked at k levels gives k primitives' streams
        log.count(
            "layer input streams consumed",
            LayerInputs::consume(world, |e, i, _| baked.contains(&(e, i))),
        );
        for (k, v) in [
            ("source materials baked", report.source_materials),
            ("bake groups", report.materials),
            ("bake levels", report.variants),
            ("bake levels with non-unit UV periods", report.nonunit_uv_variants),
            ("bake groups with gloss", report.gloss_materials),
            ("bake groups without gloss", report.missing_gloss_materials),
            ("bake groups with emission", report.emissive_materials),
            ("bake groups without emission", report.missing_emissive_materials),
        ] {
            log.count(k, v as u64);
        }
        // its counts, its approximations and the materials each was made for, as the assembly recorded them
        log.absorb(report.policies);
        Ok(())
    }
}
