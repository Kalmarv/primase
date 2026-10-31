//! Installed Stormveil environment census and full-coverage upload-fingerprint oracles.
mod common;
mod inputs;
use fromsoft_formats::{Game, Keys, dcx::Decoder, msb::MapLayout};
use primase_core::convert::Reader;
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Name, ReflectionProbeRegion, Source, TextureLayers, TextureShape, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::environment;
use primase_fromsoft::sekiro::environment_data::{
    EnvironmentCatalog, EnvironmentLoadRequest, EnvironmentSelection,
};
fn run(world: &mut World, text: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut registry = PassRegistry::new();
    primase_fromsoft::sekiro::passes::register(&mut registry);
    let manager = PassManager::new(registry).with_pass_checks(true);
    manager.run(world, &manager.registry.parse(text)?, &mut RunReport::default())?;
    Ok(())
}
const SELECT: &str = "select-fromsoft-environment{state-a=5,state-b=6,factor=0.35333335,tier=high}";
use std::collections::{BTreeMap, BTreeSet};

/// Also supplies a standalone process for catalog time/peak-memory measurement before landing.
#[test]
fn stormveil_area_reader_keeps_authored_environment_metadata_without_selection()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (Some(root), Some(keys), Some(dll)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let keys = Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?;
    let install = common::install(root, Game::EldenRing, keys);
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut world = World::new();
    let started = std::time::Instant::now();
    let report = primase_fromsoft::sekiro::read_game_area(
        &install,
        "m10_00_00_00",
        &decoder,
        &primase_fromsoft::sekiro::Options {
            region: Some(primase_fromsoft::sekiro::Region {
                rect: [-162., 5., 108., 250.],
                margin: 30.,
            }),
            irradiance: true,
            render_graph: true,
            sky: primase_fromsoft::sekiro::sky::Sky::Include,
            map_records: true,
            ..Default::default()
        },
        &mut world,
    )?;
    let elapsed = started.elapsed().as_secs_f64();
    let metadata = report.environment.expect("authored environment report");
    assert_eq!(
        (
            metadata.packages,
            metadata.boxes,
            metadata.groups,
            metadata.textures
        ),
        (21, 129, 128, 0)
    );
    assert!(world.resource::<EnvironmentSelection>().is_none());
    assert!(world.resource::<EnvironmentLoadRequest>().is_none());
    assert!(
        world
            .resource::<primase_fromsoft::sekiro::environment_data::EnvironmentPayloads>()
            .is_none()
    );
    assert_eq!(world.iter::<ReflectionProbeRegion>().count(), 0);
    assert_eq!(
        world
            .iter::<primase_fromsoft::sekiro::environment_data::EnvironmentRegion>()
            .count(),
        129
    );
    assert!(
        world
            .iter::<primase_fromsoft::sekiro::environment_data::EnvironmentRegion>()
            .all(|(_, r)| r.available_packages.len() == 21)
    );
    assert!(
        world
            .resource::<primase_core::render_graph::RenderGraph>()
            .expect("raw graph")
            .lighting
            .is_none()
    );
    assert!(
        !world
            .iter::<primase_core::ContentNote>()
            .any(|(_, n)| n.key == "environment-selection")
    );
    println!(
        "ENVIRONMENT_METADATA_MEASUREMENT {}",
        serde_json::json!({"area_read_seconds":elapsed,"catalog_seconds":metadata.catalog_seconds,"catalog_largest_member_bytes":metadata.catalog_largest_member_bytes,"catalog_packages":metadata.packages,"environment_textures_attached":metadata.textures,"rect":[-162.,5.,108.,250.],"margin":30.})
    );
    Ok(())
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1_469_598_103_934_665_603_u64, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(1_099_511_628_211)
    })
}

