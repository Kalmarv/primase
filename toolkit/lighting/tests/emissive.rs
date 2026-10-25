//! `emissive-lights` on hand-worked scenes: every expected number below is worked by hand from the pass's stated formula
//! (radiance = colour x `emissive_strength` x `nits_per_strength`; intensity = its largest channel x area x
//! `area_factor`, times `|det|^(2/3)` of the placement; range = sqrt(intensity / `e_min`) kept within the range options),
//! with `nits_per_strength` 100 and `area_factor` 1 so the arithmetic is by eye. There is no independent oracle for what
//! the pass makes: the formula is the program's stated choice, and these cases check that the pass does what it says.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform, Wrap};
use primase_core::{
    ColorSpace, Entity, Instance, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, TexFormat,
    TexRole, Texture, Topology, Transform, World,
};
use primase_lighting::{EMISSIVE_LIGHTS, EmissiveLights, EmissivePolicy, Spots};

/// The options every case starts from (weld 1 mm, a bulb up to 2.5 m, a string from 4 pieces kept as 2 lights, a strip
/// light per 8 m, 100 nits per strength, area counted whole, luminance floor 0.05, 2 lux at the range, range 3 to 30 m,
/// at most 100 lights, no shadows, no animated material).
fn policy() -> EmissivePolicy {
    EmissivePolicy {
        weld: 0.001,
        big: 2.5,
        string: 4,
        per_string: 2,
        strip: 8.0,
        nits_per_strength: 100.0,
        area_factor: 1.0,
        min_luminance: 0.05,
        e_min: 2.0,
        range: [3.0, 30.0],
        max_total: 100,
        shadows: Some(false),
        animated: BTreeSet::new(),
        spots: None,
        scale: BTreeMap::new(),
    }
}

type Quad = [[f32; 3]; 4];

/// An axis-aligned quad `w` by `h` in the z = 0 plane with its minimum corner at `(x, y)`.
fn quad(x: f32, y: f32, w: f32, h: f32) -> Quad {
    [[x, y, 0.0], [x + w, y, 0.0], [x + w, y + h, 0.0], [x, y + h, 0.0]]
}

fn material(w: &mut World, name: &str, m: PbrMaterial) -> Entity {
    let e = w.spawn();
    w.insert(e, m);
    w.insert(e, Name(name.to_owned()));
    e
}

fn emissive(colour: [f32; 3], strength: f32) -> PbrMaterial {
    PbrMaterial {
        emissive: colour,
        emissive_strength: strength,
        ..PbrMaterial::default()
    }
}

/// A prototype of `quads` (each two triangles, vertices not shared between quads) drawn with `material`, with UVs when
/// given (one pair per corner of each quad), and a placement of it by `transform`.
fn place(
    w: &mut World,
    quads: &[Quad],
    uvs: Option<Vec<[f32; 2]>>,
    material: Entity,
    transform: Transform,
) -> (Entity, Entity) {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for q in quads {
        let base = positions.len() as u32;
        positions.extend(q.iter().copied());
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let mut streams = BTreeMap::from([(Semantic::Position, Stream::F32x3(positions))]);
    if let Some(uvs) = uvs {
        streams.insert(Semantic::TexCoord(0), Stream::F32x2(uvs));
    }
    let prototype = w.spawn();
    w.insert(prototype, Name("lamp".to_owned()));
    w.insert(
        prototype,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(indices),
            material: Some(material),
        }]),
    );
    let placement = w.spawn();
    w.insert(
        placement,
        Instance {
            prototype,
            variant: 0,
        },
    );
    w.insert(placement, transform);
    (prototype, placement)
}

fn at(p: [f32; 3]) -> Transform {
    Transform {
        cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], p],
    }
}

/// Runs the pass through the pass manager (declarations checked, the world verified) and returns its log.
fn run(w: &mut World, p: EmissivePolicy) -> Result<PassLog, String> {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(EmissiveLights::new(p)?)],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report.steps.remove(0).log)
}

/// The lights made, sorted by x then y: position and light.
fn lights(w: &World) -> Vec<([f32; 3], Light)> {
    let mut v: Vec<_> = w
        .iter::<Light>()
        .map(|(e, l)| (w.get::<Transform>(e).unwrap().cols[3], l.clone()))
        .collect();
    v.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]).then(a.0[1].total_cmp(&b.0[1])));
    v
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-3 * b.abs().max(1.0)
}

