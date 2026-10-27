//! The preview export of a wardrobe: one character whose outfits share a skeleton, a hat that is one primitive of the
//! head's mesh, a skin tone as a material variant, a sword on a socket. It is written by `PreviewParts` through a
//! conversion with every choice open; the manifest is checked against the GLB's own JSON (the nodes, meshes and
//! joints it points to), round-tripped, replayed as a recipe, and each GLB goes through the Khronos validator when
//! `tools.gltf_validator` is set.
#![allow(clippy::unwrap_used)]

mod common;
mod inputs;

use std::collections::BTreeSet;

use common::{rich_primitive, scratch};
use primase_core::convert::{
    NoticeKind, PartitionGroup, PartitionPlan, PlannedParts, Reader, Report, Result, convert_parts,
};
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::state::{
    Clause, Condition, Domain, Literal, Override, PrimitiveCondition, Scope, StateAxis, StateOverride,
};
use primase_core::{
    Asset, Attachment, Binding, BindingId, Deformation, Entity, Influences, Instance, Joint, JointId,
    MaterialVariants, Mesh, Name, Rest, RigInstance, Role, Skeleton, Skin, SkinInstance, Slot, Socket,
    SocketId, Sockets, Source, Transform, WeightRule, World,
};
use primase_gltf::preview::manifest::{self as m, Set};
use primase_gltf::{Manifest, PreviewParts, Recipe, WriteOptions};
use serde_json::Value;

/// A world held in memory: read whole, a part loaded as its roots' closure.
struct Held(World);

impl Reader for Held {
    fn read(&mut self) -> Result<(World, Report)> {
        let all = self.0.entities().collect();
        let mut world = self.0.subset(&all);
        // the whole world is one part, unless a pass in the selection plans another way
        if world.resource::<PartitionPlan>().is_none() {
            world.insert_resource(PartitionPlan {
                by: "whole world".into(),
                groups: vec![PartitionGroup {
                    key: "world".into(),
                    roots: world.unreferenced().into_iter().collect(),
                    context: BTreeSet::new(),
                    area: None,
                }],
            });
        }
        Ok((world, Vec::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Vec::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "pbr"]
    }
    fn prototype_box(&self, _: &World, _: Entity) -> Option<[[f64; 3]; 2]> {
        None
    }
}

fn translate(t: [f32; 3]) -> Transform {
    let mut m = Transform::IDENTITY;
    m.cols[3] = t;
    m
}

fn axis(w: &mut World, key: &str, class: &str, values: &[&str], default: Option<u32>) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        StateAxis {
            key: key.into(),
            domain: Domain::Enum(values.iter().map(|v| (*v).into()).collect()),
            default,
            scope: Scope::Global,
            class: class.into(),
            source: format!("wardrobe {key}"),
        },
    );
    w.insert(e, Name(format!("{key} label")));
    e
}

fn when(axis: Entity, values: u64) -> Condition {
    Condition(vec![Clause(vec![Literal { axis, values }])])
}

struct Wardrobe {
    w: World,
    root: Entity,
    outfit: Entity,
}