#[test]
fn all_stormveil_boxes_resolve_and_captured_payload_fingerprints_match()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (Some(root), Some(keys), Some(dll)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let keys = Keys::from_folder(keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?;
    let install = common::install(root, Game::EldenRing, keys);
    let archive = install.open()?;
    let layout = MapLayout::read(&archive.read("/map/MapStudio/m10_00_00_00.msb.dcx", &decoder)?)?;
    let regions: Vec<_> = layout
        .map_regions()?
        .into_iter()
        .filter(|r| r.type_name == Some("environment_map_effect_box"))
        .collect();
    assert_eq!(regions.len(), 129);
    let groups: BTreeSet<_> = regions
        .iter()
        .map(|r| {
            environment::group_for(
                r,
                &fromsoft_formats::envmap::box_fields(&layout, r)?.expect("ER box fields"),
            )
        })
        .collect::<Result<_, _>>()?;
    assert_eq!(groups.len(), 128);
    let mut labelled = 0;
    for line in include_str!("fixtures/ambient-groups.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
    {
        let (name, id) = line.split_once('\t').expect("label columns");
        let region = regions
            .iter()
            .find(|r| r.name == name)
            .expect("captured box remains installed");
        assert_eq!(
            environment::group_for(
                region,
                &fromsoft_formats::envmap::box_fields(&layout, region)?.expect("ER box fields")
            )?,
            id.parse::<u32>()?,
            "{name}"
        );
        labelled += 1;
    }
    assert_eq!(labelled, 55);
    let mut world = World::new();
    // Setup: metadata read -> explicit registered selection -> payload load -> registered lowering.
    let metadata = environment::read(&archive, &decoder, "m10_00_00_00", &layout, &mut world)?;
    assert_eq!(
        world
            .resource::<EnvironmentCatalog>()
            .expect("authored catalog")
            .packages
            .len(),
        21
    );
    assert_eq!(world.iter::<TextureLayers>().count(), 0);
    assert_eq!(world.iter::<ReflectionProbeRegion>().count(), 0);
    assert!(world.resource::<EnvironmentSelection>().is_none());
    assert!(world.resource::<EnvironmentLoadRequest>().is_none());
    assert_eq!(world.iter::<primase_core::ContentNote>().count(), 0);
    run(&mut world, SELECT)?;
    assert_eq!(world.iter::<TextureLayers>().count(), 0);
    let loaded = environment::load_requested(&archive, &decoder, &mut world)?;
    run(&mut world, "lower-fromsoft-environment-probes")?;
    assert_eq!(
        (
            loaded.packages,
            loaded.textures,
            metadata.boxes,
            metadata.groups,
            world
                .iter::<primase_core::ContentNote>()
                .filter(|(_, n)| n.key == "environment-unresolved-capture")
                .count()
        ),
        (2, 1280, 129, 128, 1)
    );
    assert_eq!(world.iter::<ReflectionProbeRegion>().count(), 129);
    assert!(
        world.verify_components().is_empty(),
        "{:?}",
        world.verify_components()
    );
    assert_eq!(
        world
            .iter::<Source>()
            .filter(|(_, s)| s.0.ends_with(":probe:0901"))
            .count(),
        2
    );
    let textures: BTreeMap<_, _> = world
        .iter::<TextureLayers>()
        .map(|(e, t)| (world.get::<Name>(e).expect("texture identity").0.as_str(), t))
        .collect();
    let mut names = BTreeSet::new();
    let mut checked = 0;
    for line in include_str!("fixtures/ambient-fingerprints.tsv")
        .lines()
        .filter(|l| !l.starts_with('#'))
    {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields.len(), 9);
        let texture = textures.get(fields[0]).expect("capture-matched texture");
        let sub: usize = fields[1].parse()?;
        let (mip, payload) = match texture.shape {
            TextureShape::Cube { cubes: 1 } => {
                let mip = sub % texture.levels.len();
                (mip, texture.levels[mip][sub / texture.levels.len()].clone())
            }
            TextureShape::Volume { .. } => {
                assert_eq!(sub, 0);
                (0, texture.levels[0].concat())
            }
            _ => panic!("unexpected probe texture shape"),
        };
        assert_eq!((texture.width >> mip).max(1), fields[2].parse::<u32>()?);
        assert_eq!((texture.height >> mip).max(1), fields[3].parse::<u32>()?);
        let row: usize = fields[5].parse()?;
        let rows: usize = fields[6].parse()?;
        assert_eq!(payload.len(), row * rows, "{} sub{sub}", fields[0]);
        assert!(
            payload.len() <= 65536,
            "this oracle only covers complete retained subresources"
        );
        assert_eq!(
            fnv(&payload[..row]),
            u64::from_str_radix(fields[7], 16)?,
            "{} sub{sub} first row",
            fields[0]
        );
        assert_eq!(
            fnv(&payload),
            u64::from_str_radix(fields[8], 16)?,
            "{} sub{sub} full payload",
            fields[0]
        );
        names.insert(fields[0]);
        checked += 1;
    }
    assert_eq!((names.len(), checked), (286, 3476));
    // The production area reader binds every installed region through generic geometry and texture roles.
    let mut reader = primase_fromsoft::sekiro::convert::AreaReader::of(
        install.clone(),
        "m10_00_00_00",
        Decoder {
            oodle: Some(&dll),
            max_output: 1 << 30,
        },
        primase_fromsoft::sekiro::Options {
            region: Some(primase_fromsoft::sekiro::Region {
                rect: [1e6, 1e6, 1e6 + 1., 1e6 + 1.],
                margin: 0.,
            }),
            render_graph: true,
            irradiance: true,
            ..Default::default()
        },
    );
    let (mut drawn, _) = reader.read()?;
    assert!(
        drawn
            .resource::<primase_core::render_graph::RenderGraph>()
            .expect("raw graph")
            .lighting
            .is_none()
    );
    run(&mut drawn, SELECT)?;
    let (mut drawn, _) = reader.load_all(drawn)?;
    run(
        &mut drawn,
        "lower-fromsoft-render-graph,lower-fromsoft-environment-probes,bind-fromsoft-environment-probes",
    )?;
    let graph = drawn
        .resource::<primase_core::render_graph::RenderGraph>()
        .expect("frame graph");
    primase_core::Resource::verify(graph, &drawn)?;
    let pipeline = graph.lighting.as_ref().expect("lighting pipeline");
    assert!(pipeline.frame.is_some());
    let group = pipeline
        .passes
        .iter()
        .map(|i| &graph.passes[*i as usize])
        .find(|p| p.name == "GXEnvMapRegion")
        .expect("selected environment");
    let primase_core::render_graph::PassWork::Group { steps } = &group.work else {
        panic!("region group")
    };
    assert_eq!(steps.len(), 129);
    for pass in steps {
        let primase_core::render_graph::PassWork::Geometry { stages, .. } = &pass.work else {
            panic!("unit-box region draw")
        };
        let pixel = stages
            .iter()
            .find(|s| s.stage == iridia_interface::Stage::Pixel)
            .expect("pixel stage");
        assert_eq!(
            pixel
                .textures
                .iter()
                .filter(|t| matches!(t.source, primase_core::shader::TextureSource::Texture { .. }))
                .count(),
            10
        );
        assert!(pixel.constants.iter().any(|c| matches!(
            c.role,
            primase_core::shader::ConstantRole::Object(primase_core::shader::ObjectParameter::ViewToObject)
        )));
    }
    assert_eq!(
        graph
            .passes
            .iter()
            .filter(|p| p.name == "GXEnvMapRegion[Stencil]")
            .count(),
        6
    );
    assert!(
        pipeline
            .notices
            .iter()
            .any(|n| n.contains("six native environment stencil"))
    );
    assert!(pipeline.notices.iter().any(|n| n.contains("0-39% dimmer")));
    Ok(())
}
