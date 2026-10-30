//! What PEAK's scripts hand every shader, read from the game (`installs.peak/PEAK_Data`, with `tools.unity_tpk`):
//! level4's `FogTexture`, `_LightMap` and the light volume's values. Facts measured independently (UnityPy over the
//! same player, its MonoBehaviours' bytes decoded by the scripts' declarations and consumed to the last byte): level4
//! stores one `Peak.FogShaderParams` (path id 621203), whose `fogTexture` is the 1024 x 1024 Texture2D `A_Texture 60`,
//! and one `LightVolume` (path id 576878): brightness 3, ambienceStrength 1.25, ambienceMin 0.15, raySpacing 5, its
//! light map 132 x 276 x 572 cells from (-325.631, -127.345, -474.081), 572 BC6H and 572 BC4 slices. The volume's
//! bytes are the engine's own: Unity's editor running `Decompress`'s steps on those slices as stored
//! (`oracles.unity-editor/peak/light-volume-level4`).

#![allow(clippy::unwrap_used)]

mod inputs;

use inputs::{Game, OracleSet, Tool, install_file, oracle, tool};
use primase_core::{Component, Name, TexFormat, Texture, TextureLayers, TextureShape, World};
use primase_peak::read::{FOG_TEXTURE, LIGHT_MAP};
use primase_peak::{GlobalsReport, read_globals};
use primase_unity::components::UnityShaderGlobals;
use unity_formats::TypeTreePackage;
use unity_formats::player::Player;

#[test]
fn level4_s_globals_are_its_scripts_fields() {
    let (Some(data), Some(tpk)) = (install_file(Game::Peak, "PEAK_Data"), tool(Tool::UnityTpk)) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    let report = read_globals(&player, "level4", &mut w).unwrap();
    assert_eq!(
        report,
        GlobalsReport {
            fog_shader_params: 1,
            fog_shader_params_off: 0,
            light_volumes: 1,
            light_volumes_off: 0,
        }
    );
    let globals = w.resource::<UnityShaderGlobals>().unwrap().clone();
    assert!(globals.unset.is_empty(), "{:?}", globals.unset);

    let fog = &globals.textures[FOG_TEXTURE];
    assert_eq!(fog.set_by, "Peak.FogShaderParams.fogTexture of level4/621203");
    assert_eq!(w.get::<Name>(fog.texture).unwrap().0, "A_Texture 60");
    let t: &Texture = w.get(fog.texture).unwrap();
    assert_eq!((t.width, t.height), (1024, 1024));

    let floats = |name: &str| -> (Vec<f32>, String) {
        let g = &globals.values[name];
        (
            g.words.iter().map(|&b| f32::from_bits(b)).collect(),
            g.set_by.clone(),
        )
    };
    let at = "of level4/576878";
    for (name, want, field) in [
        ("_Brightness", vec![3.0], "brightness"),
        ("_AmbienceStrength", vec![1.25], "ambienceStrength"),
        ("_AmbienceMin", vec![0.15], "ambienceMin"),
        ("_RaySpacing", vec![5.0], "raySpacing"),
        ("_GridRes", vec![132.0, 276.0, 572.0, 0.0], "lightMapData.gridRes"),
        (
            "_GridOffset",
            vec![-325.631, -127.345_46, -474.080_5, 0.0],
            "lightMapData.gridOffset",
        ),
    ] {
        assert_eq!(
            floats(name),
            (want, format!("LightVolume.{field} {at}")),
            "{name}"
        );
    }

    let light = &globals.textures[LIGHT_MAP];
    assert_eq!(light.set_by, format!("LightVolume.lightMapData {at}"));
    assert_eq!(w.get::<Name>(light.texture).unwrap().0, "LightVolumeBakeTexture");
    let volume: &TextureLayers = w.get(light.texture).unwrap();
    assert_eq!(
        (
            volume.shape,
            volume.width,
            volume.height,
            volume.format,
            volume.levels.len()
        ),
        (
            TextureShape::Volume { depth: 572 },
            132,
            276,
            TexFormat::Rgba16Float,
            1
        )
    );
    volume.verify(light.texture, &w).unwrap();
    if let Some(editor) = oracle(OracleSet::UnityEditor, Game::Peak, "light-volume-level4") {
        let want = std::fs::read(editor.join("volume_rgba16f.bin")).unwrap();
        let slice = 132 * 276 * 8;
        assert_eq!(want.len(), slice * 572);
        let differing: Vec<usize> = volume.levels[0]
            .iter()
            .zip(want.chunks(slice))
            .enumerate()
            .filter(|(_, (ours, theirs))| ours.as_slice() != *theirs)
            .map(|(z, _)| z)
            .collect();
        assert!(
            differing.is_empty(),
            "{} of 572 slices differ from the editor's RGBAHalf bytes, the first {:?}",
            differing.len(),
            &differing[..differing.len().min(8)]
        );
    }

    // a second read finds both textures already in the world: one entity stands for each
    let (fog_entity, light_entity) = (fog.texture, light.texture);
    read_globals(&player, "level4", &mut w).unwrap();
    let again = w.resource::<UnityShaderGlobals>().unwrap();
    assert_eq!(
        (
            again.textures[FOG_TEXTURE].texture,
            again.textures[LIGHT_MAP].texture
        ),
        (fog_entity, light_entity)
    );
    assert_eq!(w.iter::<Texture>().count(), 1);
    assert_eq!(w.iter::<TextureLayers>().count(), 1);
}
