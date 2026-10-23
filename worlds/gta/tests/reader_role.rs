//! The GTA reader chooses nothing (PRIMASE-PLAN step 16, the reader's role): a region read and every placement loaded,
//! with no pass run, keeps what the source stores. Every level of detail stays a form of its prototype (`core.lods`;
//! `select-lod` chooses), fragments keep their bones unposed (`gta.bones`; `gta-pose-fragments` poses), and materials
//! stay GTA shaders (`gta.shader`; `lower-gta-pbr` lowers, so no `pbr.material` exists yet).
//!
//! Needs `installs.gta` and `keys.gta`; reads a corner of Legion Square.
use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Lods};
use primase_gta::components::{GtaBones, GtaShader};
use primase_gta::read::source::GtaSource;

#[test]
fn the_reader_keeps_levels_of_detail_bones_and_shaders_as_stored() {
    let (Some(dir), Some(keys)) = (
        primase_testdata::install(primase_testdata::Game::Gta),
        primase_testdata::keys(primase_testdata::Game::Gta),
    ) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).expect("install");
    let (read, _) = source.read([170.0, -970.0, 230.0, -910.0], 0.0).expect("region");
    let (w, _) = source.load_all(&read).expect("assets");
    let placements = w.iter::<Instance>().count();
    let with_forms = w.iter::<Lods>().filter(|(_, l)| !l.forms.is_empty()).count();
    let boned = w.iter::<GtaBones>().count();
    let shaders = w.iter::<GtaShader>().count();
    let lowered = w.iter::<PbrMaterial>().count();
    println!(
        "{placements} placements; {with_forms} prototypes with other forms; {boned} with bones; {shaders} shaders; \
         {lowered} lowered"
    );
    assert!(placements > 0, "nothing read");
    assert!(with_forms > 0, "no prototype kept its other levels of detail");
    assert!(boned > 0, "no fragment kept its bones");
    assert!(shaders > 0, "no material kept its GTA shader");
    assert_eq!(lowered, 0, "materials lowered by the reader");
}

/// The reader declares every name its read and load put in a world (`read::convert::EMITTED_NAMES`, what a coverage
/// report lists): a corner of Legion Square, read and every placement loaded, holds no other name. Declared names the
/// corner doesn't hold are listed here with why, so the declaration grows on purpose.
#[test]
fn the_reader_declares_every_name_it_emits() {
    let (Some(dir), Some(keys)) = (
        primase_testdata::install(primase_testdata::Game::Gta),
        primase_testdata::keys(primase_testdata::Game::Gta),
    ) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys).expect("install");
    let (read, _) = source.read([170.0, -970.0, 230.0, -910.0], 0.0).expect("region");
    let (loaded, _) = source.load_all(&read).expect("assets");
    let declared: std::collections::BTreeSet<&str> = primase_gta::read::convert::EMITTED_NAMES
        .iter()
        .copied()
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for w in [&read, &loaded] {
        for n in w.component_names().into_iter().chain(w.resource_names()) {
            seen.insert(n.to_owned());
        }
    }
    let undeclared: Vec<&String> = seen.iter().filter(|n| !declared.contains(n.as_str())).collect();
    assert!(undeclared.is_empty(), "emitted but not declared: {undeclared:?}");
    // declared, held by regions the corner is not: water quads and their surfaces
    let not_in_corner = ["gta.water", "gta.water_surface"];
    let unseen: Vec<&&str> = declared
        .iter()
        .filter(|n| !seen.contains(**n) && !not_in_corner.contains(n))
        .collect();
    assert!(
        unseen.is_empty(),
        "declared, emitted by the corner and not listed why: {unseen:?}"
    );
}