/// The wardrobe asset `chr/hero`. Axes: `outfit` (coat | jacket, no default), `hat` (off | on, default off), `skin`
/// (pale | dark, no default), `weapon` (off | on, no default).
fn wardrobe() -> Wardrobe {
    let mut w = World::new();
    let skeleton = w.spawn();
    w.insert(skeleton, Name("skeleton".into()));
    w.insert(
        skeleton,
        Skeleton {
            joints: vec![
                Joint {
                    id: JointId(0),
                    name: "root".into(),
                    parent: None,
                    rest: Rest::Affine(Transform::IDENTITY),
                },
                Joint {
                    id: JointId(1),
                    name: "hand".into(),
                    parent: Some(JointId(0)),
                    rest: Rest::Affine(translate([0.0, 1.0, 0.0])),
                },
            ],
        },
    );
    w.insert(
        skeleton,
        Sockets(vec![Socket {
            id: SocketId(0),
            name: "grip".into(),
            joint: JointId(1),
            offset: translate([0.0, 0.1, 0.0]),
        }]),
    );
    let (pale, dark) = (w.spawn(), w.spawn());
    w.insert(pale, PbrMaterial::default());
    w.insert(
        dark,
        PbrMaterial {
            base_color: [0.1, 0.1, 0.1, 1.0],
            ..PbrMaterial::default()
        },
    );
    // a mesh of `n` primitives, skinned wholly to joint 0 when `skinned`
    let part = |w: &mut World, name: &str, n: usize, material: Entity, skinned: bool| {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            Mesh::new((0..n).map(|i| rich_primitive(Some(material), i as f32)).collect()),
        );
        if skinned {
            w.insert(
                e,
                Skin {
                    influences: vec![Some(Influences::Whole { slot: 0 }); n],
                    weights: WeightRule::SumsToOne,
                    bindings: vec![Binding {
                        id: BindingId(0),
                        skeleton,
                        palettes: vec![
                            Some(vec![Slot {
                                joint: JointId(0),
                                inverse_bind: Transform::IDENTITY,
                            }]);
                            n
                        ],
                        deformation: Deformation::Linear,
                    }],
                },
            );
        }
        e
    };
    let body = part(&mut w, "body", 1, pale, true);
    let coat = part(&mut w, "coat", 1, pale, true);
    let jacket = part(&mut w, "jacket", 1, pale, true);
    let head = part(&mut w, "head", 2, pale, false);
    let sword = part(&mut w, "sword", 1, pale, false);
    let outfit = axis(&mut w, "outfit", "outfit", &["coat", "jacket"], None);
    let hat = axis(&mut w, "hat", "accessory", &["off", "on"], Some(0));
    let tone = axis(&mut w, "skin", "body", &["pale", "dark"], None);
    let weapon = axis(&mut w, "weapon", "accessory", &["off", "on"], None);
    w.insert(coat, when(outfit, 0b01));
    w.insert(jacket, when(outfit, 0b10));
    w.insert(sword, when(weapon, 0b10));
    w.insert(head, PrimitiveCondition(vec![None, Some(when(hat, 0b10))]));
    w.insert(
        head,
        MaterialVariants(vec![[(1, dark)].into(), [(1, dark)].into()]),
    );
    // the rig, and what is placed: skinned outfits, the head with a tone switch, the sword on the grip socket
    let rig = w.spawn();
    w.insert(rig, Name("rig".into()));
    w.insert(rig, RigInstance { skeleton });
    w.insert(rig, Transform::IDENTITY);
    let place = |w: &mut World, proto: Entity| {
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(p, Transform::IDENTITY);
        p
    };
    for proto in [body, coat, jacket] {
        let p = place(&mut w, proto);
        w.insert(
            p,
            SkinInstance {
                rig,
                binding: BindingId(0),
            },
        );
    }
    let head_node = place(&mut w, head);
    w.insert(
        head_node,
        StateOverride(vec![(
            Clause(vec![Literal {
                axis: tone,
                values: 0b10,
            }]),
            Override::Variant(1),
        )]),
    );
    let sword_node = place(&mut w, sword);
    w.insert(
        sword_node,
        Attachment {
            rig,
            socket: SocketId(0),
            offset: translate([0.0, 0.0, 0.5]),
        },
    );
    let root = w.spawn();
    w.insert(root, Name("Hero".into()));
    w.insert(root, Source("chr/hero".into()));
    w.insert(
        root,
        Asset {
            members: vec![
                (skeleton, Role::Rig),
                (body, Role::Part),
                (coat, Role::Part),
                (jacket, Role::Part),
                (head, Role::Part),
                (sword, Role::Part),
            ],
        },
    );
    Wardrobe { w, root, outfit }
}

fn manager() -> PassManager {
    let mut r = PassRegistry::new();
    primase_core::register(&mut r);
    PassManager::new(r).with_pass_checks(true)
}

/// Write `world` as a preview into a fresh folder through a conversion; the folder and the conversion's report.
fn preview(
    name: &str,
    world: World,
    select: &str,
) -> (std::path::PathBuf, primase_core::convert::ConvertReport) {
    let m = manager();
    let dir = scratch(name);
    let report = convert_parts(
        &mut Held(world),
        &mut PlannedParts::default(),
        Box::new(PreviewParts::new(&dir, WriteOptions::default()).unwrap()),
        &m,
        &m.registry.parse(select).unwrap(),
        &m.registry.parse("").unwrap(),
    )
    .unwrap();
    (dir, report)
}

