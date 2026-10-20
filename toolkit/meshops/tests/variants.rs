//! specialize-material-variants, run by the pass manager (declarations, conservation and the verifier checked).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::ledger::Conservation;
use primase_core::pass::{Options, Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    Entity, Instance, Lods, MaterialVariants, Mesh, MeshFile, Name, Primitive, Semantic, Stream, Topology,
    Transform, World,
};
use primase_meshops::{BakeTextureTransforms, SpecializeMaterialVariants, UnusedVariants};

fn run(w: &mut World, passes: Vec<Box<dyn Pass>>) -> Result<RunReport, String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_conservation(Conservation::Enforce)
        .run(w, &Pipeline { passes }, &mut report)
        .map_err(|e| e.to_string())?;
    Ok(report)
}

fn specialize(w: &mut World, unused: UnusedVariants) -> PassLog {
    let report = run(w, vec![Box::new(SpecializeMaterialVariants::new(unused))]).unwrap();
    report.steps[0].log.clone()
}

/// A material sampling a texture with `scale` tiling (none: untransformed).
fn material(w: &mut World, name: &str, scale: Option<f32>) -> Entity {
    let tex = w.spawn();
    let e = w.spawn();
    w.insert(e, Name(name.into()));
    let mut base = TextureRef::new(tex);
    base.transform = scale.map(|s| UvTransform {
        scale: [s; 2],
        ..UvTransform::IDENTITY
    });
    w.insert(
        e,
        PbrMaterial {
            base_color_texture: Some(base),
            ..PbrMaterial::default()
        },
    );
    e
}

/// A one-triangle prototype "bench" drawn with `own`, with the variants `variants`.
fn prototype(w: &mut World, own: Entity, variants: &[(u32, Entity)]) -> Entity {
    let p = w.spawn();
    w.insert(p, Name("bench".into()));
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
            material: Some(own),
        }]),
    );
    w.insert(p, MaterialVariants(vec![variants.iter().copied().collect()]));
    p
}

fn place(w: &mut World, prototype: Entity, variant: u32) -> Entity {
    let e = w.spawn();
    w.insert(e, Instance { prototype, variant });
    w.insert(e, Transform::IDENTITY);
    e
}

/// The prototype a placement draws and its material; the placement chooses no variant.
fn drawn(w: &World, placement: Entity) -> (Entity, Option<Entity>) {
    let i = w.get::<Instance>(placement).unwrap();
    assert_eq!(i.variant, 0);
    (
        i.prototype,
        w.get::<Mesh>(i.prototype).unwrap().primitives()[0].material,
    )
}

/// Kept: the placements drawing the prototype's own materials stay on it; each other form used gets one copy drawing
/// it (two variants drawing the same materials share it), named and declared as made from the prototype, without its
/// mesh file; the prototype keeps its own materials and every alternative.
#[test]
fn each_form_used_gets_its_prototype_and_nothing_is_lost() {
    let mut w = World::new();
    let (own, moss, rust) = (
        material(&mut w, "stone", None),
        material(&mut w, "moss", None),
        material(&mut w, "rust", None),
    );
    let p = prototype(&mut w, own, &[(1, moss), (2, rust), (3, moss)]);
    w.insert(
        p,
        MeshFile {
            path: "bench.glb".into(),
            mesh: 0,
            materials: vec![Some(own)],
        },
    );
    let (a, b, c, d) = (
        place(&mut w, p, 0),
        place(&mut w, p, 1),
        place(&mut w, p, 3),
        place(&mut w, p, 7),
    );
    let log = specialize(&mut w, UnusedVariants::Keep);
    assert_eq!(drawn(&w, a), (p, Some(own)));
    // a variant no primitive lists draws the prototype's own materials
    assert_eq!(drawn(&w, d), (p, Some(own)));
    let (copy, m) = drawn(&w, b);
    assert_ne!(copy, p);
    assert_eq!(m, Some(moss));
    assert_eq!(
        drawn(&w, c).0,
        copy,
        "variants 1 and 3 draw the same materials: one copy"
    );
    assert_eq!(w.get::<Name>(copy).unwrap().0, "bench variant 1");
    assert!(!w.has::<MaterialVariants>(copy) && !w.has::<MeshFile>(copy));
    assert_eq!(log.placed_from.get(&copy), Some(&p));
    assert_eq!(w.get::<Mesh>(p).unwrap().primitives()[0].material, Some(own));
    assert_eq!(w.get::<MaterialVariants>(p).unwrap().0[0].len(), 3);
    assert!(
        w.has::<MeshFile>(p),
        "the prototype still draws its file's materials"
    );
    assert_eq!(log.counts["prototype copies drawing a material variant"], 1);
    assert_eq!(log.counts["placements given their variant's prototype"], 2);
    assert!(log.omitted.is_empty());
}