/// A quad 0.2 x 0.1 m (area 0.02) of colour (1, 0.5, 0) and strength 10: radiance (1000, 500, 0) cd/m2, intensity
/// 1000 x 0.02 = 20 cd, colour (1, 0.5, 0), range sqrt(20 / 2) = 3.162 m (inside 3 to 30), radius sqrt(0.02 / pi) =
/// 0.0798 m, at its centre (0.1, 0.05, 0) moved by the placement to (10.1, 0.05, 0); a point (no `spots`).
#[test]
fn one_emissive_quad_is_one_light_by_the_formula() {
    let mut w = World::new();
    let m = material(&mut w, "lamp glass", emissive([1.0, 0.5, 0.0], 10.0));
    let (_, placement) = place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], None, m, at([10.0, 0.0, 0.0]));
    let log = run(&mut w, policy()).unwrap();
    let made = lights(&w);
    assert_eq!(made.len(), 1);
    let (p, l) = &made[0];
    assert!(near(p[0], 10.1) && near(p[1], 0.05) && near(p[2], 0.0), "{p:?}");
    assert_eq!(l.kind, LightKind::Point);
    assert!(near(l.intensity, 20.0), "{}", l.intensity);
    assert_eq!(l.color, [1.0, 0.5, 0.0]);
    assert!(near(l.range.unwrap(), 3.1623), "{:?}", l.range);
    assert!(near(l.radius.unwrap(), 0.079_788), "{:?}", l.radius);
    assert_eq!(l.cast_shadows, Some(false));
    assert_eq!(log.counts["emissive lights made"], 1);
    assert_eq!(log.counts["points"], 1);
    assert_eq!(log.approximations[APPROXIMATION], 1);
    // made from its placement: a conversion in parts keeps it with the owner
    assert_eq!(
        log.placed_from.values().copied().collect::<Vec<_>>(),
        vec![placement]
    );
}

/// A material with no emission, or a zero strength, makes nothing; so does a placement without a transform.
#[test]
fn what_emits_nothing_makes_no_light() {
    let mut w = World::new();
    let dull = material(&mut w, "dull", PbrMaterial::default());
    let off = material(&mut w, "off", emissive([1.0, 1.0, 1.0], 0.0));
    place(&mut w, &[quad(0.0, 0.0, 1.0, 1.0)], None, dull, at([0.0; 3]));
    place(&mut w, &[quad(0.0, 0.0, 1.0, 1.0)], None, off, at([0.0; 3]));
    let log = run(&mut w, policy()).unwrap();
    assert!(lights(&w).is_empty());
    assert_eq!(log.counts["emissive lights made"], 0);
}

/// Two quads that touch nowhere are two pieces, two lights; two that share an edge (the same positions) are one piece:
/// one light at the mean of its six distinct vertices.
#[test]
fn pieces_are_triangles_joined_at_one_position() {
    let mut w = World::new();
    let m = material(&mut w, "glass", emissive([1.0, 1.0, 1.0], 1.0));
    place(
        &mut w,
        &[quad(0.0, 0.0, 0.1, 0.1), quad(5.0, 0.0, 0.1, 0.1)],
        None,
        m,
        at([0.0; 3]),
    );
    run(&mut w, policy()).unwrap();
    assert_eq!(lights(&w).len(), 2);

    let mut w = World::new();
    let m = material(&mut w, "glass", emissive([1.0, 1.0, 1.0], 1.0));
    // the second quad starts at the first's right edge: x 0.1 is shared by two vertices, 1 mm apart or less
    place(
        &mut w,
        &[quad(0.0, 0.0, 0.1, 0.1), quad(0.1, 0.0, 0.1, 0.1)],
        None,
        m,
        at([0.0; 3]),
    );
    run(&mut w, policy()).unwrap();
    let made = lights(&w);
    assert_eq!(made.len(), 1);
    // six distinct vertices: x 0, 0.1, 0.2 twice each over y 0 and 0.1: mean (0.1, 0.05)
    assert!(
        near(made[0].0[0], 0.1) && near(made[0].0[1], 0.05),
        "{:?}",
        made[0].0
    );
}

