//! prepare-reskate-uv-transforms and prepare-reskate-wrap, run by the pass manager (declarations, conservation and the
//! verifier checked): a material's slots made to sample as one, as skate samples them.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::Conservation;
use primase_core::pass::{Options, Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{PbrMaterial, TextureRef, TextureSlot, UvTransform, Wrap};
use primase_core::{
    Entity, Instance, MaterialVariants, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World,
};
use primase_meshops::{SpecializeMaterialVariants, UnusedVariants};
use primase_reskate::passes::{PrepareReskateUvTransforms, PrepareReskateWrap, SamplingConflicts};

fn run(w: &mut World, passes: Vec<Box<dyn Pass>>) -> Result<PassLog, String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_conservation(Conservation::Enforce)
        .run(w, &Pipeline { passes }, &mut report)
        .map_err(|e| e.to_string())?;
    Ok(report.steps.last().unwrap().log.clone())
}

fn uv(anchor: TextureSlot, conflicts: SamplingConflicts) -> Box<dyn Pass> {
    Box::new(PrepareReskateUvTransforms::new(anchor, conflicts))
}

fn scaled(tex: Entity, s: f32) -> TextureRef {
    let mut r = TextureRef::new(tex);
    r.transform = Some(UvTransform {
        scale: [s; 2],
        ..UvTransform::IDENTITY
    });
    r
}

/// A material `name` with the base colour and normal slots given.
fn material(w: &mut World, name: &str, base: Option<TextureRef>, normal: Option<TextureRef>) -> Entity {
    let e = w.spawn();
    w.insert(e, Name(name.into()));
    w.insert(
        e,
        PbrMaterial {
            base_color_texture: base,
            normal_texture: normal,
            ..PbrMaterial::default()
        },
    );
    e
}

/// A placed one-triangle prototype drawn with `material`, its coordinates (0,0) (1,0) (0,1).
fn prototype(w: &mut World, material: Entity) -> Entity {
    let p = w.spawn();
    w.insert(
        p,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
                ),
                (
                    Semantic::TexCoord(0),
                    Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
                ),
            ]),
            indices: None,
            material: Some(material),
        }]),
    );
    place(w, p, 0);
    p
}

fn place(w: &mut World, prototype: Entity, variant: u32) -> Entity {
    let e = w.spawn();
    w.insert(e, Instance { prototype, variant });
    w.insert(e, Transform::IDENTITY);
    e
}

fn coords(w: &World, p: Entity) -> Vec<[f32; 2]> {
    match w.get::<Mesh>(p).unwrap().primitives()[0]
        .streams
        .get(&Semantic::TexCoord(0))
    {
        Some(Stream::F32x2(v)) => v.clone(),
        _ => panic!("no coordinates"),
    }
}

fn slots(w: &World, m: Entity) -> Vec<TextureRef> {
    w.get::<PbrMaterial>(m).unwrap().textures().collect()
}

