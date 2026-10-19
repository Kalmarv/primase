//! `prepare-web-textures` decodes a texture's stored level directly (`primase_image::texture_level_rgba8`) where the
//! web writer used to decode the PNG the glTF writer made of it: on every texture of a real scene
//! (`PRIMASE_FROMSOFT_SCENE`, slice A's skate scene), the two give the same RGBA bytes.
use primase_core::{Texture, World};
use primase_testdata::real_data;

#[test]
fn direct_decoding_is_the_png_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let Some(folder) = real_data("PRIMASE_FROMSOFT_SCENE") else {
        return Ok(());
    };
    let mut world = World::new();
    primase_reskate::read::interchange::read(&folder, &mut world)?;
    primase_image::load_textures(&mut world, &folder)?;
    let (mut textures, mut pixels) = (0, 0u64);
    for (e, t) in world.iter::<Texture>() {
        let direct = primase_image::texture_level_rgba8(t, 0)?;
        let png = primase_image::read_png(&primase_image::texture_level_png(t, 0)?)?.image;
        assert_eq!((direct.width, direct.height), (png.width, png.height), "{e:?}");
        let (primase_image::Samples::U8(a), primase_image::Samples::U8(b)) = (direct.samples, png.samples)
        else {
            return Err(format!("{e:?}: not RGBA8").into());
        };
        assert!(a == b, "{e:?}: direct and PNG round-trip pixels differ");
        textures += 1;
        pixels += u64::from(t.width) * u64::from(t.height);
    }
    assert!(textures > 0, "the scene has no texture");
    println!("{textures} textures, {pixels} pixels: direct decoding equals the PNG round trip, bit for bit");
    Ok(())
}
