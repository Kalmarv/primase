//! Installed Stormveil environment census and full-coverage upload-fingerprint oracles.
mod common;
mod inputs;
use fromsoft_formats::{Game, Keys, dcx::Decoder, msb::MapLayout};
use primase_core::{Name, ReflectionProbeRegion, Source, TextureLayers, TextureShape, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::environment::{self, Selection, Tier};
use std::collections::{BTreeMap, BTreeSet};

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1_469_598_103_934_665_603_u64, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(1_099_511_628_211)
    })
}

#[test]
fn all_stormveil_boxes_resolve_and_captured_payload_fingerprints_match()
-> Result<(), Box<dyn std::error::Error>> {
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
        .map(environment::group_for)
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
        assert_eq!(environment::group_for(region)?, id.parse::<u32>()?, "{name}");
        labelled += 1;
    }
    assert_eq!(labelled, 55);
    let mut world = World::new();
    let report = environment::read(
        &archive,
        &decoder,
        "m10_00_00_00",
        &layout,
        Selection {
            state_a: 5,
            state_b: 6,
            factor: f32::from_bits(0x3eb4e81c),
            tier: Tier::High,
        },
        &mut world,
    )?;
    assert_eq!(
        (
            report.packages,
            report.textures,
            report.boxes,
            report.groups,
            report.unresolved_captured_boxes
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
    let mut drawn = World::new();
    primase_fromsoft::sekiro::read_game_area(
        &install,
        "m10_00_00_00",
        &decoder,
        &primase_fromsoft::sekiro::Options {
            region: Some(primase_fromsoft::sekiro::Region {
                rect: [1e6, 1e6, 1e6 + 1., 1e6 + 1.],
                margin: 0.,
            }),
            render_graph: true,
            environment: report.selection,
            ..Default::default()
        },
        &mut drawn,
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