fn glb_json(path: &std::path::Path) -> Value {
    primase_glb::Document::parse(&std::fs::read(path).unwrap())
        .unwrap()
        .json
}

fn manifest_of(dir: &std::path::Path) -> Manifest {
    Manifest::read_file(&dir.join("manifest.json")).unwrap()
}

fn member<'a>(a: &'a m::Asset, id: &str) -> &'a m::Member {
    a.members
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("no member {id}"))
}

/// The manifest of the wardrobe, hand-checked: every id, condition, binding and index below is stated here, and the
/// node, mesh and joint indices are checked against the GLB's own JSON, which the writer did not read back.
#[test]
fn the_manifest_states_the_wardrobe_and_points_into_the_glb() {
    let (dir, _) = preview("preview-wardrobe", wardrobe().w, "");
    let manifest = manifest_of(&dir);
    assert_eq!(
        (manifest.format.as_str(), manifest.version),
        ("primase.preview-manifest", 1)
    );
    assert_eq!(
        manifest.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["chr_hero.glb"]
    );
    assert_eq!(
        manifest.files[0].bytes,
        std::fs::metadata(dir.join("chr_hero.glb")).unwrap().len()
    );

    // choices: no default invented for outfit, skin or weapon
    let ids: Vec<&str> = manifest.choices.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids.len(), 4);
    for (id, options, default, class, label) in [
        ("outfit", vec!["coat", "jacket"], None, "outfit", "outfit label"),
        ("hat", vec!["off", "on"], Some("off"), "accessory", "hat label"),
        ("skin", vec!["pale", "dark"], None, "body", "skin label"),
        ("weapon", vec!["off", "on"], None, "accessory", "weapon label"),
    ] {
        let c = manifest.choice(id).unwrap();
        assert_eq!(c.options, options, "{id}");
        assert_eq!(c.default.as_deref(), default, "{id}");
        assert_eq!((c.class.as_str(), c.label.as_str()), (class, label), "{id}");
        assert_eq!(c.source, format!("wardrobe {id}"));
    }

    let [asset] = &manifest.assets[..] else {
        panic!("{:?}", manifest.assets)
    };
    assert_eq!(
        (asset.id.as_str(), asset.label.as_str(), asset.file.as_str()),
        ("chr/hero", "Hero", "chr_hero.glb")
    );
    let member_ids: Vec<&str> = asset.members.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        member_ids,
        ["skeleton", "body", "coat", "jacket", "head", "sword"]
    );
    let lit = |choice: &str, options: &[&str]| m::Literal {
        choice: choice.into(),
        options: options.iter().map(|o| (*o).into()).collect(),
    };
    let only = |l: m::Literal| m::Condition(vec![vec![l]]);
    assert_eq!(member(asset, "body").requires, None);
    assert_eq!(
        member(asset, "coat").requires,
        Some(only(lit("outfit", &["coat"])))
    );
    assert_eq!(
        member(asset, "jacket").requires,
        Some(only(lit("outfit", &["jacket"])))
    );
    assert_eq!(
        member(asset, "sword").requires,
        Some(only(lit("weapon", &["on"])))
    );

    let gltf = glb_json(&dir.join("chr_hero.glb"));
    let nodes = gltf["nodes"].as_array().unwrap();
    let meshes = gltf["meshes"].as_array().unwrap();

    // the rig: its root and joints are the nodes the GLB names so
    let [rig] = &asset.rigs[..] else { panic!() };
    assert_eq!(
        (rig.id.as_str(), rig.skeleton.as_str()),
        ("skeleton@0", "skeleton")
    );
    assert_eq!(nodes[rig.node]["name"], "rig");
    assert_eq!(
        rig.joints
            .iter()
            .map(|j| (j.id, j.name.as_str()))
            .collect::<Vec<_>>(),
        [(0, "root"), (1, "hand")]
    );
    for j in &rig.joints {
        assert_eq!(nodes[j.node]["name"], j.name.as_str());
    }
    assert_eq!(rig.sockets.len(), 1);
    assert_eq!(
        (
            rig.sockets[0].id,
            rig.sockets[0].name.as_str(),
            rig.sockets[0].joint
        ),
        (0, "grip", 1)
    );
    assert_eq!(rig.sockets[0].offset[12..15], [0.0, 0.1, 0.0]);

    // each draw: the node draws the mesh it says
    for mem in &asset.members {
        for d in &mem.draws {
            assert_eq!(nodes[d.node]["mesh"], d.mesh, "{}", d.id);
        }
    }
    let draw = |id: &str, n: usize| {
        let d = &member(asset, id).draws;
        assert_eq!(d.len(), n, "{id}");
        d[0].clone()
    };
    // outfits: skinned to the one rig, binding 0, and the glTF node carries a skin
    for id in ["body", "coat", "jacket"] {
        let d = draw(id, 1);
        assert_eq!(d.id, format!("{id}@0"));
        assert_eq!(
            d.skin,
            Some(m::SkinBinding {
                rig: "skeleton@0".into(),
                binding: 0
            })
        );
        assert!(nodes[d.node].get("skin").is_some(), "{id}");
    }
    // the head: a hat on primitive 1 only when hat is on, and the tone as a variant
    let head = draw("head", 1);
    assert_eq!(
        head.primitives,
        [m::PrimitiveRule {
            primitive: 1,
            requires: only(lit("hat", &["on"]))
        }]
    );
    assert_eq!(meshes[head.mesh]["primitives"].as_array().unwrap().len(), 2);
    assert_eq!(head.variants.len(), 2);
    assert_eq!(
        head.variants.iter().map(|v| v.variant).collect::<Vec<_>>(),
        [0, 1]
    );
    let base = head.variants[0].mesh;
    let darker = head.variants[1].mesh;
    assert_eq!(base, head.mesh);
    assert_ne!(base, darker);
    // the dark mesh shares the geometry and differs in materials
    let primitives = |mesh: usize| meshes[mesh]["primitives"].as_array().unwrap().clone();
    for (a, b) in primitives(base).iter().zip(primitives(darker)) {
        assert_eq!(a["attributes"], b["attributes"]);
        assert_ne!(a["material"], b["material"]);
    }
    assert_eq!(
        head.overrides,
        [m::Override {
            when: vec![lit("skin", &["dark"])],
            set: Set::Variant(1),
        }]
    );
    // the sword: attached to the grip socket, offset 0.5 in z
    let sword = draw("sword", 1);
    let at = sword.attachment.unwrap();
    assert_eq!((at.rig.as_str(), at.socket), ("skeleton@0", 0));
    assert_eq!(at.offset[12..15], [0.0, 0.0, 0.5]);
    // nothing of the state leaks into the GLB's extras: the manifest is the one place it is stated
    let text = gltf.to_string();
    for leaked in [
        "core.condition",
        "core.primitive_condition",
        "core.state_override",
        "core.state_axis",
    ] {
        assert!(!text.contains(leaked), "{leaked} in the GLB");
    }
    // what the manifest carries is not told as left out; nothing else here is left out
    assert_eq!(manifest.fidelity, [], "{:?}", manifest.fidelity);
}

