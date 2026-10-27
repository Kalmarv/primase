//! The GTA reader chooses nothing (PRIMASE-PLAN step 16, the reader's role): a region read and every placement loaded,
//! with no pass run, keeps what the source stores. Every level of detail stays a form of its prototype (`core.lods`;
//! `select-lod` chooses), fragments keep their bones unposed (`gta.bones`; `gta-pose-fragments` poses), and materials
//! stay GTA shaders (`gta.shader`; `lower-gta-pbr` lowers, so no `pbr.material` exists yet).
//!
//! Needs `installs.gta` and `keys.gta`; reads a corner of Legion Square.
mod inputs;

use primase_core::pbr::PbrMaterial;
use primase_core::{Instance, Lods};
use primase_gta::components::{GtaBones, GtaShader};
use primase_gta::read::source::GtaSource;

#[test]
fn the_reader_keeps_levels_of_detail_bones_and_shaders_as_stored() {
    let (Some(dir), Some(keys)) = (
        inputs::install(inputs::Game::Gta),
        inputs::keys(inputs::Game::Gta),
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
        inputs::install(inputs::Game::Gta),
        inputs::keys(inputs::Game::Gta),
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

/// A reader asked for the shader database loads the effects its materials name with the programs they list, each
/// material linked to its effect, in the same load: the loaded part holds exactly the effects its materials use (not the
/// database's 388), no name beyond the declared ones, and `resolve-gta-shaders` binds its materials with no second
/// reading of the install. Without the option the load holds none of it.
#[test]
fn the_reader_reads_the_shader_database_with_the_load_when_asked() {
    use primase_core::convert::Reader;
    use primase_core::pass::{Options, PassLog};
    use primase_core::shader::{Shader, ShaderBinding};
    use primase_gta::components::{GtaEffect, GtaMaterialEffect};
    use primase_gta::convert::{EMITTED_NAMES, GtaOptions, GtaReader, SHADER_NAMES};
    use primase_gta::passes::resolve_shaders::RESOLVE_GTA_SHADERS;
    let (Some(install), Some(keys)) = (
        inputs::install(inputs::Game::Gta),
        inputs::keys(inputs::Game::Gta),
    ) else {
        return;
    };
    let options = |shaders| GtaOptions {
        install: install.clone(),
        keys: keys.clone(),
        region: [170.0, -970.0, 230.0, -910.0],
        reach: 0.0,
        shaders,
    };
    let load = |shaders| {
        let mut reader = GtaReader::open(&options(shaders)).expect("install");
        let (read, _) = reader.read().expect("region");
        let roots = read.iter::<Instance>().map(|(e, _)| e).collect();
        let (w, _) = reader.load(&read, &roots).expect("assets");
        (w, reader.notices())
    };
    let (without, _) = load(false);
    assert_eq!(without.iter::<GtaEffect>().count(), 0);
    assert_eq!(without.iter::<GtaMaterialEffect>().count(), 0);
    let (mut w, notices) = load(true);
    let materials = w.iter::<GtaShader>().count();
    let joined = w.iter::<GtaMaterialEffect>().count();
    let effects = w.iter::<GtaEffect>().count();
    let programs = w.iter::<Shader>().count();
    let used: std::collections::BTreeSet<_> = w.iter::<GtaMaterialEffect>().map(|(_, l)| l.effect).collect();
    println!(
        "{materials} materials, {joined} joined to {effects} effects, {programs} programs; notices {:?}",
        notices.iter().map(|n| (&n.reason, n.count)).collect::<Vec<_>>()
    );
    assert!(joined > 0 && joined <= materials);
    assert_eq!(effects, used.len(), "effects no material uses were read");
    assert!(effects < 388, "the whole database was read");
    assert!(programs > 0);
    // the unjoined materials are noticed
    let unjoined = materials - joined;
    let noticed: u64 = notices
        .iter()
        .filter(|n| n.reason.contains("effect"))
        .map(|n| n.count)
        .sum();
    assert!(unjoined == 0 || noticed > 0, "{unjoined} unjoined, none noticed");
    // no name beyond the declared ones
    let declared: std::collections::BTreeSet<&str> =
        EMITTED_NAMES.iter().chain(SHADER_NAMES).copied().collect();
    let undeclared: Vec<String> = w
        .component_names()
        .into_iter()
        .chain(w.resource_names())
        .filter(|n| !declared.contains(*n))
        .map(str::to_owned)
        .collect();
    assert!(undeclared.is_empty(), "emitted but not declared: {undeclared:?}");
    // and the shaders resolve on it
    let o = Options::from([
        ("technique".to_owned(), "deferred_draw".to_owned()),
        ("unset-textures".to_owned(), "white".to_owned()),
    ]);
    RESOLVE_GTA_SHADERS
        .instance(&o)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    assert!(w.iter::<ShaderBinding>().count() > 0);
}
