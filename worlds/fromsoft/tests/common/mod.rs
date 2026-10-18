//! What the area tests share: the trusted decoder they are given, and the passes they run on a read area.
#![allow(dead_code)]
use primase_core::World;
use primase_core::pass::{PassManager, PassRegistry, RunReport};

/// The geometry the earlier reader produced by itself, as explicit choices: distant backdrops left out, static
/// meshes posed, the finest level of detail, each material's UV tiling applied, windings repaired from the stored
/// normals, degenerate triangles dropped, sky cards, light volumes and shadow proxies left out, blocker cards
/// recorded rather than drawn.
pub const LEGACY_GEOMETRY: &str = "drop-fromsoft-far, fromsoft-apply-rigid-skinning, \
    lower-fromsoft-geometry{uv-tiling=apply,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow,blocker-cards=record}, \
    select-lod{form=finest}";

/// The FromSoft passes and `select-lod`.
pub fn manager() -> PassManager {
    let mut registry = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    registry.register("select-lod", primase_core::select_lod);
    PassManager::new(registry)
}

/// Run `pipeline` on `world`.
pub fn run(world: &mut World, pipeline: &str) -> Result<RunReport, Box<dyn std::error::Error>> {
    let m = manager();
    let mut report = RunReport::default();
    m.run(world, &m.registry.parse(pipeline)?, &mut report)?;
    Ok(report)
}

/// The caller's trusted version-6 Oodle library.
#[allow(unsafe_code)]
pub fn oodle(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
    // SAFETY: the test configuration names a trusted native library.
    unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
}
