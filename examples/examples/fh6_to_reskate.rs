//! The Shibuya Scramble of Forza Horizon 6's Tokyo read, prepared and written as a skate. map folder (`out/shibuya`),
//! then, given a skate. install and ReSkate Studio's CLI, compiled and finished into `out/staging`. Usage:
//! `fh6_to_reskate <ForzaHorizon6 install> [<skate install> <ReSkate CLI>]`.
use std::path::{Path, PathBuf};

use forzatech_formats::world::{Area, Season};
use primase_core::convert::{Result, convert};
use primase_core::pass::{PassManager, PassRegistry, Pipeline};
use primase_core::pbr::TextureSlot;
use primase_core::{LodChoice, SelectLod};
use primase_forzatech::passes::{
    CollideForzaDrawn, Ground, LowerForzaMaterials, LowerForzaPlanarUv, SeeThrough, SelectForzaDrawn,
    SelectForzaMainView,
};
use primase_forzatech::read::convert::ForzaReader;
use primase_forzatech::read::open::Track;
use primase_forzatech::read::placements::Scope;
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_image::{Beyond, CapMode, CapTextures, MissingLevels, NormalDownscale, NormalizeNormals, OrmSize, PackOrm};
use primase_meshops::{BakeNonrigid, Mirrors, OmitUnshadedPrimitives, SpecializeMaterialVariants, UnusedVariants};
use primase_reskate::compile::{CompileChoices, Streaming, Studio, TimeOfDay, compile_map};
use primase_reskate::components::{CollisionSurfaces, CollisionWeld};
use primase_reskate::layered::{LowerLayeredReskate, OverlayLimit};
use primase_reskate::passes::{
    ClipReskateRegion, LightCalibration, PlaceReskateSpawn, PrepareReskateCollision, PrepareReskateFloor,
    PrepareReskateGrinds, PrepareReskateLaidLayers, PrepareReskateTextures, PrepareReskateUvTransforms, RaiseOptions,
    RaiseSkate, SamplingConflicts, SnapReskateMarkers, SummarizeReskateMap, Unclassified,
};
use primase_reskate::{EncodingOptions, MapSink};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).map(PathBuf::from);
    let usage = "usage: fh6_to_reskate <ForzaHorizon6 install> [<skate install> <ReSkate CLI>]";
    let fh6 = args.next().ok_or(usage)?;
    let compile = match (args.next(), args.next()) {
        (Some(skate), Some(cli)) => Some((skate, cli)),
        (None, None) => None,
        _ => return Err(usage.into()),
    };
    // the Shibuya Scramble, [west, south, east, north] in world metres (the game's x and z: north is its +z), and the
    // placements whose origins lie within 30 m of it
    let region = [-1550.0, -5000.0, -1100.0, -4566.67];
    let margin = 30.0;
    let area = Area::new(
        f64::from(region[0] - margin),
        f64::from(region[1] - margin),
        f64::from(region[2] + margin),
        f64::from(region[3] + margin),
    );
    let track = Track::open(fh6.join("media/Tracks/Brio"))?;
    let mut reader = ForzaReader::new(track, Scope { area, season: Season::Summer });
    let e = EncodingOptions {
        out_root: "out".into(),
        name: Some("shibuya".into()),
        max_render_edge: Some(48.0),
        ..EncodingOptions::default()
    };

    // Run once on the box as read: what the game draws near the camera, by the hand-placed LOD profiles of quality tier
    // 1 (the tier most profile groups hold a main-view record of; what a tier stands for is not established).
    let select = Pipeline { passes: vec![Box::new(SelectForzaDrawn::new(5.0, 1)?)] };

    // Run on each part, loaded, before it is written: its second level of detail, its materials lowered to
    // `pbr.material` (the families with a rule; the others are left out, counted), then the core world prepared as the
    // ReSkate writer encodes it. Levels and materials exist once a part is loaded.
    let prepare = Pipeline {
        passes: vec![
            Box::new(SelectLod { choice: LodChoice::AtMost(1) }),
            // a placement drawing another material variant gets a prototype of its own, so each tiles one way; the
            // variants nobody places are dropped, counted
            Box::new(SpecializeMaterialVariants::new(UnusedVariants::Discard)),
            Box::new(SelectForzaMainView),
            Box::new(LowerForzaMaterials),
            // the terrain and the roads' planar layers read the position of a surface point: its coordinates, per placement
            Box::new(LowerForzaPlanarUv),
            Box::new(CollideForzaDrawn { see_through: SeeThrough::LeaveOut, ground: Ground::World }),
            // what draws nothing the writer can take is left out, counted: a slot with no material instance, and the
            // shader families with no lowering rule yet; after the collision, so their ground stays solid
            Box::new(OmitUnshadedPrimitives),
            Box::new(NormalizeNormals),
            // the writer takes rigid placements: a mirrored or sheared one draws a baked copy of its prototype
            Box::new(BakeNonrigid { mirrors: Mirrors::Bake }),
            Box::new(PrepareReskateLaidLayers::default()),
            // each map of an environment material tiles on its own; skate samples one way, the base colour's
            Box::new(PrepareReskateUvTransforms::new(TextureSlot::BaseColor, SamplingConflicts::Adopt)),
            // skate's mask holds both: a material's separate occlusion and metallic-roughness maps packed as one, at the
            // metallic-roughness map's size
            Box::new(PackOrm::new(OrmSize::MetallicRoughness, MissingLevels::Generate)),
            // collision surfaces have no class yet: concrete, the city's most common ground (the program's choice)
            Box::new(RaiseSkate::new(RaiseOptions::new(
                "shibuya",
                LightCalibration::Standard,
                Unclassified::Surface("concrete".into()),
            )?)),
            // a road of two or three layers (core.layered) is drawn as its base layer with each other layer a decal
            // overlay over it, as the writer takes blends
            Box::new(LowerLayeredReskate::new(OverlayLimit::All)),
            Box::new(ClipReskateRegion::new(region, margin)?),
            Box::new(PrepareReskateFloor::new(100.0, 250.0)?),
            Box::new(PrepareReskateGrinds::studio()),
            Box::new(PlaceReskateSpawn::centre()),
            Box::new(SnapReskateMarkers::for_encoding(&e)?),
            Box::new(CapTextures::new(256, CapMode { beyond: Beyond::Filter, normals: NormalDownscale::Box })?),
            Box::new(PrepareReskateTextures),
            Box::new(PrepareReskateCollision::new(CollisionSurfaces::Coalesce, CollisionWeld::Millimetre, &e)?),
            Box::new(SummarizeReskateMap::for_encoding(&e)?),
        ],
    };

    let mut registry = PassRegistry::new();
    primase_forzatech::register(&mut registry);
    let writer = Box::new(MapSink::new(Path::new(""), &e));
    let report = convert(&mut reader, writer, &PassManager::new(registry), &select, &prepare)?;
    for notice in report.notices() {
        println!("{notice}");
    }

    // without Studio, the map folder is the result
    let Some((skate, cli)) = compile else {
        return Ok(());
    };
    let studio = Studio { cli, game: skate.clone() };
    let skate_oodle = load_oodle(&skate.join("oo2core_9_win64.dll"), Version::V9)?;
    let choices = CompileChoices {
        streaming: Some(Streaming::Cells),
        lods: true,
        gi: true,
        time_of_day: Some(TimeOfDay::Noon),
        pause_map: None,
    };
    let compiled =
        compile_map(&studio, Path::new("out/shibuya"), Path::new("out/staging"), "shibuya", &choices, &skate_oodle)?;
    println!("{} ({} warnings)", compiled.patch.display(), compiled.warnings);
    Ok(())
}

/// A game's Oodle library, which decompresses its archives.
#[allow(unsafe_code)]
fn load_oodle(path: &Path, version: Version) -> Result<Oodle> {
    // SAFETY: the library shipped with that game.
    Ok(unsafe { Oodle::load(path, version) }?)
}
