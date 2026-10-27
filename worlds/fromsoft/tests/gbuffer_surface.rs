//! Each game's G-buffer ([`Game::gbuffer`]) read as a surface (`iridia_interface::surface`), the one reading a
//! material bake and a viewer's composite both take: which target and lanes hold each term, and what a stored value
//! decodes to, by hand from the layout's own statements (the normal `n * 0.5 + 0.50048876`, the gloss the square of the
//! smoothness over 0.95, Sekiro's reflectance its square root).
#![allow(clippy::unwrap_used)]
use fromsoft_formats::Game;
use iridia_interface::{NormalEncoding, NormalSpace, ScalarEncoding};
use iridia_interface::{
    surface::Base, surface::Emission, surface::Place, surface::Roughness, surface::Surface, surface::Term,
};
use primase_fromsoft::game::Shading;

/// The game's G-buffer as a surface, and each output's target.
fn surface(game: Game) -> (Surface, Vec<u32>) {
    let layout = game.gbuffer();
    let surface = Surface::read(layout.iter().map(|(_, lanes, role)| (*lanes, role)));
    (surface, layout.iter().map(|(target, _, _)| *target).collect())
}

/// The target and lanes of a place.
fn at(targets: &[u32], place: Place) -> (u32, u8) {
    (targets[place.output], place.lanes)
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn sekiros_gbuffer_is_a_specular_workflow_surface() {
    let (s, targets) = surface(Game::Sekiro);
    assert!(s.refused.is_empty(), "{:?}", s.refused);
    let Some(Base::Albedo(albedo)) = s.base else {
        panic!("an albedo: {:?}", s.base)
    };
    assert_eq!(at(&targets, albedo), (2, 0b0111));
    let normal = s.normal.unwrap();
    assert_eq!(at(&targets, normal.place), (0, 0b0111));
    assert_eq!(
        (normal.encoding, normal.space),
        (
            NormalEncoding::XyzBiased {
                bias_bits: 0x3f00_2008
            },
            NormalSpace::World
        )
    );
    let Some(Roughness::Gloss(gloss)) = s.roughness else {
        panic!("a gloss: {:?}", s.roughness)
    };
    assert_eq!(at(&targets, gloss.place), (1, 0b0001));
    let reflectance = s.reflectance.unwrap();
    assert_eq!(at(&targets, reflectance.place), (3, 0b0111));
    let Some(Emission::Colour(emission)) = s.emission else {
        panic!("an emission colour: {:?}", s.emission)
    };
    assert_eq!(at(&targets, emission.place), (4, 0b0111));
    // no metallic, specular strength or occlusion: the reflectance is the F0
    assert_eq!(
        (s.metallic, s.specular, s.occlusion, s.ambient_occlusion),
        (None, None, None, None)
    );

    // decoded by hand: the up normal as the writer stores it, a gloss of 0.64 (smoothness 0.8 * 0.95), F0 0.04 stored
    // as 0.2, the emission as it is
    let bias = f32::from_bits(0x3f00_2008);
    let up = normal.decode([bias, 1.0 * 0.5 + bias, bias, 0.0]).unwrap();
    assert!(
        close(up[0], 0.0) && close(up[1], 1.0) && close(up[2], 0.0),
        "{up:?}"
    );
    assert!(close(s.roughness.unwrap().decode([0.64, 0.0, 0.0, 0.0]), 0.24));
    let f0 = reflectance.colour([0.2, 0.2, 0.2, 1.0]);
    assert!(f0.iter().all(|c| close(*c, 0.04)), "{f0:?}");
    assert_eq!(
        s.emission.unwrap().decode([0.5, 1.5, 0.0, 0.0], [1.0; 3]),
        [0.5, 1.5, 0.0]
    );
}

#[test]
fn elden_rings_gbuffer_is_a_metallic_workflow_surface() {
    let (s, targets) = surface(Game::EldenRing);
    assert!(s.refused.is_empty(), "{:?}", s.refused);
    let Some(Base::Albedo(albedo)) = s.base else {
        panic!("an albedo: {:?}", s.base)
    };
    assert_eq!(at(&targets, albedo), (2, 0b0111));
    assert_eq!(at(&targets, s.normal.unwrap().place), (0, 0b0111));
    let metallic: Term = s.metallic.unwrap();
    assert_eq!(at(&targets, metallic.place), (1, 0b0001));
    assert_eq!(metallic.encoding, ScalarEncoding::LINEAR);
    let Some(Roughness::Gloss(gloss)) = s.roughness else {
        panic!("a gloss: {:?}", s.roughness)
    };
    assert_eq!(at(&targets, gloss.place), (1, 0b0010));
    assert_eq!(gloss.encoding, ScalarEncoding::square_of(0.95));
    // the gloss in lane 1 of target 1
    assert!(close(s.roughness.unwrap().decode([0.3, 0.64, 0.0, 0.0]), 0.24));
    // the emission in target 4's rgb, as stored (the programs write the emissive map's radiance there)
    let Some(Emission::Colour(emission)) = s.emission else {
        panic!("an emission colour: {:?}", s.emission)
    };
    assert_eq!(at(&targets, emission.place), (4, 0b0111));
    assert_eq!(
        s.emission.unwrap().decode([0.5, 1.5, 0.0, 0.0], [1.0; 3]),
        [0.5, 1.5, 0.0]
    );
    assert_eq!((s.reflectance, s.specular, s.occlusion), (None, None, None));
}
