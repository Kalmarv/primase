#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
//! `lower-trackmania-materials` over the campaigns' maps (`installs.tmnf`), run with the pass checks.
//!
//! - L1: every material is lowered but those of the shaders the pass names as having no PBR form (the additive glows,
//!   the fake shadows and the grass fins), and no primitive still draws with one of those; every primitive drawn with a
//!   projected material holds the projected set, one coordinate pair per vertex; every turned block whose prototype
//!   draws one takes the variant of its turn, which names a turned copy for each such primitive.

mod inputs;

use std::collections::{BTreeMap, BTreeSet};

use inputs::{Game, install};
use primase_core::convert::Reader;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, MaterialVariants, Mesh, Semantic, Stream};
use primase_trackmania::components::{TrackmaniaBlock, TrackmaniaMaterial};
use primase_trackmania::passes::LowerTrackmaniaMaterials;
use primase_trackmania::passes::materials::PROJECTED_SET;
use primase_trackmania::read::GameFiles;
use primase_trackmania::{FRAME, TrackmaniaReader};

/// The shaders the pass leaves without a PBR form: the additive glows, the fake shadows and the grass fins.
const NOT_LOWERED: &[&str] = &[
    "ShadowSkirt",
    "TAdd Night",
    "TAdd Night ZBias",
    "TSelfI Add",
    "VDep Fence",
];

#[test]
fn l1_campaign_materials_lower() {
    let Some(root) = install(Game::Tmnf) else {
        return;
    };
    let files = GameFiles::open(&root).unwrap();
    let maps: Vec<String> = files
        .loose_paths()
        .filter(|p| p.starts_with(r"tracks\campaigns\nations\") && p.ends_with(".challenge.gbx"))
        .map(str::to_owned)
        .collect();
    assert!(!maps.is_empty());
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let mut unlowered = BTreeSet::new();
    let (mut projected, mut turned) = (0usize, 0usize);
    for path in &maps {
        let bytes = files.read(path).unwrap().unwrap();
        let mut reader = TrackmaniaReader::new(&files, &bytes, path.clone(), FRAME);
        let (mut w, _) = reader.read().unwrap();
        let mut report = RunReport::default();
        manager
            .run(
                &mut w,
                &Pipeline {
                    passes: vec![Box::new(LowerTrackmaniaMaterials)],
                },
                &mut report,
            )
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        for (e, m) in w.iter::<TrackmaniaMaterial>() {
            if !w.has::<PbrMaterial>(e) {
                unlowered.insert(m.shader_name().unwrap_or("-").to_owned());
            }
        }
        // the materials whose base colour is projected: those sampling the projected set
        let samples_projected = |m: primase_core::Entity| {
            w.get::<PbrMaterial>(m)
                .and_then(|p| p.base_color_texture.as_ref())
                .is_some_and(|t| t.texcoord == PROJECTED_SET)
        };
        let mut draws_projected = BTreeMap::new();
        for (e, mesh) in w.iter::<Mesh>() {
            let mut any = false;
            for p in mesh.primitives() {
                if let Some(m) = p.material {
                    assert!(
                        w.has::<PbrMaterial>(m),
                        "{path}: a primitive drawn with an unlowered material"
                    );
                }
                if !p.material.is_some_and(samples_projected) {
                    continue;
                }
                any = true;
                projected += 1;
                let Some(Stream::F32x3(positions)) = p.streams.get(&Semantic::Position) else {
                    panic!("{path}: a primitive without positions");
                };
                let Some(Stream::F32x2(set)) = p.streams.get(&Semantic::TexCoord(PROJECTED_SET)) else {
                    panic!("{path}: a projected primitive without the projected set");
                };
                assert_eq!(set.len(), positions.len(), "{path}");
            }
            draws_projected.insert(e, any);
        }
        for (e, b) in w.iter::<TrackmaniaBlock>() {
            let Some(i) = w.get::<Instance>(e) else { continue };
            let draws = draws_projected.get(&i.prototype).copied().unwrap_or(false);
            if b.direction == 0 || !draws {
                assert_eq!(i.variant, 0, "{path}: {}", b.name);
                continue;
            }
            turned += 1;
            assert_eq!(i.variant, u32::from(b.direction), "{path}: {}", b.name);
            let table = w.get::<MaterialVariants>(i.prototype).unwrap();
            let mesh = w.get::<Mesh>(i.prototype).unwrap();
            for (k, p) in mesh.primitives().iter().enumerate() {
                if p.material.is_some_and(samples_projected) {
                    let copy = table.0[k][&i.variant];
                    assert!(w.has::<PbrMaterial>(copy), "{path}: {}", b.name);
                }
            }
        }
    }
    println!(
        "{} maps: {projected} projected primitives, {turned} turned blocks drawing them",
        maps.len()
    );
    let expected: BTreeSet<String> = NOT_LOWERED.iter().map(|s| (*s).to_owned()).collect();
    assert!(
        unlowered.is_subset(&expected),
        "shaders not lowered beyond the named ones: {:?}",
        unlowered.difference(&expected).collect::<Vec<_>>()
    );
    assert!(projected > 0 && turned > 0);
}