/// Ten quads 0.1 x 0.1 (area 0.01) a metre apart, white, strength 10: 10 cd each, 100 cd in all. `string` 5 and
/// `per_string` 4 divide the ten, in order along the string, into groups of 2, 3, 2 and 3 (`g * 10 / 4` for g = 0..4):
/// four lights of 20, 30, 20 and 30 cd at the groups' mean positions x 0.55, 3.05, 5.55 and 8.05 (a quad's centre is
/// at i + 0.05), the string's 100 cd kept.
#[test]
fn a_string_is_thinned_and_its_power_is_kept() {
    let mut w = World::new();
    let m = material(&mut w, "bulbs", emissive([1.0, 1.0, 1.0], 10.0));
    let quads: Vec<Quad> = (0..10).map(|i| quad(i as f32, 0.0, 0.1, 0.1)).collect();
    place(&mut w, &quads, None, m, at([0.0; 3]));
    let p = EmissivePolicy {
        weld: 0.001,
        big: 2.5,
        string: 5,
        per_string: 4,
        strip: 8.0,
        nits_per_strength: 100.0,
        area_factor: 1.0,
        min_luminance: 0.05,
        e_min: 2.0,
        range: [3.0, 30.0],
        max_total: 100,
        shadows: Some(false),
        animated: BTreeSet::new(),
        spots: None,
        scale: BTreeMap::new(),
    };
    let log = run(&mut w, p).unwrap();
    let made = lights(&w);
    assert_eq!(made.len(), 4);
    for ((p, l), (x, cd)) in made
        .iter()
        .zip([(0.55, 20.0), (3.05, 30.0), (5.55, 20.0), (8.05, 30.0)])
    {
        assert!(near(p[0], x) && near(l.intensity, cd), "{p:?} {}", l.intensity);
    }
    assert!(near(made.iter().map(|(_, l)| l.intensity).sum::<f32>(), 100.0));
    assert_eq!(log.counts["pieces folded into fewer lights (strings)"], 6);
    // fewer pieces than `string` (3 of the default policy's 4): one light each, none thinned
    let mut w = World::new();
    let m = material(&mut w, "bulbs", emissive([1.0, 1.0, 1.0], 10.0));
    let quads: Vec<Quad> = (0..3).map(|i| quad(i as f32, 0.0, 0.1, 0.1)).collect();
    place(&mut w, &quads, None, m, at([0.0; 3]));
    run(&mut w, policy()).unwrap();
    assert_eq!(lights(&w).len(), 3);
}

/// A rectangle 20 m x 0.1 m (area 2) is longer than `big`: a strip of ceil(20 / 8) = 3 lights spaced along its long axis
/// at the centres of its thirds, x -6.667, 0 and 6.667, sharing its 1000 x 2 = 2000 cd (666.67 cd each).
#[test]
fn a_strip_is_split_into_lights_along_it() {
    let mut w = World::new();
    let m = material(&mut w, "strip", emissive([1.0, 1.0, 1.0], 10.0));
    place(&mut w, &[quad(-10.0, 0.0, 20.0, 0.1)], None, m, at([0.0; 3]));
    let log = run(&mut w, policy()).unwrap();
    let made = lights(&w);
    assert_eq!(made.len(), 3);
    for ((p, l), x) in made.iter().zip([-6.6667, 0.0, 6.6667]) {
        assert!(near(p[0], x) && near(p[1], 0.05), "{p:?}");
        assert!(near(l.intensity, 666.667), "{}", l.intensity);
    }
    assert_eq!(log.counts["strips split into lights"], 1);
}

fn texture(w: &mut World, bytes: [u8; 8], space: ColorSpace) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Texture {
            width: 2,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![bytes.to_vec()],
            color_space: space,
            role: TexRole::Color,
            normal: None,
        },
    );
    e
}

fn textured(w: &mut World, name: &str, tex: Entity, strength: f32) -> Entity {
    material(
        w,
        name,
        PbrMaterial {
            emissive: [1.0, 1.0, 1.0],
            emissive_strength: strength,
            emissive_texture: Some(TextureRef::new(tex)),
            ..PbrMaterial::default()
        },
    )
}

