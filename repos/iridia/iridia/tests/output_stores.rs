//! What a pixel program stores into its colour targets ([`iridia::output_stores`]), on
//! `fixtures/output_stores.hlsl` lifted by the DXIL lifter: each target lane's stores are found through the entry
//! point's result, each stored value's make is told apart (biased, a square root, the constant 0, a floored byte),
//! a target the program does not declare has no stores, and an output's constant rows are those its stores reach.
use iridia::Label;
use iridia::output_stores::OutputStores;

#[test]
fn a_pixel_programs_output_stores_and_their_makes() {
    let program = iridia_dxil::lift(include_bytes!("fixtures/output_stores.dxil")).unwrap();
    let stores = OutputStores::of(&program).unwrap();
    let every =
        |target: u32, lane: u32, test: &dyn Fn(&OutputStores<'_>, naga::Handle<naga::Expression>) -> bool| {
            let values = stores.lane(target, lane);
            !values.is_empty() && values.iter().all(|v| test(&stores, *v))
        };
    for lane in 0..3 {
        assert!(
            every(0, lane, &|s, v| s.is_biased(v)),
            "target 0 lane {lane} biased"
        );
        assert!(
            every(2, lane, &|s, v| s.is_floored_byte(v)),
            "target 2 lane {lane} floored"
        );
        assert!(!every(0, lane, &|s, v| s.is_sqrt(v)
            || s.is_zero(v)
            || s.is_floored_byte(v)));
    }
    assert!(every(1, 0, &|s, v| s.is_sqrt(v)));
    assert!(every(1, 1, &|s, v| s.is_zero(v)));
    assert!(!every(1, 0, &|s, v| s.is_biased(v) || s.is_zero(v)));
    assert!(stores.lane(3, 0).is_empty(), "no target 3");
    let target = |n: u32| Label {
        name: "SV_Target".into(),
        index: n,
    };
    let rows = stores.constant_rows(&target(1)).unwrap();
    // target 1 is made of the material's rows 0 (tint.x) and 1 (gloss.x); target 2 of no constant
    assert_eq!(
        rows.values().flatten().copied().collect::<Vec<_>>(),
        vec![0, 1],
        "{rows:?}"
    );
    assert!(stores.constant_rows(&target(2)).unwrap().is_empty());
}