/// The manifest written is the manifest read, and a reader refuses what it cannot trust.
#[test]
fn the_manifest_round_trips_and_the_reader_refuses_what_is_wrong() {
    let (dir, _) = preview("preview-roundtrip", wardrobe().w, "");
    let manifest = manifest_of(&dir);
    let again = Manifest::from_slice(manifest.to_json().unwrap().as_bytes()).unwrap();
    assert_eq!(manifest, again);
    // the file is the text of to_json: written twice, the same bytes
    assert_eq!(
        std::fs::read_to_string(dir.join("manifest.json")).unwrap(),
        manifest.to_json().unwrap()
    );

    let mut value: Value = serde_json::from_str(&manifest.to_json().unwrap()).unwrap();
    let refused = |edit: &dyn Fn(&mut Value), why: &str| {
        let mut v = value.clone();
        edit(&mut v);
        let err = Manifest::from_slice(v.to_string().as_bytes())
            .unwrap_err()
            .to_string();
        assert!(err.contains(why), "{why}: {err}");
    };
    refused(&|v| v["version"] = 2.into(), "version 2");
    refused(&|v| v["format"] = "other".into(), "format");
    refused(&|v| v["surprise"] = 1.into(), "unknown field");
    refused(&|v| v["assets"][0]["file"] = "gone.glb".into(), "is not listed");
    refused(
        &|v| v["choices"][0]["default"] = "nope".into(),
        "not one of its options",
    );
    refused(
        &|v| v["assets"][0]["members"][2]["requires"][0][0]["options"][0] = "nope".into(),
        "not one of its options",
    );
    refused(
        &|v| v["assets"][0]["members"][2]["requires"][0][0]["choice"] = "nope".into(),
        "names the choice \"nope\"",
    );
    refused(
        &|v| v["assets"][0]["members"][3]["id"] = "coat".into(),
        "used twice",
    );
    refused(
        &|v| v["assets"][0]["members"][1]["draws"][0]["skin"]["rig"] = "other@0".into(),
        "skinned to rig",
    );
    value["assets"][0]["members"][1]["draws"][0]["id"] = "body@0".into();
    assert!(Manifest::from_slice(value.to_string().as_bytes()).is_ok());
}