/// A 2 x 1 texture, its left texel black and its right white. A quad whose texture coordinates lie in the left half
/// samples black: luminance 0 is under the floor, left out and counted; one in the right half samples white and is a
/// light of 1 x 10 x 100 = 1000 cd/m2 over 0.02 m2 = 20 cd.
#[test]
fn a_piece_is_coloured_by_the_texel_under_its_texture_coordinates() {
    let mut w = World::new();
    let tex = texture(&mut w, [0, 0, 0, 255, 255, 255, 255, 255], ColorSpace::Linear);
    let m = textured(&mut w, "glass", tex, 10.0);
    let left = vec![[0.1, 0.5], [0.4, 0.5], [0.4, 0.6], [0.1, 0.6]];
    let right = vec![[0.6, 0.5], [0.9, 0.5], [0.9, 0.6], [0.6, 0.6]];
    place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], Some(left), m, at([0.0; 3]));
    place(&mut w, &[quad(5.0, 0.0, 0.2, 0.1)], Some(right), m, at([0.0; 3]));
    let log = run(&mut w, policy()).unwrap();
    let made = lights(&w);
    assert_eq!(made.len(), 1);
    assert!(
        near(made[0].0[0], 5.1) && near(made[0].1.intensity, 20.0),
        "{:?}",
        made[0]
    );
    assert_eq!(made[0].1.color, [1.0, 1.0, 1.0]);
    assert_eq!(
        log.counts["pieces left out: luminance under the floor or no area"],
        1
    );
    // the texture's transform and wrap move the sample: an offset of 0.5 puts the left piece on the white texel, and
    // clamp keeps a coordinate past 1 on the last texel
    let mut w = World::new();
    let tex = texture(&mut w, [0, 0, 0, 255, 255, 255, 255, 255], ColorSpace::Linear);
    let mut mat = emissive([1.0, 1.0, 1.0], 10.0);
    let mut r = TextureRef::new(tex);
    r.transform = Some(UvTransform {
        offset: [0.5, 0.0],
        scale: [1.0, 1.0],
        rotation: 0.0,
    });
    r.wrap = [Wrap::Clamp, Wrap::Clamp];
    mat.emissive_texture = Some(r);
    let m = material(&mut w, "shifted", mat);
    let left = vec![[0.1, 0.5], [0.4, 0.5], [0.4, 0.6], [0.1, 0.6]];
    place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], Some(left), m, at([0.0; 3]));
    run(&mut w, policy()).unwrap();
    assert_eq!(lights(&w).len(), 1);
}

/// The texture's colour space is undone: an sRGB byte 188 is linear 0.5029 (the sRGB curve), so the light's radiance
/// is 0.5029 x 10 x 100 = 502.9 cd/m2 and its intensity 502.9 x 0.02 = 10.06 cd; a linear texture's byte 188 would be
/// 0.7373.
#[test]
fn an_srgb_texture_is_linearised() {
    let mut w = World::new();
    let tex = texture(&mut w, [188, 188, 188, 255, 188, 188, 188, 255], ColorSpace::Srgb);
    let m = textured(&mut w, "glass", tex, 10.0);
    let uv = vec![[0.1, 0.5], [0.4, 0.5], [0.4, 0.6], [0.1, 0.6]];
    place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], Some(uv), m, at([0.0; 3]));
    run(&mut w, policy()).unwrap();
    let made = lights(&w);
    assert!(near(made[0].1.intensity, 10.058), "{}", made[0].1.intensity);
}

/// A material whose name starts with an `animated` prefix takes the texture's mean, not a texel: here (0 + 1) / 2 = 0.5
/// per channel, 0.5 x 10 x 100 = 500 cd/m2, 10 cd over 0.02 m2, whichever half the piece's coordinates are in. A prefix
/// no emissive material starts with is counted, and the material is then read by its texel.
#[test]
fn an_animated_material_takes_the_textures_mean() {
    let mut w = World::new();
    let tex = texture(&mut w, [0, 0, 0, 255, 255, 255, 255, 255], ColorSpace::Linear);
    let m = textured(&mut w, "Anim_blinker", tex, 10.0);
    let left = vec![[0.1, 0.5], [0.4, 0.5], [0.4, 0.6], [0.1, 0.6]];
    place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], Some(left), m, at([0.0; 3]));
    let animated = |prefix: &str| EmissivePolicy {
        weld: 0.001,
        big: 2.5,
        string: 4,
        per_string: 2,
        strip: 8.0,
        nits_per_strength: 100.0,
        area_factor: 1.0,
        min_luminance: 0.05,
        e_min: 2.0,
        range: [3.0, 30.0],
        max_total: 100,
        shadows: Some(false),
        animated: BTreeSet::from([prefix.to_owned()]),
        spots: None,
        scale: BTreeMap::new(),
    };
    let log = run(&mut w, animated("Nothing_")).unwrap();
    assert_eq!(
        log.counts["animated prefixes that no emissive material's name starts with"],
        1
    );
    // not animated: the texel under the coordinates (black: left out), not the mean
    assert_eq!(w.iter::<Light>().count(), 0);
    let log = run(&mut w, animated("Anim_")).unwrap();
    assert_eq!(
        log.counts["animated prefixes that no emissive material's name starts with"],
        0
    );
    let made = lights(&w);
    assert!(
        near(made[0].1.intensity, 10.0) && made[0].1.color == [1.0, 1.0, 1.0],
        "{:?}",
        made[0]
    );
}

