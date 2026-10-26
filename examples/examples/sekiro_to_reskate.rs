//! An area of Sekiro read, lowered, prepared and written as a skate. map folder, then compiled by ReSkate Studio. The
//! README's "From a Sekiro area to a skate. map" quotes this file; keep the two the same.
use std::path::Path;

use primase_compress::oodle::{Oodle, Version};
use primase_core::convert::{Result, convert};
use primase_core::pass::{PassManager, PassRegistry, Pipeline};
use primase_core::{LodChoice, SelectLod};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::materials::SkipClass;
use primase_fromsoft::rigid::ApplyRigidSkinning;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::collision::LowerFromSoftCollision;
use primase_fromsoft::sekiro::convert::AreaReader;
use primase_fromsoft::sekiro::extras::LowerFromSoftExtras;
use primase_fromsoft::sekiro::lights::LowerFromSoftLights;
use primase_fromsoft::sekiro::objects::{ConvexBodies, ConvexSurface, LowerFromSoftObjectCollision, ResolutionChoice};
use primase_fromsoft::sekiro::passes::{
    BakeFromSoftLayers, Cards, Degenerates, DropFromSoftFar, DropFromSoftTextureTransforms, LowerFromSoftGeometry,
    LowerFromSoftMaterials, Pose, ResolveFromSoftShaders, SpeedTreeFrame, Tiling, Unbound, Winding,
};
use primase_fromsoft::sekiro::settings::{Factor, Fraction, HitClass, HitFilters, Metres, PixelCap, Seconds};
use primase_image::{Beyond, CapMode, CapTextures, NormalDownscale, NormalizeNormals};
use primase_reskate::compile::{CompileChoices, Streaming, Studio, TimeOfDay, compile_map};
use primase_reskate::components::{CollisionSurfaces, CollisionWeld};
use primase_reskate::passes::{
    ClipReskateRegion, LightCalibration, OceanMaterials, PauseColour, PauseDrawing, PauseExtent, PauseLeaveOut,
    PauseSource, PlaceReskateSpawn, PlanReskateBlendBudget, PrepareReskateBlendOrder, PrepareReskateCollision,
    PrepareReskateFloor, PrepareReskateGrinds, PrepareReskateOverlays, PrepareReskatePauseMap, PrepareReskateTextures,
    PrepareReskateUvRepeats, PrepareReskateWater, RaiseOptions, RaiseSkate, RepeatBlended, RepeatDecals,
    SnapReskateMarkers, SummarizeReskateMap, Unclassified, WaterExtent,
};
use primase_reskate::{EncodingOptions, MapSink};

