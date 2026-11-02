//! `release-fromsoft-sources`: the stored files a FromSoftware read keeps for its lowering, released once the lowering
//! that reads them has run.
use crate::components::models::SourceModel;
use crate::sekiro::source::{LowTexturePayloads, Payloads, TexturePayloads};
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Resource, World};

/// `release-fromsoft-sources`: every prototype's model as stored (`fromsoft.flver`) and the read's texture files
/// (`fromsoft.texture_payloads`, and the low-resolution twins `fromsoft.low_texture_payloads` where the read took
/// them) taken out of the world, counted with their bytes. The passes that read them declare them
/// (`lower-fromsoft-geometry` with `pose=stored` and `fromsoft-apply-rigid-skinning` the models;
/// `resolve-fromsoft-shaders`, `lower-fromsoft-materials` and `bake-fromsoft-layers` the texture files, the twins with
/// them), so one stated after it is refused for what it lacks rather than lowering without it; the render surface of
/// `lower-fromsoft-object-collision` refuses a prototype without its model. What a program releases is its choice: a
/// program that lowers again later (a state switch, a viewer's re-read) keeps them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReleaseFromSoftSources;

/// `release-fromsoft-sources`' declaration ([`primase_core::pass::PassRegistry::add`]): no options.
pub const RELEASE_FROMSOFT_SOURCES: PassSpec = PassSpec {
    name: "release-fromsoft-sources",
    options: &[],
    make: |_| Ok(Box::new(ReleaseFromSoftSources)),
};

/// The bytes of a read's files.
fn bytes(payloads: &Payloads) -> u64 {
    payloads.values().map(|b| b.len() as u64).sum()
}

impl Pass for ReleaseFromSoftSources {
    fn name(&self) -> &'static str {
        "release-fromsoft-sources"
    }
    /// A world that holds none of them (a read that kept no model or texture file, or one already released) has
    /// nothing to release: the step is skipped.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[SourceModel::NAME, TexturePayloads::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[SourceModel::NAME, TexturePayloads::NAME, LowTexturePayloads::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let models: Vec<Entity> = world.iter::<SourceModel>().map(|(e, _)| e).collect();
        let model_bytes: u64 = world.iter::<SourceModel>().map(|(_, m)| m.payload_bytes()).sum();
        for e in &models {
            world.remove::<SourceModel>(*e);
        }
        log.count("stored models released", models.len() as u64);
        log.count("bytes of the stored models' meshes released", model_bytes);
        if let Some(files) = world.remove_resource::<TexturePayloads>() {
            log.count("texture files released", files.0.len() as u64);
            log.count("bytes of the texture files released", bytes(&files.0));
        }
        if let Some(twins) = world.remove_resource::<LowTexturePayloads>() {
            log.count("low-resolution texture files released", twins.0.len() as u64);
            log.count(
                "bytes of the low-resolution texture files released",
                bytes(&twins.0),
            );
        }
        Ok(())
    }
}
