//! What the area tests share: the trusted decoder they are given, and the passes they run on a read area.
#![allow(dead_code)]
use primase_core::World;
use primase_core::pass::{PassManager, PassRegistry, RunReport};

/// The geometry the earlier reader produced by itself, as explicit choices: distant backdrops left out, static
/// meshes posed, the finest level of detail, each material's UV tiling applied, windings repaired from the stored
/// normals, degenerate triangles dropped, sky cards, light volumes, shadow proxies and billboard cards left out,
/// blocker cards recorded rather than drawn.
pub const LEGACY_GEOMETRY: &str = "drop-fromsoft-far, fromsoft-apply-rigid-skinning, \
    lower-fromsoft-geometry{pose=rigid,uv-tiling=apply,winding=normals,degenerates=drop,leave-out=sky+light-volume+shadow+billboard,blocker-cards=record,speedtree-frame=program}, \
    select-lod{form=finest}";

/// The FromSoft passes and `select-lod`.
pub fn manager() -> PassManager {
    let mut registry = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    registry.add(&primase_core::SELECT_LOD);
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

/// The threads the configuration lets a heavy test's work use (`[run] threads`), as the world states them
/// (`core.threads`).
pub fn threads() -> primase_core::parallel::Threads {
    primase_core::parallel::Threads::new(primase_testdata::threads().get()).expect("at least one thread")
}

/// The version-6 Oodle codec: the configured game library on Windows, oozextract elsewhere.
#[allow(unsafe_code)]
pub fn oodle(
    library: primase_testdata::OodleLibrary,
) -> primase_compress::Result<primase_compress::oodle::Oodle> {
    // SAFETY: the test configuration names a trusted native library.
    unsafe { primase_compress::oodle::Oodle::open(library.path(), primase_compress::oodle::Version::V6) }
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

/// A configured game's archives (`installs.sekiro`, or `installs.elden` with `keys.elden`), or `None` when the
/// configuration lacks them.
pub fn archive(
    game: primase_fromsoft::Game,
) -> Result<Option<primase_fromsoft::Archive>, Box<dyn std::error::Error>> {
    use primase_fromsoft::{Archive, Game, Keys};
    use primase_testdata::{Game as Data, install, keys};
    Ok(match game {
        Game::Sekiro => install(Data::Sekiro).map(Archive::open).transpose()?,
        Game::EldenRing => match (install(Data::Elden), keys(Data::Elden)) {
            (Some(dir), Some(keys)) => Some(Archive::open_game(dir, game, &Keys::from_folder(keys, game)?)?),
            _ => None,
        },
    })
}

/// The reference readers (SoulsFormats) rename repeated entries in file order: the second and later uses of a name (and every blank
/// name) become `"<name> {n}"`, repeating until no name is ambiguous.
pub fn disambiguate(names: &mut [String]) {
    loop {
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::from([(String::new(), 0)]);
        let mut ambiguous = false;
        for name in names.iter_mut() {
            if !counts.contains_key(name.as_str()) && !name.is_empty() {
                counts.insert(name.clone(), 1);
            } else {
                ambiguous = true;
                let n = counts.entry(name.clone()).or_default();
                *n += 1;
                *name = format!("{name} {{{n}}}");
            }
        }
        if !ambiguous {
            break;
        }
    }
}

/// A float a reference dump writes as its u32 bit pattern in hex.
pub fn bits(v: &serde_json::Value) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(u32::from_str_radix(v.as_str().ok_or("float bits")?, 16)?)
}

/// Three floats a reference dump writes as bit patterns.
pub fn bits3(v: &serde_json::Value) -> Result<[u32; 3], Box<dyn std::error::Error>> {
    let a = v.as_array().ok_or("vector")?;
    Ok([bits(&a[0])?, bits(&a[1])?, bits(&a[2])?])
}

/// The cells a smoke gate samples of a region `[west, south, east, north]` (metres): a 3 by 3 grid of equal cells,
/// west to east, then south to north.
pub fn cells(region: [f32; 4]) -> Vec<[f32; 4]> {
    let [w, s, e, n] = region;
    let (dx, dy) = ((e - w) / 3.0, (n - s) / 3.0);
    (0..3)
        .flat_map(|row| {
            (0..3).map(move |col| {
                let (x, y) = (w + col as f32 * dx, s + row as f32 * dy);
                [x, y, x + dx, y + dy]
            })
        })
        .collect()
}

/// `n` distinct indices of `0..count` drawn with `seed` (splitmix64), in the order drawn: the same seed, the same
/// sample.
pub fn sample(seed: u64, count: usize, n: usize) -> Vec<usize> {
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let mut picked = Vec::new();
    while picked.len() < n.min(count) {
        let k = (next() % count as u64) as usize;
        if !picked.contains(&k) {
            picked.push(k);
        }
    }
    picked
}
