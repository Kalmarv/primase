//! What PEAK's scripts hand every shader, read from the game (`installs.peak/PEAK_Data`, with `tools.unity_tpk`):
//! level4's `FogTexture`. Facts measured independently (UnityPy over the same player, its MonoBehaviours' bytes
//! decoded by the script's declaration and consumed to the last byte): level4 stores one `Peak.FogShaderParams`
//! (path id 621203), whose `fogTexture` is the 1024 x 1024 Texture2D `A_Texture 60`.

#![allow(clippy::unwrap_used)]

mod inputs;

use inputs::{Game, Tool, install_file, tool};
use primase_core::{Name, Texture, World};
use primase_peak::read::FOG_TEXTURE;
use primase_peak::{GlobalsReport, read_globals};
use primase_unity::components::UnityShaderGlobals;
use unity_formats::TypeTreePackage;
use unity_formats::player::Player;

#[test]
fn level4_s_fog_texture_is_its_fog_shader_params_texture() {
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
            fog_shader_params_off: 0
        }
    );
    let globals = w.resource::<UnityShaderGlobals>().unwrap().clone();
    assert!(globals.unset.is_empty(), "{:?}", globals.unset);
    let fog = &globals.textures[FOG_TEXTURE];
    assert_eq!(fog.set_by, "Peak.FogShaderParams.fogTexture of level4/621203");
    assert_eq!(w.get::<Name>(fog.texture).unwrap().0, "A_Texture 60");
    let t: &Texture = w.get(fog.texture).unwrap();
    assert_eq!((t.width, t.height), (1024, 1024));
    // a second read finds the texture already in the world: one entity stands for it
    read_globals(&player, "level4", &mut w).unwrap();
    assert_eq!(
        w.resource::<UnityShaderGlobals>().unwrap().textures[FOG_TEXTURE].texture,
        fog.texture
    );
    assert_eq!(w.iter::<Texture>().count(), 1);
}