/// A placement scaled by 2 has 4 times the area and stands 2 times as far out: the 20 cd light of the first case is 80 cd
/// at (0.2, 0.1, 0) (the range, sqrt(80 / 2) = 6.32 m).
#[test]
fn a_scaled_placement_scales_the_area_and_the_position() {
    let mut w = World::new();
    let m = material(&mut w, "glass", emissive([1.0, 0.5, 0.0], 10.0));
    place(
        &mut w,
        &[quad(0.0, 0.0, 0.2, 0.1)],
        None,
        m,
        Transform {
            cols: [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0], [0.0; 3]],
        },
    );
    run(&mut w, policy()).unwrap();
    let made = lights(&w);
    assert!(
        near(made[0].0[0], 0.2) && near(made[0].0[1], 0.1),
        "{:?}",
        made[0].0
    );
    assert!(near(made[0].1.intensity, 80.0), "{}", made[0].1.intensity);
    assert!(near(made[0].1.range.unwrap(), 6.3246), "{:?}", made[0].1.range);
}

/// The emissive lights' approximation, as the pass logs it.
const APPROXIMATION: &str = "emissive surfaces as lights (radiance times area as intensity; the surface as a sphere, or a disc's spot)";

/// Each light made with its transform, sorted by position.
fn aimed(w: &World) -> Vec<(Transform, Light)> {
    let mut v: Vec<_> = w
        .iter::<Light>()
        .map(|(e, l)| (*w.get::<Transform>(e).unwrap(), l.clone()))
        .collect();
    v.sort_by(|a, b| {
        a.0.cols[3]
            .iter()
            .zip(&b.0.cols[3])
            .map(|(x, y)| x.total_cmp(y))
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    v
}

fn near3(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(&b).all(|(x, y)| near(*x, *y))
}

/// With `spots=0.9,10,90` a flat quad (counter-clockwise from +z: its front faces +z) is a spot shining along +z (its
/// transform's local -Z is +z), cones 10 and 90 degrees in radians; turned 90 degrees about x by its placement it shines
/// along -y; mirrored in z (its winding flipped, so its drawn front faces -z) along -z. A closed box's normals cancel: a
/// point, whatever `spots` says. The on-axis intensity is the point's (20 cd).
#[test]
fn a_flat_piece_is_a_spot_along_its_normal() {
    let spots = || EmissivePolicy {
        spots: Some(Spots {
            flat: 0.9,
            inner: 10.0,
            outer: 90.0,
        }),
        ..policy()
    };
    let shines = |placement: Transform| -> (Transform, Light) {
        let mut w = World::new();
        let m = material(&mut w, "panel", emissive([1.0, 0.5, 0.0], 10.0));
        place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], None, m, placement);
        let log = run(&mut w, spots()).unwrap();
        assert_eq!(log.counts["spots along their pieces' normal"], 1);
        aimed(&w).remove(0)
    };
    let (t, l) = shines(at([0.0; 3]));
    assert_eq!(
        l.kind,
        LightKind::Spot {
            inner_cone: 10f64.to_radians() as f32,
            outer_cone: 90f64.to_radians() as f32,
        }
    );
    assert!(near3(t.cols[2], [0.0, 0.0, -1.0]), "{:?}", t.cols[2]);
    assert!(near(l.intensity, 20.0), "{}", l.intensity);
    assert!(near3(t.cols[3], [0.1, 0.05, 0.0]), "{:?}", t.cols[3]);
    // about x by 90 degrees: y onto z, z onto -y
    let (t, _) = shines(Transform {
        cols: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0], [0.0; 3]],
    });
    assert!(near3(t.cols[2], [0.0, 1.0, 0.0]), "{:?}", t.cols[2]);
    // mirrored in z
    let (t, _) = shines(Transform {
        cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0], [0.0; 3]],
    });
    assert!(near3(t.cols[2], [0.0, 0.0, 1.0]), "{:?}", t.cols[2]);
    // a closed unit box, its faces out: its normals sum to nothing
    let mut w = World::new();
    let m = material(&mut w, "bulb", emissive([1.0, 1.0, 1.0], 10.0));
    let v = |x: f32, y: f32, z: f32| [x, y, z];
    let faces: Vec<Quad> = vec![
        [v(0., 0., 1.), v(1., 0., 1.), v(1., 1., 1.), v(0., 1., 1.)],
        [v(0., 1., 0.), v(1., 1., 0.), v(1., 0., 0.), v(0., 0., 0.)],
        [v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 1.), v(0., 0., 1.)],
        [v(0., 1., 1.), v(1., 1., 1.), v(1., 1., 0.), v(0., 1., 0.)],
        [v(1., 0., 0.), v(1., 1., 0.), v(1., 1., 1.), v(1., 0., 1.)],
        [v(0., 0., 1.), v(0., 1., 1.), v(0., 1., 0.), v(0., 0., 0.)],
    ];
    place(&mut w, &faces, None, m, at([0.0; 3]));
    let log = run(&mut w, spots()).unwrap();
    assert_eq!(log.counts["points"], 1);
    assert_eq!(aimed(&w)[0].1.kind, LightKind::Point);
}