/// A recipe of chosen ids is the passes that apply it, and applying it gives the world selecting by hand gives.
#[test]
fn a_recipe_applied_gives_the_world_selecting_by_hand_gives() {
    let (dir, _) = preview("preview-recipe", wardrobe().w, "");
    let manifest = manifest_of(&dir);
    let mut recipe = Recipe::new();
    recipe.choose("outfit", "jacket").choose("hat", "on");
    assert_eq!(recipe.undecided(&manifest), ["skin", "weapon"]);
    let pipeline = recipe.pipeline(&manifest).unwrap();
    assert_eq!(
        pipeline.to_string(),
        "select-outfit{choices=hat:on;outfit:jacket}"
    );
    // the recipe in JSON and back
    assert_eq!(
        Recipe::from_slice(recipe.to_json().unwrap().as_bytes()).unwrap(),
        recipe
    );

    let m = manager();
    let mut by_recipe = wardrobe();
    m.run(&mut by_recipe.w, &pipeline, &mut RunReport::default())
        .unwrap();
    let mut by_hand = wardrobe();
    m.run(
        &mut by_hand.w,
        &m.registry
            .parse("select-outfit{choices=outfit:jacket;hat:on}")
            .unwrap(),
        &mut RunReport::default(),
    )
    .unwrap();
    let dump = |w: &World| {
        let mut s = String::new();
        for e in w.entities() {
            for c in w.components_json(e).unwrap() {
                s += &format!("{e:?} {} {:?}\n", c.name, c.value);
            }
        }
        s
    };
    assert_eq!(dump(&by_recipe.w), dump(&by_hand.w));

    // by hand, what is left: the coat is gone with its placement, the jacket stays, both head primitives stay, and
    // the choices the recipe did not make are still open with what depends on them
    let w = &by_recipe.w;
    let asset = w.get::<Asset>(by_recipe.root).unwrap();
    let names: Vec<&str> = asset
        .members
        .iter()
        .map(|&(e, _)| w.get::<Name>(e).unwrap().0.as_str())
        .collect();
    assert_eq!(names, ["skeleton", "body", "jacket", "head", "sword"]);
    assert!(!w.has::<StateAxis>(by_recipe.outfit));
    let open: BTreeSet<String> = w.iter::<StateAxis>().map(|(_, a)| a.key.clone()).collect();
    assert_eq!(open, ["skin".to_owned(), "weapon".to_owned()].into());
    let head = asset
        .members
        .iter()
        .map(|&(e, _)| e)
        .find(|&e| w.get::<Name>(e).unwrap().0 == "head")
        .unwrap();
    assert_eq!(w.get::<Mesh>(head).unwrap().primitives().len(), 2);
    assert!(!w.has::<PrimitiveCondition>(head));
    assert!(w.has::<StateOverride>(w.iter::<Instance>().find(|(_, i)| i.prototype == head).unwrap().0));

    // the empty recipe is no pass; a recipe naming what the manifest lacks is refused
    assert!(Recipe::new().pipeline(&manifest).unwrap().passes.is_empty());
    for (choice, option, why) in [
        ("colour", "red", "no such choice"),
        ("outfit", "cape", "is not one of its options (coat, jacket)"),
    ] {
        let mut bad = Recipe::new();
        bad.choose(choice, option);
        assert!(
            bad.pipeline(&manifest).unwrap_err().to_string().contains(why),
            "{choice}"
        );
    }
    assert!(
        Recipe::from_slice(br#"{"format":"primase.preview-recipe","manifest_version":9,"choices":{}}"#)
            .is_err()
    );
}

/// The recipe's pipeline equals `resolve-state` for the choices it makes once the rest is resolved the same way, so
/// a program that resolves what is left gets the same world as one that resolved everything at once.
#[test]
fn a_recipe_then_resolve_state_equals_resolving_at_once() {
    let m = manager();
    let (dir, _) = preview("preview-recipe-once", wardrobe().w, "");
    let manifest = manifest_of(&dir);
    let mut recipe = Recipe::new();
    recipe.choose("outfit", "coat").choose("skin", "dark");
    let mut stepped = wardrobe();
    m.run(
        &mut stepped.w,
        &recipe.pipeline(&manifest).unwrap(),
        &mut RunReport::default(),
    )
    .unwrap();
    m.run(
        &mut stepped.w,
        &m.registry.parse("resolve-state{assign=weapon:off}").unwrap(),
        &mut RunReport::default(),
    )
    .unwrap();
    let mut once = wardrobe();
    m.run(
        &mut once.w,
        &m.registry
            .parse("resolve-state{assign=outfit:coat;skin:dark;weapon:off}")
            .unwrap(),
        &mut RunReport::default(),
    )
    .unwrap();
    let count = |w: &World| {
        (
            w.iter::<Mesh>().count(),
            w.iter::<Instance>().count(),
            w.iter::<StateAxis>().count(),
        )
    };
    assert_eq!(count(&stepped.w), count(&once.w));
    // by hand: the body, the coat and the head stay; the jacket and the sword are gone with their placements
    assert_eq!(count(&once.w), (3, 3, 0));
}

/// Oracle: the Khronos glTF-Validator on every file the preview wrote (with every alternative drawn and the variant
/// meshes), and on the file of a resolved world: no errors.
#[test]
fn khronos_validator_accepts_each_preview_glb() {
    let Some(validator) = inputs::tool(inputs::Tool::GltfValidator) else {
        return;
    };
    let (dir, _) = preview("preview-validator", wardrobe().w, "");
    let mut resolved = wardrobe();
    let m = manager();
    m.run(
        &mut resolved.w,
        &m.registry
            .parse("resolve-state{assign=outfit:coat;skin:dark;weapon:on;hat:off}")
            .unwrap(),
        &mut RunReport::default(),
    )
    .unwrap();
    let dir2 = scratch("preview-validator-resolved");
    convert_parts(
        &mut Held(resolved.w),
        &mut PlannedParts::default(),
        Box::new(primase_gltf::GlbParts::new(&dir2, WriteOptions::default()).unwrap()),
        &m,
        &m.registry.parse("").unwrap(),
        &m.registry.parse("").unwrap(),
    )
    .unwrap();
    let mut validated = 0;
    for d in [&dir, &dir2] {
        for entry in std::fs::read_dir(d).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "glb") {
                continue;
            }
            let out = std::process::Command::new(&validator)
                .arg("-o")
                .arg(&path)
                .output()
                .unwrap();
            let report: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(
                report["issues"]["numErrors"],
                0,
                "{}: {}",
                path.display(),
                report["issues"]
            );
            validated += 1;
        }
    }
    println!("validated {validated} GLBs");
    assert_eq!(validated, 2);
}