/// Adopted: the normal slot takes the base colour's tiling, reported by material and slot; the tiling is baked into the
/// coordinates and no slot keeps a transform (an identity one included, which raise-skate would refuse).
#[test]
fn slots_adopt_the_anchors_tiling_and_it_is_baked() {
    let mut w = World::new();
    let tex = w.spawn();
    let brick = material(&mut w, "brick", Some(scaled(tex, 4.0)), Some(scaled(tex, 2.0)));
    let p = prototype(&mut w, brick);
    let mut plain = TextureRef::new(tex);
    plain.transform = Some(UvTransform::IDENTITY);
    let floor = material(&mut w, "floor", Some(plain), None);
    let q = prototype(&mut w, floor);
    let log = run(&mut w, vec![uv(TextureSlot::BaseColor, SamplingConflicts::Adopt)]).unwrap();
    assert_eq!(
        log.approximations["texture tiling replaced by the base colour's (one coordinate set): brick normal"],
        1
    );
    assert_eq!(log.counts["texture slots given their anchor's tiling"], 1);
    assert_eq!(coords(&w, p), [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]]);
    assert_eq!(coords(&w, q), [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
    for m in [brick, floor] {
        assert!(slots(&w, m).iter().all(|r| r.transform.is_none()), "{m:?}");
    }
}

/// The anchor is the program's: anchored on the normal, the base colour takes the normal's tiling; a material
/// without a texture in the anchor slot follows its first textured slot.
#[test]
fn the_anchor_is_chosen_and_falls_back_to_the_first_slot() {
    let mut w = World::new();
    let tex = w.spawn();
    let brick = material(&mut w, "brick", Some(scaled(tex, 4.0)), Some(scaled(tex, 2.0)));
    let p = prototype(&mut w, brick);
    let bare = material(&mut w, "bare", Some(scaled(tex, 3.0)), None);
    let q = prototype(&mut w, bare);
    let log = run(&mut w, vec![uv(TextureSlot::Normal, SamplingConflicts::Adopt)]).unwrap();
    assert_eq!(coords(&w, p)[1], [2.0, 0.0]);
    assert_eq!(coords(&w, q)[1], [3.0, 0.0]);
    let tilings: Vec<&String> = log
        .approximations
        .keys()
        .filter(|k| k.starts_with("texture tiling"))
        .collect();
    assert_eq!(
        tilings,
        ["texture tiling replaced by the normal's (one coordinate set): brick base colour"]
    );
}

/// Refused: a slot tiling otherwise than its anchor (conflicts=refuse), and a slot sampling another coordinate set.
#[test]
fn conflicts_refused_and_other_sets_are_errors() {
    let mut w = World::new();
    let tex = w.spawn();
    let brick = material(&mut w, "brick", Some(scaled(tex, 4.0)), Some(scaled(tex, 2.0)));
    prototype(&mut w, brick);
    let e = run(
        &mut w,
        vec![uv(TextureSlot::BaseColor, SamplingConflicts::Refuse)],
    )
    .unwrap_err();
    assert!(
        e.contains("brick's normal texture tiles otherwise than its base colour texture"),
        "{e}"
    );
    let mut w = World::new();
    let tex = w.spawn();
    let mut second = TextureRef::new(tex);
    second.texcoord = 1;
    let lit = material(&mut w, "lit", Some(TextureRef::new(tex)), Some(second));
    prototype(&mut w, lit);
    let e = run(&mut w, vec![uv(TextureSlot::BaseColor, SamplingConflicts::Adopt)]).unwrap_err();
    assert!(e.contains("lit's normal texture samples coordinate set 1"), "{e}");
}

/// A prototype whose variants tile differently cannot be baked: refused, the tiling never erased; with its forms
/// specialized first (the program's choice of what becomes of unused ones), each bakes its own.
#[test]
fn variants_tiling_differently_are_refused_until_specialized() {
    let build = || {
        let mut w = World::new();
        let tex = w.spawn();
        let fine = material(&mut w, "fine", Some(scaled(tex, 4.0)), None);
        let coarse = material(&mut w, "coarse", Some(scaled(tex, 2.0)), None);
        let p = prototype(&mut w, fine);
        w.insert(p, MaterialVariants(vec![BTreeMap::from([(1, coarse)])]));
        let b = place(&mut w, p, 1);
        (w, b, fine)
    };
    let (mut w, _, fine) = build();
    let e = run(&mut w, vec![uv(TextureSlot::BaseColor, SamplingConflicts::Adopt)]).unwrap_err();
    assert!(
        e.contains("could not be baked") && e.contains("specialize-material-variants"),
        "{e}"
    );
    assert!(slots(&w, fine)[0].transform.is_some(), "the tiling is kept");
    let (mut w, b, _) = build();
    run(
        &mut w,
        vec![
            Box::new(SpecializeMaterialVariants::new(UnusedVariants::Discard)),
            uv(TextureSlot::BaseColor, SamplingConflicts::Adopt),
        ],
    )
    .unwrap();
    let copy = w.get::<Instance>(b).unwrap().prototype;
    assert_eq!(coords(&w, copy)[1], [2.0, 0.0]);
}

/// Every slot wraps along both axes as the anchor wraps along the chosen axis; each material changed is reported by
/// name, one already uniform is not.
#[test]
fn slots_take_the_anchors_wrap_along_the_axis() {
    let mut w = World::new();
    let tex = w.spawn();
    let mut base = TextureRef::new(tex);
    base.wrap = [Wrap::Clamp, Wrap::Repeat];
    let mut normal = TextureRef::new(tex);
    normal.wrap = [Wrap::Mirror, Wrap::Mirror];
    let rail = material(&mut w, "rail", Some(base), Some(normal));
    let plain = material(
        &mut w,
        "plain",
        Some(TextureRef::new(tex)),
        Some(TextureRef::new(tex)),
    );
    let log = run(
        &mut w,
        vec![Box::new(
            PrepareReskateWrap::new(TextureSlot::BaseColor, 0).unwrap(),
        )],
    )
    .unwrap();
    assert!(slots(&w, rail).iter().all(|r| r.wrap == [Wrap::Clamp; 2]));
    assert!(slots(&w, plain).iter().all(|r| r.wrap == [Wrap::Repeat; 2]));
    assert_eq!(
        log.approximations["texture wrap made one (the base colour's along u): rail"],
        1
    );
    assert_eq!(log.approximations.len(), 1);
    assert_eq!(log.counts["texture slots given their anchor's wrap"], 2);
    // along v, the base colour repeats
    let mut w = World::new();
    let tex = w.spawn();
    let rail = material(&mut w, "rail", Some(base), Some(normal));
    run(
        &mut w,
        vec![Box::new(
            PrepareReskateWrap::new(TextureSlot::BaseColor, 1).unwrap(),
        )],
    )
    .unwrap();
    assert!(slots(&w, rail).iter().all(|r| r.wrap == [Wrap::Repeat; 2]));
    let _ = tex;
}

/// Every choice is required, and the options each prints make it again.
#[test]
fn the_choices_are_required_and_printed() {
    let o = |kv: &[(&str, &str)]| -> Options { kv.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect() };
    assert!(PrepareReskateUvTransforms::from_options(&o(&[("anchor", "base-colour")])).is_err());
    assert!(PrepareReskateUvTransforms::from_options(&o(&[("conflicts", "adopt")])).is_err());
    assert!(
        PrepareReskateUvTransforms::from_options(&o(&[("anchor", "albedo"), ("conflicts", "adopt")]))
            .is_err()
    );
    let p = PrepareReskateUvTransforms::from_options(&o(&[
        ("anchor", "specular-colour"),
        ("conflicts", "refuse"),
    ]))
    .unwrap();
    assert_eq!(p.anchor(), TextureSlot::SpecularColor);
    assert_eq!(PrepareReskateUvTransforms::from_options(&p.options()).unwrap(), p);
    assert!(PrepareReskateWrap::from_options(&o(&[("anchor", "normal")])).is_err());
    assert!(PrepareReskateWrap::from_options(&o(&[("anchor", "normal"), ("axis", "w")])).is_err());
    let p = PrepareReskateWrap::from_options(&o(&[("anchor", "normal"), ("axis", "v")])).unwrap();
    assert_eq!((p.anchor(), p.axis()), (TextureSlot::Normal, 1));
    assert_eq!(PrepareReskateWrap::from_options(&p.options()).unwrap(), p);
}