/// `scale=Flood_:10;Flood_Big:4` multiplies the lights of a prototype named `Flood_Big_A` by 4 (the longest prefix
/// that matches) and of `Flood_A` by 10, before the range (200 cd reaches sqrt(200 / 2) = 10 m); `lamp` keeps 20 cd;
/// an unused prefix is counted. A placement scaled by 2 doubles the radius (0.1596 m).
#[test]
fn a_prototype_scale_multiplies_its_lights() {
    let mut w = World::new();
    let m = material(&mut w, "glass", emissive([1.0, 0.5, 0.0], 10.0));
    place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], None, m, at([0.0; 3]));
    let (flood, _) = place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], None, m, at([10.0, 0.0, 0.0]));
    w.insert(flood, Name("Flood_A".into()));
    let (big, _) = place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], None, m, at([20.0, 0.0, 0.0]));
    w.insert(big, Name("Flood_Big_A".into()));
    let (_, _) = place(
        &mut w,
        &[quad(0.0, 0.0, 0.2, 0.1)],
        None,
        m,
        Transform {
            cols: [
                [2.0, 0.0, 0.0],
                [0.0, 2.0, 0.0],
                [0.0, 0.0, 2.0],
                [30.0, 0.0, 0.0],
            ],
        },
    );
    let p = EmissivePolicy {
        scale: BTreeMap::from([
            ("Flood_".to_owned(), 10.0),
            ("Flood_Big".to_owned(), 4.0),
            ("Nothing".to_owned(), 3.0),
        ]),
        ..policy()
    };
    let log = run(&mut w, p).unwrap();
    let made = lights(&w);
    let cd: Vec<f32> = made.iter().map(|(_, l)| l.intensity).collect();
    assert!(
        near(cd[0], 20.0) && near(cd[1], 200.0) && near(cd[2], 80.0) && near(cd[3], 80.0),
        "{cd:?}"
    );
    assert!(near(made[1].1.range.unwrap(), 10.0), "{:?}", made[1].1.range);
    assert!(
        near(made[3].1.radius.unwrap(), 2.0 * 0.079_788),
        "{:?}",
        made[3].1.radius
    );
    assert_eq!(log.counts["lights scaled by their prototype's factor"], 2);
    assert_eq!(
        log.counts["scale prefixes that no prototype's name starts with"],
        1
    );
}

