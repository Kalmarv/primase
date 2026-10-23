#![allow(clippy::unwrap_used)]
//! A shader of an install by name (read::shader_source): the container of MAD_Tex1_TerrainBlend's pixel permutation
//! at body offset 0x081AB2 of `DataPC_MAD_USNAT.forge` entry 61916 (the template the shader oracle reads, there by
//! offset through `template::shaders`) is the one its name returns, its key lists, and it parses as a pixel program.
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::shader_source::{ShaderName, ShaderSource};
use primase_anvil::read::template::{self, Stage};
use primase_testdata::{Game, install};

#[test]
fn a_shader_by_name_is_the_permutations_container() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let forge = "DataPC_MAD_USNAT.forge";
    let mut archive = Archive::open(&[dir.join(forge)]).unwrap();
    let body = archive
        .resource(At {
            forge: 0,
            entry: 61_916,
        })
        .unwrap()
        .body;
    let read = template::permutations(&body).unwrap();
    let wanted = read
        .iter()
        .find(|p| p.at == 0x08_1AB2)
        .expect("the permutation at 0x081AB2");
    assert_eq!(wanted.stage, Stage::Pixel);

    let mut source = ShaderSource::open(&dir).unwrap();
    assert!(source.forges().iter().any(|f| f == forge));
    let name = ShaderName {
        forge: forge.into(),
        template: 61_916,
        stage: Stage::Pixel,
        key: wanted.key,
    };
    let container = source.named(&name).unwrap();
    assert_eq!(container, wanted.container);
    assert_eq!(
        source
            .named(&ShaderName::parse(&name.to_string()).unwrap())
            .unwrap(),
        container
    );
    assert!(
        source
            .keys(forge, 61_916, Stage::Pixel)
            .unwrap()
            .contains(&wanted.key)
    );
    assert!(primase_dxbc::lift(&container).is_ok());
    // an entry that is no template, and a key the template lacks, are refused by reason
    let not_template = source
        .container(forge, 0, Stage::Pixel, 0)
        .unwrap_err()
        .to_string();
    assert!(
        not_template.contains("is not a MaterialTemplate"),
        "{not_template}"
    );
    let missing = source
        .container(forge, 61_916, Stage::Vertex, 0xffff_ffff)
        .unwrap_err()
        .to_string();
    assert!(
        missing.contains("has no vs permutation with key ffffffff"),
        "{missing}"
    );
    assert!(
        source
            .templates(forge)
            .unwrap()
            .iter()
            .any(|(entry, name)| *entry == 61_916 && !name.is_empty())
    );
}