/// Two assets partitioned into two files share their choices once; what a stage told the program is added to the
/// fidelity list and the manifest is written again.
#[test]
fn parts_merge_their_choices_and_notices_become_fidelity() {
    let mut w = World::new();
    let material = w.spawn();
    w.insert(material, PbrMaterial::default());
    let outfit = axis(&mut w, "outfit", "outfit", &["coat", "jacket"], None);
    let character = |w: &mut World, key: &str, name: &str, worn: u64| {
        let mesh = w.spawn();
        w.insert(mesh, Mesh::new(vec![rich_primitive(Some(material), 0.0)]));
        w.insert(mesh, Name(format!("{name} outfit")));
        w.insert(mesh, when(outfit, worn));
        let root = w.spawn();
        w.insert(root, Name(name.into()));
        w.insert(root, Source(key.into()));
        w.insert(
            root,
            Asset {
                members: vec![(mesh, Role::Part)],
            },
        );
    };
    character(&mut w, "chr/hero", "Hero", 0b01);
    character(&mut w, "chr/friend", "Friend", 0b10);
    let m = manager();
    let dir = scratch("preview-two");
    let report = convert_parts(
        &mut Held(w),
        &mut PlannedParts::default(),
        Box::new(PreviewParts::new(&dir, WriteOptions::default()).unwrap()),
        &m,
        &m.registry.parse("partition-by-asset").unwrap(),
        &m.registry.parse("place-assets").unwrap(),
    )
    .unwrap();
    let mut manifest = manifest_of(&dir);
    let files: Vec<&str> = manifest.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(files, ["chr_hero.glb", "chr_friend.glb"]);
    let assets: Vec<(&str, &str)> = manifest
        .assets
        .iter()
        .map(|a| (a.id.as_str(), a.file.as_str()))
        .collect();
    assert_eq!(
        assets,
        [("chr/hero", "chr_hero.glb"), ("chr/friend", "chr_friend.glb")]
    );
    // the shared axis is listed once, and each asset's draws are placed by place-assets
    assert_eq!(manifest.choices.iter().filter(|c| c.id == "outfit").count(), 1);
    for a in &manifest.assets {
        assert_eq!(a.members[0].draws.len(), 1, "{}", a.id);
    }

    // what the conversion reported is added, as it is; the same notice twice is one entry
    let before = manifest.fidelity.len();
    let notice = primase_core::convert::Notice::new(
        "fit-outfit",
        NoticeKind::Approximation,
        "shoulders approximated",
        3,
    )
    .example("coat");
    manifest.add_notices(report.reported.iter().chain([&notice, &notice]));
    assert_eq!(manifest.fidelity.len(), before + 1 + report.reported.len());
    let last = manifest.fidelity.last().unwrap();
    assert_eq!(
        (last.stage.as_str(), last.kind, last.count),
        ("fit-outfit", m::Kind::Approximation, 3)
    );
    manifest.write_file(&dir.join("manifest.json")).unwrap();
    assert_eq!(manifest_of(&dir), manifest);
}