fn main() -> Result<()> {
    // Ashina Castle (m11_01), the part within [west, south, east, north] metres, and placements reaching 30 m into it
    let region = [-170.0, 180.0, -50.0, 300.0];
    let oodle = load_oodle(Path::new("C:/Games/Sekiro/oo2core_6_win64.dll"), Version::V6)?;
    let decoder = Decoder { oodle: Some(&oodle), max_output: 1 << 30 };
    let options = sekiro::Options { region: Some(sekiro::Region { rect: region, margin: 30.0 }), ..Default::default() };
    let mut reader = AreaReader::new("C:/Games/Sekiro", "m11_01", decoder, options);
    let e = EncodingOptions {
        out_root: "out".into(),
        name: Some("ashina".into()),
        max_render_edge: Some(48.0),
        ..EncodingOptions::default()
    };

    // Run once on the area as read: Sekiro's own data lowered to core meanings.
    let (reach, water, leave_out) = (Some(Metres::new(50.0)?), HitClass::set([22, 36])?, HitClass::set([29, 40])?);
    let hit_filters = HitFilters::new([8, 27])?;
    let skip = [SkipClass::Sky, SkipClass::LightVolume, SkipClass::Shadow].into();
    #[rustfmt::skip]
    let lower = Pipeline {
        passes: vec![
            Box::new(LowerFromSoftCollision { hit_filters, water: water.clone(), leave_out: leave_out.clone(), reach }),
            Box::new(LowerFromSoftObjectCollision { resolution: ResolutionChoice::High, convex: ConvexBodies::Tessellate, leave_out: &water | &leave_out,
                leave_out_motion: [2].into(), convex_surface: ConvexSurface::Render, reach }),
            Box::new(DropFromSoftFar),
            Box::new(ApplyRigidSkinning),
            Box::new(LowerFromSoftGeometry { pose: Pose::Rigid, uv_tiling: Tiling::Apply, winding: Winding::Normals,
                degenerates: Degenerates::Drop, leave_out: skip, blocker_cards: Cards::Record,
                speedtree_frame: SpeedTreeFrame::Program }),
            Box::new(SelectLod { choice: LodChoice::Finest }),
            Box::new(ResolveFromSoftShaders::new(Unbound::Preview)),
            Box::new(BakeFromSoftLayers::new(PixelCap::new(512)?, Seconds::new(0.0)?)),
            // water without a base texture: the skate look's flat blue-grey (the destination's choice)
            Box::new(LowerFromSoftMaterials::new(None, primase_image::Srgba8([48, 98, 122, 190]))),
            Box::new(LowerFromSoftLights::new(Factor::new(1.0)?, Fraction::new(0.15)?).with_min_range(Metres::new(0.5)?)),
            Box::new(LowerFromSoftExtras),
        ],
    };

    // Run on each part before it is written: the core world prepared as the ReSkate writer encodes it.
    let prepare = Pipeline {
        passes: vec![
            Box::new(NormalizeNormals),
            Box::new(DropFromSoftTextureTransforms::new()),
            Box::new(RaiseSkate::new(RaiseOptions::new("ashina", LightCalibration::Standard, Unclassified::LeaveOut)?)),
            Box::new(ClipReskateRegion::new(region, 30.0)?),
            Box::new(PrepareReskateFloor::new(100.0, 250.0)?),
            Box::new(PrepareReskateGrinds::studio()),
            Box::new(PlaceReskateSpawn::centre()),
            Box::new(SnapReskateMarkers::for_encoding(&e)?),
            Box::new(PrepareReskateWater::new(WaterExtent::Region { margin: 400.0 }, OceanMaterials::Stated)?),
            Box::new(PrepareReskatePauseMap::new(PauseSource::Geometry {
                height: 2160,
                extent: PauseExtent::Region,
                drawing: PauseDrawing { leave_out: PauseLeaveOut::Nothing, colour: PauseColour::Mean },
            })?),
            Box::new(CapTextures::new(256, CapMode { beyond: Beyond::Filter, normals: NormalDownscale::Box })?),
            Box::new(PrepareReskateTextures),
            Box::new(PrepareReskateUvRepeats::new(RepeatDecals::Omit, RepeatBlended::Cutout)),
            Box::new(PrepareReskateOverlays::new(0.004)?.for_encoding(&e)?),
            Box::new(PrepareReskateBlendOrder::new(200.0, primase_reskate::blend_order::ChainOrder::Tie)?.for_encoding(&e)?),
            Box::new(PrepareReskateCollision::new(CollisionSurfaces::Coalesce, CollisionWeld::Millimetre, &e)?),
            Box::new(SummarizeReskateMap::for_encoding(&e)?),
            Box::new(PlanReskateBlendBudget::pool()),
        ],
    };

    // Read, lower, prepare, write `out/ashina`; what was left out or approximated comes back as notices.
    let writer = Box::new(MapSink::new(Path::new(""), &e));
    let report = convert(&mut reader, writer, &PassManager::new(PassRegistry::new()), &lower, &prepare)?;
    for notice in report.notices() {
        println!("{notice}");
    }

    // ReSkate Studio compiles the map folder into a mod, then the post-compile steps finish it (blend stage, level
    // root, layering gate) against skate.'s data, read with skate.'s own Oodle library (version 9).
    let studio = Studio { cli: "C:/ReSkate/reskate_cli.exe".into(), game: "C:/Games/skate".into() };
    let skate_oodle = load_oodle(Path::new("C:/Games/skate/oo2core_9_win64.dll"), Version::V9)?;
    let choices = CompileChoices {
        streaming: Some(Streaming::Cells),
        lods: true,
        gi: true,
        time_of_day: Some(TimeOfDay::Noon),
        pause_map: None,
    };
    let compiled =
        compile_map(&studio, Path::new("out/ashina"), Path::new("out/staging"), "ashina", &choices, &skate_oodle)?;
    println!(
        "{} ({} warnings; layering gate {:.0} s, failing but not yet enforced: {:?})",
        compiled.patch.display(),
        compiled.warnings,
        compiled.finished.gate_seconds,
        compiled.finished.reported
    );
    Ok(())
}

/// A game's Oodle library, which decompresses its archives.
#[allow(unsafe_code)]
fn load_oodle(path: &Path, version: Version) -> Result<Oodle> {
    // SAFETY: the library shipped with that game.
    Ok(unsafe { Oodle::load(path, version) }?)
}
