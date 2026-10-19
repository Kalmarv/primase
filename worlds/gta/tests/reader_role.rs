//! The GTA reader chooses nothing (PRIMASE-PLAN step 16, the reader's role): a region read and every placement loaded,
//! with no pass run, keeps what the source stores. Every level of detail stays a form of its prototype (`core.lods`;
//! `select-lod` chooses), fragments keep their bones unposed (`gta.bones`; `gta-pose-fragments` poses), and materials
//! stay GTA shaders (`gta.shader`; `lower-gta-pbr` lowers, so no `pbr.material` exists yet).
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`; reads a corner of Legion Square.
use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Lods};
use primase_gta::components::{GtaBones, GtaShader};
use primase_gta::pipeline::{GtaSource, Map};

#[test]
fn the_reader_keeps_levels_of_detail_bones_and_shaders_as_stored() {
    let (Some(dir), Some(keys)) = (
        primase_testdata::real_data("PRIMASE_GTA_DIR"),
        primase_testdata::real_data("PRIMASE_GTA_KEYS"),
    ) else {
        return;
    };
    let source = GtaSource::open(&dir, &keys, Map::Online).expect("install");
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
