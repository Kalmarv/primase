//! The user's Elden Ring params key (`regulation.key` of `keys.elden`) is the key whose SHA-256 [`ParamKey`] states:
//! read with that digest it is accepted, with another refused, and without one read unchecked to the same key.
use fromsoft_formats::{Game, Keys};
use primase_fromsoft::game::ParamKey;

#[test]
fn the_users_regulation_key_has_the_stated_digest() {
    let Some(folder) = primase_testdata::keys(primase_testdata::Game::Elden) else {
        return;
    };
    let game = Game::EldenRing;
    let checked = Keys::from_folder(&folder, game, game.param_key_sha256()).unwrap();
    let key = *checked.regulation().expect("keys.elden supplies regulation.key");
    let unchecked = Keys::from_folder(&folder, game, None).unwrap();
    assert_eq!(unchecked.regulation(), Some(&key));
    let refused = Keys::from_folder(&folder, game, Some(&[0; 32]))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("not the key the game states (its SHA-256 differs)"),
        "{refused}"
    );
}

#[test]
fn only_elden_ring_states_a_params_key() {
    assert!(Game::Sekiro.param_key_sha256().is_none());
    assert!(Game::EldenRing.param_key_sha256().is_some());
}