/// The range is kept within its options: a 0.05 cd light would reach 0.16 m and is raised to 3; a 10,000 cd one would reach
/// 70.7 m and is cut at 30.
#[test]
fn the_range_is_kept_within_its_options() {
    let mut w = World::new();
    let dim = material(&mut w, "dim", emissive([1.0, 1.0, 1.0], 0.05));
    let bright = material(&mut w, "bright", emissive([1.0, 1.0, 1.0], 10000.0));
    place(&mut w, &[quad(0.0, 0.0, 0.1, 0.1)], None, dim, at([0.0; 3]));
    place(
        &mut w,
        &[quad(0.0, 0.0, 0.1, 0.1)],
        None,
        bright,
        at([10.0, 0.0, 0.0]),
    );
    run(&mut w, policy()).unwrap();
    let made = lights(&w);
    assert_eq!(made[0].1.range, Some(3.0));
    assert_eq!(made[1].1.range, Some(30.0));
}

/// More lights than `max_total` is an error naming the count, and the world is left as it was (no light made).
#[test]
fn more_lights_than_the_total_is_refused_not_dropped() {
    let mut w = World::new();
    let m = material(&mut w, "glass", emissive([1.0, 1.0, 1.0], 1.0));
    for i in 0..5 {
        place(
            &mut w,
            &[quad(0.0, 0.0, 0.1, 0.1)],
            None,
            m,
            at([i as f32, 0.0, 0.0]),
        );
    }
    let p = EmissivePolicy {
        weld: 0.001,
        big: 2.5,
        string: 4,
        per_string: 2,
        strip: 8.0,
        nits_per_strength: 100.0,
        area_factor: 1.0,
        min_luminance: 0.05,
        e_min: 2.0,
        range: [3.0, 30.0],
        max_total: 4,
        shadows: Some(false),
        animated: BTreeSet::new(),
        spots: None,
        scale: BTreeMap::new(),
    };
    let err = run(&mut w, p).unwrap_err();
    assert!(err.contains("5 lights") && err.contains("max_total 4"), "{err}");
    assert_eq!(w.iter::<Light>().count(), 0);
}

/// An emissive texture with no texture-coordinate stream, or that is no `core.texture`, is an error naming it.
#[test]
fn an_unreadable_emissive_texture_is_an_error() {
    let mut w = World::new();
    let tex = texture(&mut w, [255; 8], ColorSpace::Linear);
    let m = textured(&mut w, "glass", tex, 1.0);
    place(&mut w, &[quad(0.0, 0.0, 0.1, 0.1)], None, m, at([0.0; 3]));
    let err = run(&mut w, policy()).unwrap_err();
    assert!(err.contains("no TexCoord(0)"), "{err}");

    let mut w = World::new();
    let nowhere = w.spawn();
    let m = textured(&mut w, "glass", nowhere, 1.0);
    place(
        &mut w,
        &[quad(0.0, 0.0, 0.1, 0.1)],
        Some(vec![[0.0; 2]; 4]),
        m,
        at([0.0; 3]),
    );
    let err = run(&mut w, policy()).unwrap_err();
    assert!(err.contains("is no core.texture"), "{err}");
}

/// A mesh drawn by an entity of its own (a mesh and a transform, placed by no placement) is a placement too.
#[test]
fn an_entity_that_draws_its_own_mesh_is_placed() {
    let mut w = World::new();
    let m = material(&mut w, "glass", emissive([1.0, 1.0, 1.0], 10.0));
    let (prototype, placement) = place(&mut w, &[quad(0.0, 0.0, 0.2, 0.1)], None, m, at([0.0; 3]));
    // the entity draws the prototype's mesh itself: the mesh moves to the placement, and the instance goes
    let mesh = w.get::<Mesh>(prototype).unwrap().clone();
    w.insert(placement, mesh);
    w.remove::<Instance>(placement);
    w.despawn(prototype);
    w.insert(placement, at([3.0, 0.0, 0.0]));
    run(&mut w, policy()).unwrap();
    let made = lights(&w);
    assert_eq!(made.len(), 1);
    assert!(near(made[0].0[0], 3.1), "{:?}", made[0].0);
}

