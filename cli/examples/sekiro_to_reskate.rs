//! An area of Sekiro read and written as a skate. map folder for ReSkate Studio, every choice a typed pass:
//!
//! `cargo run --release -p primase-cli --example sekiro_to_reskate -- <Sekiro install> <oo2core_6 DLL> <out root>`
//!
//! The README's "From a Sekiro area to a skate. map" quotes this file; keep the two the same. A complete program
//! adds passes of its own around these: ledges chained into grind rails, travel markers named for players, collision
//! added where the game has none under a rendered surface.
use std::path::Path;

use primase_compress::oodle::{Oodle, Version};
use primase_core::convert::{Result, WholeWorld, convert_parts};
use primase_core::pass::{PassManager, PassRegistry, Pipeline};
use primase_core::{LodChoice, SelectLod};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::materials::SkipClass;
use primase_fromsoft::rigid::ApplyRigidSkinning;
use primase_fromsoft::sekiro::collision::{ClassChoice, LowerFromSoftCollision};
use primase_fromsoft::sekiro::convert::AreaReader;
use primase_fromsoft::sekiro::extras::LowerFromSoftExtras;
use primase_fromsoft::sekiro::lights::LowerFromSoftLights;
use primase_fromsoft::sekiro::objects::{ConvexBodies, ConvexSurface, LowerFromSoftObjectCollision};
use primase_fromsoft::sekiro::passes::{
    BakeFromSoftLayers, Cards, Degenerates, DropFromSoftFar, LowerFromSoftGeometry, LowerFromSoftMaterials,
    Pose, Tiling, Winding,
};
use primase_fromsoft::sekiro::settings::{Factor, Fraction, HitClass, HitFilters, Metres, PixelCap, Seconds};
use primase_image::{CapMode, CapTextures, NormalDownscale, NormalizeNormals};
use primase_reskate::components::{CollisionSurfaces, CollisionWeld};
use primase_reskate::passes::{
    ClipReskateRegion, LightCalibration, OceanMaterials, PauseExtent, PauseSource, PlaceReskateSpawn,
    PlanReskateBlendBudget, PrepareReskateBlendOrder, PrepareReskateCollision, PrepareReskateFloor,
    PrepareReskateGrinds, PrepareReskateOverlays, PrepareReskatePauseMap, PrepareReskateTextures,
    PrepareReskateUvRepeats, PrepareReskateWater, RaiseOptions, RaiseSkate, RepeatBlended, RepeatDecals,
    SnapReskateMarkers, SummarizeReskateMap, WaterExtent,
};
use primase_reskate::{EncodingOptions, MapSink};

/// Ashina Castle.
const AREA: &str = "m11_01";
/// The part of it converted: `[west, south, east, north]`, metres east and north in the world frame.
const REGION: [f32; 4] = [-170.0, 180.0, -50.0, 300.0];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [install, oodle, out] = args.as_slice() else {
        return Err("usage: sekiro_to_reskate <Sekiro install> <oo2core_6 DLL> <out root>".into());
    };
    let oodle = load_oodle(Path::new(oodle))?;

    // Read: the area as the game stores it, placements reaching within 30 m of the region.
    let decoder = Decoder {
        oodle: Some(&oodle),
        max_output: 1 << 30,
    };
    let options = primase_fromsoft::sekiro::Options {
        region: Some(REGION),
        region_margin: 30.0,
        ..Default::default()
    };
    let mut reader = AreaReader::new(install, AREA, decoder, options);

    // Write: one ReSkate map folder, `<out root>/ashina`.
    let encoding = EncodingOptions {
        out_root: out.into(),
        name: Some("ashina".into()),
        max_render_edge: Some(48.0),
        ..EncodingOptions::default()
    };
    let select = select()?;
    let passes = prepare_reskate(&encoding)?;
    println!("select: {select}\npasses: {passes}");
    let writer = Box::new(MapSink::new(Path::new(""), &encoding));

    // A ReSkate map is laid out whole: the selected area is one part.
    let manager = PassManager::new(PassRegistry::new());
    let report = convert_parts(
        &mut reader,
        &mut WholeWorld::default(),
        writer,
        &manager,
        &select,
        &passes,
    )?;
    for notice in report.notices() {
        println!("{notice}");
    }
    Ok(())
}