/// Discarded: every prototype's variants go; a prototype none of whose placements draws its own materials draws its
/// first used variant itself (none left behind unplaced); each prototype losing a form nobody places is counted, and
/// one whose every form is placed is not.
#[test]
fn discarded_forms_nobody_places_are_counted() {
    let mut w = World::new();
    let (own, moss, rust) = (
        material(&mut w, "stone", None),
        material(&mut w, "moss", None),
        material(&mut w, "rust", None),
    );
    let p = prototype(&mut w, own, &[(1, moss), (2, rust)]);
    let (b, c) = (place(&mut w, p, 1), place(&mut w, p, 2));
    let unplaced = prototype(&mut w, own, &[(1, moss)]);
    let whole = prototype(&mut w, own, &[(1, moss)]);
    let (e, f) = (place(&mut w, whole, 0), place(&mut w, whole, 1));
    let log = specialize(&mut w, UnusedVariants::Discard);
    assert_eq!(drawn(&w, b), (p, Some(moss)));
    assert_eq!(w.get::<Name>(p).unwrap().0, "bench variant 1");
    let (copy, m) = drawn(&w, c);
    assert_ne!(copy, p);
    assert_eq!(m, Some(rust));
    assert_eq!(drawn(&w, e), (whole, Some(own)));
    assert_eq!(drawn(&w, f).1, Some(moss));
    assert_eq!(w.iter::<MaterialVariants>().count(), 0);
    // p loses its own form (stone), the unplaced prototype every form; the one placed in both forms loses none
    assert_eq!(
        log.omitted["prototypes' material forms nobody places (unused=discard)"],
        2
    );
    assert!(w.is_alive(unplaced));
}

/// Why a destination with one transform per coordinate set needs it: a prototype whose variants tile differently
/// cannot be baked (its primitive serves both materials); its forms specialized and the variants discarded, each copy
/// bakes its own tiling.
#[test]
fn specialized_forms_bake_their_own_tiling() {
    let mut w = World::new();
    let (fine, coarse) = (
        material(&mut w, "fine", Some(4.0)),
        material(&mut w, "coarse", Some(2.0)),
    );
    let p = prototype(&mut w, fine, &[(1, coarse)]);
    let (a, b) = (place(&mut w, p, 0), place(&mut w, p, 1));
    let mut unbaked = w.clone();
    let report = run(&mut unbaked, vec![Box::new(BakeTextureTransforms)]).unwrap();
    assert_eq!(
        report.steps[0].log.counts["not baked: materials sample a set with different transforms"],
        2
    );
    run(
        &mut w,
        vec![
            Box::new(SpecializeMaterialVariants::new(UnusedVariants::Discard)),
            Box::new(BakeTextureTransforms),
        ],
    )
    .unwrap();
    let uv = |e: Entity| match w
        .get::<Mesh>(w.get::<Instance>(e).unwrap().prototype)
        .unwrap()
        .primitives()[0]
        .streams
        .get(&Semantic::TexCoord(0))
    {
        Some(Stream::F32x2(v)) => v[1],
        _ => panic!("no coordinates"),
    };
    assert_eq!((uv(a), uv(b)), ([4.0, 0.0], [2.0, 0.0]));
    for m in [fine, coarse] {
        assert!(
            w.get::<PbrMaterial>(m)
                .unwrap()
                .base_color_texture
                .unwrap()
                .transform
                .is_none()
        );
    }
}

/// A prototype with variants and levels of detail is refused, and nothing changes; so is a placed one whose variants
/// list another number of primitives than its mesh.
#[test]
fn levels_of_detail_and_mismatched_variants_are_refused() {
    let mut w = World::new();
    let (own, moss) = (material(&mut w, "stone", None), material(&mut w, "moss", None));
    let p = prototype(&mut w, own, &[(1, moss)]);
    let b = place(&mut w, p, 1);
    w.insert(
        p,
        Lods {
            detail: 0,
            forms: Vec::new(),
            default: None,
        },
    );
    let e = run(
        &mut w,
        vec![Box::new(SpecializeMaterialVariants::new(UnusedVariants::Keep))],
    )
    .unwrap_err();
    assert!(e.contains("levels of detail"), "{e}");
    assert_eq!(w.get::<Instance>(b).unwrap().prototype, p);
    w.remove::<Lods>(p);
    w.insert(p, MaterialVariants(vec![BTreeMap::new(), BTreeMap::new()]));
    let e = run(
        &mut w,
        vec![Box::new(SpecializeMaterialVariants::new(UnusedVariants::Keep))],
    )
    .unwrap_err();
    assert!(e.contains("list 2 primitives, its mesh 1"), "{e}");
}

/// A world without variants is left as it is.
#[test]
fn a_world_without_variants_is_unchanged() {
    let mut w = World::new();
    let own = material(&mut w, "stone", None);
    let p = prototype(&mut w, own, &[]);
    w.remove::<MaterialVariants>(p);
    let a = place(&mut w, p, 0);
    let log = specialize(&mut w, UnusedVariants::Discard);
    assert_eq!(drawn(&w, a), (p, Some(own)));
    assert!(log.counts.is_empty() && log.omitted.is_empty());
}

/// The choice has no default, and the options it prints make it again.
#[test]
fn the_choice_is_required_and_printed() {
    assert!(SpecializeMaterialVariants::from_options(&Options::new()).is_err());
    assert!(
        SpecializeMaterialVariants::from_options(&Options::from([("unused".into(), "some".into())])).is_err()
    );
    let p = SpecializeMaterialVariants::from_options(&Options::from([("unused".into(), "discard".into())]))
        .unwrap();
    assert_eq!(p.unused(), UnusedVariants::Discard);
    assert_eq!(SpecializeMaterialVariants::from_options(&p.options()).unwrap(), p);
}