/// Every option is required, an unknown one refused, and a policy prints and reads back as itself.
#[test]
fn options_are_required_and_round_trip() {
    let full: Options = policy_options();
    let make = |o: &Options| EMISSIVE_LIGHTS.instance(o);
    assert_eq!(make(&full).unwrap().options(), full);
    for k in full.keys() {
        let mut o = full.clone();
        o.remove(k);
        let err = make(&o).err().unwrap();
        assert!(err.starts_with(&format!("{k} is required: ")), "{k}: {err}");
    }
    let mut o = full.clone();
    o.insert("colour".into(), "red".into());
    assert!(make(&o).err().unwrap().contains("unknown option colour"));
    for (k, v) in [
        ("per_string", "8"),
        ("string", "1"),
        ("range_min", "40"),
        ("weld", "0"),
        ("shadows", "maybe"),
        ("spots", "0.9,0"),
        ("spots", "0,0,90"),
        ("spots", "0.9,60,30"),
        ("spots", "0.9,0,120"),
        ("scale", "Stadium"),
        ("scale", "Stadium:0"),
        ("scale", ":2"),
    ] {
        let mut o = full.clone();
        o.insert(k.into(), v.into());
        assert!(make(&o).is_err(), "{k}={v}");
    }
    let mut o = full;
    o.insert("animated".into(), "A_;B_".into());
    o.insert("spots".into(), "0.9,0,90".into());
    o.insert("scale".into(), "Flood_:10;Stadium:2.5".into());
    let made = make(&o).unwrap().options();
    assert_eq!(made["animated"], "A_;B_");
    assert_eq!(made["spots"], "0.9,0,90");
    assert_eq!(made["scale"], "Flood_:10;Stadium:2.5");
}

fn policy_options() -> Options {
    [
        ("weld", "0.001"),
        ("big", "2.5"),
        ("string", "8"),
        ("per_string", "6"),
        ("strip", "8"),
        ("nits_per_strength", "558.5"),
        ("area_factor", "1"),
        ("min_luminance", "0.05"),
        ("e_min", "2"),
        ("range_min", "3"),
        ("range_max", "30"),
        ("max_total", "3500"),
        ("shadows", "off"),
        ("animated", "none"),
        ("spots", "none"),
        ("scale", "none"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect()
}

/// A primitive the mesh verification would refuse (a stream of another length, an index past the vertices) is an error
/// naming the primitive and the numbers, never a light sampled at texture coordinate (0, 0) or placed at a made-up point.
/// Run directly, without the manager that verifies the world first.
#[test]
fn a_malformed_primitive_is_an_error_not_a_guess() {
    let direct = |w: &mut World| {
        EmissiveLights::new(policy())
            .unwrap()
            .run(w, &mut PassLog::default())
    };
    // texture coordinates for 3 of the quad's 4 vertices
    let mut w = World::new();
    let tex = texture(&mut w, [255; 8], ColorSpace::Linear);
    let m = textured(&mut w, "glass", tex, 1.0);
    place(
        &mut w,
        &[quad(0.0, 0.0, 0.1, 0.1)],
        Some(vec![[0.0; 2]; 3]),
        m,
        at([0.0; 3]),
    );
    let err = direct(&mut w).unwrap_err();
    assert!(
        err.contains("primitive 0") && err.contains("TexCoord(0) has 3 entries for 4 vertices"),
        "{err}"
    );
    // an index past the vertices
    let mut w = World::new();
    let m = material(&mut w, "glass", emissive([1.0, 1.0, 1.0], 1.0));
    let (prototype, _) = place(&mut w, &[quad(0.0, 0.0, 0.1, 0.1)], None, m, at([0.0; 3]));
    let mut mesh = w.get::<Mesh>(prototype).unwrap().clone();
    for mut p in mesh.primitives_mut() {
        *p.indices_mut() = Some(vec![0, 1, 9]);
    }
    w.insert(prototype, mesh);
    let err = direct(&mut w).unwrap_err();
    assert!(err.contains("indexes vertex 9 of 4"), "{err}");
}

/// A piece with no area (corners all one point) has no light to give: left out and counted, not a light of NaN colour
/// from a total of 0.
#[test]
fn a_degenerate_piece_is_left_out() {
    let mut w = World::new();
    let m = material(&mut w, "glass", emissive([1.0, 1.0, 1.0], 10.0));
    place(&mut w, &[[[0.0; 3]; 4]], None, m, at([0.0; 3]));
    let log = run(&mut w, policy()).unwrap();
    assert!(lights(&w).is_empty());
    assert_eq!(
        log.counts["pieces left out: luminance under the floor or no area"],
        1
    );
}
