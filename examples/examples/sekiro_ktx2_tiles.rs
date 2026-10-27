//! Slice A of Sekiro's Ashina Castle (m11_01) as web tiles, twice from one conversion: a KTX2 set in `<out>` and its plain
//! twin (PNG, uncompressed, not instanced) in `<out>/plain`, both written from the same prepared cell worlds
//! (`primase_web::convert::tiles`), so the plain set holds the very pixels the KTX2 encoder was given. It makes
//! `oracles.baseline.sekiro/ktx2-tiles`, which `formats/web/tests/ktx2_audit.rs` decodes with the Basis Universal
//! transcoder and compares with the plain set.
//!
//! `cargo run --release -p primase-examples --features ktx2 --example sekiro_ktx2_tiles -- <Sekiro install>
//! <oo2core_6_win64.dll> <out>`
use std::path::{Path, PathBuf};

use primase_core::convert::{Result, convert_parts};
use primase_core::pass::{PassManager, PassRegistry, Pipeline};
use primase_core::{LodChoice, SelectLod};
use primase_foundations::compress::oodle::{Oodle, Version};
use primase_fromsoft::dcx::Decoder;
use primase_fromsoft::materials::SkipClass;
use primase_fromsoft::rigid::ApplyRigidSkinning;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::convert::AreaReader;
use primase_fromsoft::sekiro::passes::{
    BakeFromSoftLayers, Cards, Degenerates, DropFromSoftFar, LowerFromSoftGeometry, LowerFromSoftMaterials, Pose,
    ResolveFromSoftShaders, Tiling, Unbound, Winding,
};
use primase_fromsoft::sekiro::settings::{PixelCap, Seconds};
use primase_web::{Options, TileSize};

fn main() -> Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [install, dll, out] = args.as_slice() else {
        return Err("usage: sekiro_ktx2_tiles <Sekiro install> <oo2core_6_win64.dll> <out>".into());
    };
    // slice A: the part of m11_01 within [west, south, east, north] metres, and placements reaching 30 m into it
    let region = [-170.0, 180.0, -50.0, 300.0];
    let oodle = load_oodle(dll)?;
    let decoder = Decoder { oodle: Some(&oodle), max_output: 1 << 30 };
    let options = sekiro::Options { region: Some(sekiro::Region { rect: region, margin: 30.0 }), ..Default::default() };
    let mut reader = AreaReader::new(install, "m11_01", decoder, options);

    // the two sets: KTX2, and its plain twin; one plan (the same cells), so the same prepared worlds reach both
    let mut ktx2 = Options::new(TileSize::new(200.0)?);
    ktx2.ktx2 = true;
    ktx2.tile_byte_budget = Some(96 << 20);
    let mut plain = Options::new(TileSize::new(200.0)?);
    plain.compress = false;
    plain.gpu_instancing = false;
    plain.tile_byte_budget = Some(96 << 20);
    let (mut parts, writer) = primase_web::convert::tiles(vec![(ktx2, out.clone()), (plain, out.join("plain"))])?;

    // run once on the area as read: Sekiro's own data lowered to core meanings
    let skip = [SkipClass::Sky, SkipClass::LightVolume, SkipClass::Shadow].into();
    #[rustfmt::skip]
    let lower = Pipeline {
        passes: vec![
            Box::new(DropFromSoftFar),
            Box::new(ApplyRigidSkinning),
            Box::new(LowerFromSoftGeometry { pose: Pose::Rigid, uv_tiling: Tiling::Apply, winding: Winding::Normals,
                degenerates: Degenerates::Drop, leave_out: skip, blocker_cards: Cards::Record }),
            Box::new(SelectLod { choice: LodChoice::Finest }),
            Box::new(ResolveFromSoftShaders::new(Unbound::Preview)),
            Box::new(BakeFromSoftLayers::new(PixelCap::new(512)?, Seconds::new(0.0)?)),
            // water without a base texture: the skate look's flat blue-grey (the destination's choice)
            Box::new(LowerFromSoftMaterials::new(None, primase_image::Srgba8([48, 98, 122, 190]))),
        ],
    };
    // run on each cell before it is written: the glTF materials and the web textures both sets carry
    let registry = registry();
    let prepare = registry.parse(
        "normalize-normals, fill-missing-texcoords, \
         prepare-gltf-materials{base-color=0.8/0.8/0.8/1,metallic=0,roughness=0.85}, \
         prepare-web-textures{color=512,normal=512,mask=256,memo-mib=512}",
    )?;
    let report =
        convert_parts(&mut reader, &mut parts, Box::new(writer), &PassManager::new(registry), &lower, &prepare)?;
    for notice in report.notices() {
        println!("{notice}");
    }
    Ok(())
}

/// Every crate's passes the prepare pipeline names.
fn registry() -> PassRegistry {
    let mut r = PassRegistry::new();
    primase_core::register(&mut r);
    primase_image::register(&mut r);
    primase_gltf::register(&mut r);
    primase_web::prepare::register(&mut r);
    r
}

/// The game's Oodle library (version 6), which decompresses its archives.
#[allow(unsafe_code)]
fn load_oodle(path: &Path) -> Result<Oodle> {
    // SAFETY: the library shipped with the game, named by the caller.
    Ok(unsafe { Oodle::load(path, Version::V6) }?)
}