/// What the manifest cannot state is refused, naming the item: an axis scoped to an entity, an asset with no id.
#[test]
fn what_the_manifest_cannot_state_is_refused() {
    let m = manager();
    let run = |w: World| {
        convert_parts(
            &mut Held(w),
            &mut PlannedParts::default(),
            Box::new(PreviewParts::new(scratch("preview-refused"), WriteOptions::default()).unwrap()),
            &m,
            &m.registry.parse("").unwrap(),
            &m.registry.parse("").unwrap(),
        )
        .unwrap_err()
        .to_string()
    };
    let mut local = wardrobe();
    let owner = local.w.spawn();
    local.w.insert(owner, Name("door".into()));
    let a = local.w.spawn();
    local.w.insert(
        a,
        StateAxis {
            key: "door.open".into(),
            domain: Domain::Bool,
            default: Some(0),
            scope: Scope::Local(owner),
            class: "door".into(),
            source: "a door".into(),
        },
    );
    let err = run(local.w);
    assert!(err.contains("axis door.open is scoped to an entity"), "{err}");
    let mut anonymous = wardrobe();
    anonymous.w.remove::<Source>(anonymous.root);
    anonymous.w.remove::<Name>(anonymous.root);
    let err = run(anonymous.w);
    assert!(err.contains("neither core.source nor core.name"), "{err}");
}
