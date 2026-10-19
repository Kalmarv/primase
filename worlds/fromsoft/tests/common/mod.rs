//! What the area tests share: the trusted decoder they are given, and the passes they run on a read area.
#![allow(dead_code)]
use primase_core::World;
use primase_core::pass::{PassManager, PassRegistry, RunReport};

/// The geometry the earlier reader produced by itself, as explicit choices: distant backdrops left out, static
/// meshes posed, the finest level of detail, each material's UV tiling applied, windings repaired from the stored
/// normals, degenerate triangles dropped, sky cards, light volumes and shadow proxies left out, blocker cards
/// recorded rather than drawn.
pub const LEGACY_GEOMETRY: &str = "drop-fromsoft-far, fromsoft-apply-rigid-skinning, \
    lower-fromsoft-geometry{pose=rigid,uv-tiling=apply,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow,blocker-cards=record}, \
    select-lod{form=finest}";

/// The collision the earlier reader produced by itself, as explicit choices: map collision parts of hit filters 8 and
/// 27, HitMtrl classes 22 and 36 as water, blockers (29) and class 40 left out; object bodies of motion type 2 and
/// those classes left out, convex bodies tessellated as their offset planes and given the surface of their
/// prototype's largest render area; both cropped 50 m around the read region. It runs before the geometry passes, which may despawn placements.
pub const LEGACY_COLLISION: &str = "lower-fromsoft-collision{hit-filters=8+27,water=22+36,leave-out=29+40,reach=50}, \
    lower-fromsoft-object-collision{convex=tessellate,leave-out=22+29+36+40,leave-out-motion=2,convex-surface=render,reach=50}";

/// The FromSoft passes and `select-lod`.
pub fn manager() -> PassManager {
    let mut registry = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    registry.register("select-lod", primase_core::select_lod);
    PassManager::new(registry)
}

/// Run `pipeline` on `world`, with the conservation check enforced: every placement, primitive, triangle and
/// texture a pass leaves out is one it declares.
pub fn run(world: &mut World, pipeline: &str) -> Result<RunReport, Box<dyn std::error::Error>> {
    let m = manager().with_conservation(primase_core::ledger::Conservation::Enforce);
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

/// One quad on the ground, u along +x and v along +z, with Sekiro's stored frame for its most common class: the
/// tangent along +v and the bitangent `cross(N, T) * w` along +u, so source red follows u and green v. Its material
/// binds `texture` (in the source convention) as its normal map.
pub fn quad(
    texture: primase_core::Texture,
) -> (primase_core::World, primase_core::Entity, primase_core::Entity) {
    use primase_core::pbr::{PbrMaterial, TextureRef};
    use primase_core::{Mesh, Primitive, Semantic, Stream, Topology, World};
    let mut world = World::default();
    let t = world.spawn();
    world.insert(t, texture);
    let m = world.spawn();
    world.insert(
        m,
        PbrMaterial {
            normal_texture: Some(TextureRef::new(t)),
            ..Default::default()
        },
    );
    let e = world.spawn();
    world.insert(
        e,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: std::collections::BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.], [1., 0., 1.]]),
                ),
                (Semantic::Normal, Stream::F32x3(vec![[0., 1., 0.]; 4])),
                (Semantic::Tangent, Stream::F32x4(vec![[0., 0., 1., 1.]; 4])),
                (
                    Semantic::TexCoord(0),
                    Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.], [1., 1.]]),
                ),
            ]),
            indices: Some(vec![0, 2, 1, 1, 2, 3]),
            material: Some(m),
        }]),
    );
    (world, t, e)
}

pub fn image_passes(world: &mut primase_core::World, pipeline: &str) -> primase_core::pass::RunReport {
    let mut registry = primase_core::pass::PassRegistry::new();
    primase_image::register(&mut registry);
    let manager = primase_core::pass::PassManager::new(registry);
    let mut report = primase_core::pass::RunReport::default();
    manager
        .run(
            world,
            &manager.registry.parse(pipeline).expect("pipeline parses"),
            &mut report,
        )
        .expect("passes run");
    report
}
