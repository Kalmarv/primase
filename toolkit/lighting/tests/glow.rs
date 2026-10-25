//! `scale-emission` on hand-built worlds: which primitives' emission is scaled, by which factor, and which materials
//! are copied for it.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pass::{PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, Mesh, Name, Primitive, Semantic, Stream, Topology, World};
use primase_lighting::{SCALE_EMISSION, ScaleEmission};

fn material(w: &mut World, name: &str, strength: f32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        PbrMaterial {
            emissive: [1.0, 1.0, 1.0],
            emissive_strength: strength,
            ..PbrMaterial::default()
        },
    );
    w.insert(e, Name(name.to_owned()));
    e
}

/// A mesh named `name` of one triangle per material in `materials`.
fn mesh(w: &mut World, name: &str, materials: &[Entity]) -> Entity {
    let prims = materials
        .iter()
        .map(|m| Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            )]),
            indices: Some(vec![0, 1, 2]),
            material: Some(*m),
        })
        .collect();
    let e = w.spawn();
    w.insert(e, Mesh::new(prims));
    w.insert(e, Name(name.to_owned()));
    e
}

fn run(w: &mut World, pass: ScaleEmission) -> Result<PassLog, String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(pass)],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report.steps.remove(0).log)
}

/// What primitive `i` of `mesh` draws with: its material's name and emissive strength.
fn drawn(w: &World, mesh: Entity, i: usize) -> (String, f32) {
    let m = w.get::<Mesh>(mesh).unwrap().primitives()[i].material.unwrap();
    (
        w.get::<Name>(m).unwrap().0.clone(),
        w.get::<PbrMaterial>(m).unwrap().emissive_strength,
    )
}

fn list(items: &[(&str, f64)]) -> BTreeMap<String, f64> {
    items.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

/// The lamp material (strength 16) on a lamp, a bollard and a sign; the lantern material (8) on a lantern. Factors:
/// the lamp material 1/64 (materials), the bollard 1/8 (prototypes); the sign has no prefix of its own, so it takes
/// the material's 1/64 with the lamp; the lantern's material 1/2. The lamp material is drawn at two factors and none
/// at 1: the most common (1/64, lamp and sign) scales it in place, a copy `lamp x0.125` at 2 carries the bollard. The
/// lantern's is scaled in place. A plain material (no emission) is left as it is; unused prefixes are counted.
#[test]
fn each_primitive_draws_at_its_factor_and_shared_materials_are_copied() {
    let mut w = World::new();
    let lamp_m = material(&mut w, "lamp", 16.0);
    let lantern_m = material(&mut w, "lantern", 8.0);
    let plain = w.spawn();
    w.insert(plain, PbrMaterial::default());
    let lamp = mesh(&mut w, "Pole_Long_A", &[lamp_m, plain]);
    let bollard = mesh(&mut w, "Bollard_A", &[lamp_m]);
    let sign = mesh(&mut w, "Sign_A", &[lamp_m]);
    let lantern = mesh(&mut w, "Lantern_B", &[lantern_m]);
    let pass = ScaleEmission::new(
        list(&[("Bollard_", 0.125), ("Nothing_", 3.0)]),
        list(&[("lamp", 1.0 / 64.0), ("lantern", 0.5)]),
    )
    .unwrap();
    let log = run(&mut w, pass).unwrap();
    assert_eq!(drawn(&w, lamp, 0), ("lamp".into(), 0.25));
    assert_eq!(drawn(&w, sign, 0), ("lamp".into(), 0.25));
    assert_eq!(drawn(&w, bollard, 0), ("lamp x0.125".into(), 2.0));
    assert_eq!(drawn(&w, lantern, 0), ("lantern".into(), 4.0));
    assert_eq!(w.get::<PbrMaterial>(plain).unwrap(), &PbrMaterial::default());
    assert_eq!(log.counts["emissive materials copied for another factor"], 1);
    assert_eq!(log.counts["emissive materials scaled in place"], 2);
    assert_eq!(log.counts["primitives drawn with a scaled emission"], 4);
    assert_eq!(
        log.counts["prototypes prefixes that no mesh's name starts with"],
        1
    );
    assert_eq!(
        log.counts["materials prefixes that no emissive material's name starts with"],
        0
    );
}

/// A material some primitive draws at factor 1 (no prefix names it) keeps its strength for that one; the scaled
/// primitive gets a copy.
#[test]
fn a_material_drawn_unscaled_somewhere_keeps_its_strength() {
    let mut w = World::new();
    let m = material(&mut w, "lamp", 16.0);
    let tube = mesh(&mut w, "Tube_A", &[m]);
    let other = mesh(&mut w, "Other_A", &[m]);
    let pass = ScaleEmission::new(list(&[("Tube_", 0.5)]), BTreeMap::new()).unwrap();
    run(&mut w, pass).unwrap();
    assert_eq!(drawn(&w, other, 0), ("lamp".into(), 16.0));
    assert_eq!(drawn(&w, tube, 0), ("lamp x0.5".into(), 8.0));
}

/// The options are required, round-trip, and refuse a bad factor or prefix.
#[test]
fn options_are_required_and_checked() {
    let make = |p: &str, m: &str| {
        let o = [("prototypes", p), ("materials", m)]
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        SCALE_EMISSION.instance(&o)
    };
    let made = make(
        "Bollard_:0.125;Tube_:0.0078125",
        "MAD_MAT_ATL_Emissive_ColorSpectrum:0.015625",
    )
    .unwrap();
    assert_eq!(made.options()["prototypes"], "Bollard_:0.125;Tube_:0.0078125");
    assert_eq!(
        made.options()["materials"],
        "MAD_MAT_ATL_Emissive_ColorSpectrum:0.015625"
    );
    assert_eq!(make("none", "none").unwrap().options()["materials"], "none");
    for (p, m) in [
        ("Bollard_", "none"),
        ("Bollard_:0", "none"),
        (":2", "none"),
        ("none", "lamp:-1"),
    ] {
        assert!(make(p, m).is_err(), "{p} {m}");
    }
    let o = [("prototypes".to_owned(), "none".to_owned())]
        .into_iter()
        .collect();
    assert!(SCALE_EMISSION.instance(&o).is_err());
}