/// Run once on the area as read: Sekiro's own data lowered to core meanings.
fn select() -> Result<Pipeline> {
    // Collision within 50 m of the region: the ground parts (hit filters 8 and 27), HitMtrl classes 22 and 36 as
    // water, blockers (29) and 40 left out; objects leave out both, and bodies of motion type 2.
    let reach = Metres::new(50.0)?;
    let water = [HitClass::new(22)?, HitClass::new(36)?];
    let left_out = [HitClass::new(29)?, HitClass::new(40)?];
    let mut collision = LowerFromSoftCollision::new(HitFilters::new([8, 27])?).with_reach(reach);
    for c in water {
        collision = collision.with_class(c, ClassChoice::Water)?;
    }
    for c in left_out {
        collision = collision.with_class(c, ClassChoice::LeaveOut)?;
    }
    let objects = LowerFromSoftObjectCollision::new(ConvexBodies::Tessellate)
        .with_leave_out(water.into_iter().chain(left_out).collect())
        .with_leave_out_motion([2].into())
        .with_convex_surface(ConvexSurface::Render)
        .with_reach(reach);
    let geometry = LowerFromSoftGeometry::new(Pose::Rigid)
        .with_uv_tiling(Tiling::Apply)
        .with_winding(Winding::Normals)
        .with_degenerates(Degenerates::Drop)
        .with_leave_out([SkipClass::Sky, SkipClass::LightVolume, SkipClass::Shadow].into())
        .with_blocker_cards(Cards::Record);
    let lights =
        LowerFromSoftLights::new(Factor::new(1.0)?, Fraction::new(0.15)?).with_min_range(Metres::new(0.5)?);
    Ok(Pipeline {
        passes: vec![
            Box::new(collision),
            Box::new(objects),
            Box::new(DropFromSoftFar),
            Box::new(ApplyRigidSkinning),
            Box::new(geometry),
            Box::new(SelectLod {
                choice: LodChoice::Finest,
            }),
            // layered materials baked to plain textures over the whole area, then every material lowered
            Box::new(BakeFromSoftLayers::new(PixelCap::new(512)?, Seconds::new(0.0)?)),
            Box::new(LowerFromSoftMaterials::new(None)),
            Box::new(lights),
            Box::new(LowerFromSoftExtras),
        ],
    })
}

/// Run on each part before it is written: the core world prepared as the ReSkate writer encodes it.
fn prepare_reskate(e: &EncodingOptions) -> Result<Pipeline> {
    let name = e.name.as_deref().unwrap_or("map");
    Ok(Pipeline {
        passes: vec![
            Box::new(NormalizeNormals),
            Box::new(RaiseSkate::new(RaiseOptions::new(
                name,
                LightCalibration::Standard,
            )?)),
            Box::new(ClipReskateRegion::new(REGION, 30.0)?),
            Box::new(PrepareReskateFloor::new(100.0, 250.0)?),
            Box::new(PrepareReskateGrinds::studio()),
            Box::new(PlaceReskateSpawn::centre()),
            Box::new(SnapReskateMarkers::for_encoding(e)?),
            Box::new(PrepareReskateWater::new(
                WaterExtent::Region { margin: 400.0 },
                OceanMaterials::Stated,
            )?),
            Box::new(PrepareReskatePauseMap::new(PauseSource::Geometry {
                height: 2160,
                extent: PauseExtent::Region,
            })?),
            Box::new(CapTextures::new(256)?.with_mode(CapMode {
                normals: NormalDownscale::Box,
                ..CapMode::default()
            })),
            Box::new(PrepareReskateTextures),
            Box::new(PrepareReskateUvRepeats::new(
                RepeatDecals::Omit,
                RepeatBlended::Cutout,
            )),
            Box::new(PrepareReskateOverlays::new(0.004)?.for_encoding(e)?),
            Box::new(PrepareReskateBlendOrder::new(200.0)?.for_encoding(e)?),
            Box::new(PrepareReskateCollision::new(
                CollisionSurfaces::Coalesce,
                CollisionWeld::Millimetre,
                e,
            )?),
            Box::new(SummarizeReskateMap::for_encoding(e)?),
            Box::new(PlanReskateBlendBudget::pool()),
        ],
    })
}

/// The game's Oodle library (version 6), which decompresses its archives.
#[allow(unsafe_code)]
fn load_oodle(path: &Path) -> Result<Oodle> {
    // SAFETY: the caller names the trusted library shipped with the game.
    Ok(unsafe { Oodle::load(path, Version::V6) }?)
}
