#![allow(clippy::unwrap_used)]
//! The ray tracing pipeline on the CPU, run on the lifted programs of `ray_tracing.hlsl` (raygen, miss, closest_hit,
//! any_hit, intersection, callable) over scenes built here. The values are the arithmetic of the HLSL.
use primase_dxil::lift;
use primase_shader::exec::{
    Geometry, Instance, InstanceFlags, Pipeline, PipelineResources, Scene, ShaderTable,
};
use primase_shader::{HitGroup, HitGroupKind, Program, Sampler, Store};
use std::collections::BTreeMap;
use std::path::Path;

fn program(export: &str, variant: &str) -> Program {
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(format!("ray_tracing.{export}{variant}.dxil")),
    )
    .unwrap();
    lift(&bytes).unwrap_or_else(|e| panic!("{export}{variant}: {e}"))
}

/// The pipeline of the six programs, two hit groups (triangles: any-hit and closest-hit; procedural primitives:
/// intersection, any-hit and closest-hit) and a shader table that selects them.
fn pipeline(variant: &str) -> Pipeline {
    let programs = [
        "raygen",
        "miss",
        "closest_hit",
        "any_hit",
        "intersection",
        "callable",
    ]
    .map(|n| program(n, variant))
    .to_vec();
    let group = |name: &str, kind, intersection: Option<&str>| HitGroup {
        name: name.to_owned(),
        kind,
        any_hit: Some("any_hit".to_owned()),
        closest_hit: Some("closest_hit".to_owned()),
        intersection: intersection.map(str::to_owned),
    };
    Pipeline::new(
        programs,
        ShaderTable {
            miss: vec!["miss".to_owned()],
            // the ray's offset 0 and stride 1 give a hit the group of its instance's contribution
            hit_groups: vec!["triangles".to_owned(), "procedural".to_owned()],
            callable: vec!["callable".to_owned()],
        },
    )
    .unwrap()
    .with_hit_groups([
        group("triangles", HitGroupKind::Triangles, None),
        group(
            "procedural",
            HitGroupKind::ProceduralPrimitives,
            Some("intersection"),
        ),
    ])
}

/// Keeps what the raygen program writes to its image, by texel.
#[derive(Default)]
struct Image {
    texels: BTreeMap<(u32, u32), [f32; 4]>,
}
impl Sampler for Image {
    fn sample(&mut self, _: primase_shader::Sample) -> Result<[f32; 4], String> {
        Err("no sampling".into())
    }
    fn store(&mut self, s: Store) -> Result<(), String> {
        assert_eq!(s.mask, 0xf);
        self.texels
            .insert((s.address[0], s.address[1]), s.value.map(f32::from_bits));
        Ok(())
    }
}

const IDENTITY: [[f32; 4]; 3] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

fn close(got: [f32; 4], want: [f32; 4], what: &str) {
    for k in 0..4 {
        assert!(
            (got[k] - want[k]).abs() <= 1e-5 * want[k].abs().max(1.0),
            "{what}: lane {k}: {got:?}, {want:?}"
        );
    }
}

fn triangle(corners: [[f32; 3]; 3], opaque: bool) -> Geometry {
    Geometry {
        positions: corners.to_vec(),
        boxes: Vec::new(),
        indices: None,
        opaque,
    }
}

fn instance(transform: [[f32; 4]; 3], contribution: u32) -> Instance {
    Instance {
        bottom: 0,
        transform,
        id: 7,
        mask: 0xff,
        contribution,
        flags: InstanceFlags::default(),
    }
}

/// A 2 by 2 dispatch, the ray of pixel (x, y) from ((x + 0.5) / 2, (y + 0.5) / 2, 0) along +z, at an opaque triangle
/// (0,0,5), (1.25,0,5), (0,1.25,5): pixels (0,0), (1,0) and (0,1) hit it and pixel (1,1) misses. A hit runs the
/// closest-hit program: payload.distance = t + t min = 5.001, count = id 7 + instance 0 + primitive 0 + geometry 0 +
/// hit kind 255 (the triangle is seen from its back) + ray flags 0, color = (b1 + 2 ox, b2 + 2 oy + 1, 3 - b1 - b2), b
/// being the barycentric weights (x / 1.25, y / 1.25); the raygen then calls the callable program, which doubles the
/// distance, and writes (color * 2 distance, distance). A miss runs the miss program: color = (0.1, 0.2, 0.3) * the
/// ray direction = (0, 0, 0.3), distance -1, doubled to -2, written (0, 0, -0.6, -1).
#[test]
fn rays_that_hit_run_the_closest_hit_and_rays_that_miss_the_miss_program() {
    for v in ["", ".od"] {
        let pipeline = pipeline(v);
        let resources = PipelineResources {
            constants: BTreeMap::new(),
            scene: Scene {
                bottoms: vec![vec![triangle(
                    [[0.0, 0.0, 5.0], [1.25, 0.0, 5.0], [0.0, 1.25, 5.0]],
                    true,
                )]],
                instances: vec![instance(IDENTITY, 0)],
            },
        };
        let mut image = Image::default();
        let stats = pipeline
            .dispatch_rays("raygen", [2, 2, 1], &resources, &mut image)
            .unwrap_or_else(|e| panic!("{v}: {e}"));
        assert_eq!(stats.rays, 4, "{v}");
        let t = 5.0f32 + 0.001;
        for (x, y) in [(0u32, 0u32), (1, 0), (0, 1)] {
            let (ox, oy) = ((x as f32 + 0.5) / 2.0, (y as f32 + 0.5) / 2.0);
            let (b1, b2) = (ox / 1.25, oy / 1.25);
            let color = [b1 + 2.0 * ox, b2 + 2.0 * oy + 1.0, 3.0 - b1 - b2];
            let value = 2.0 * t;
            close(
                image.texels[&(x, y)],
                [color[0] * value, color[1] * value, color[2] * value, t],
                &format!("{v} pixel ({x}, {y})"),
            );
        }
        close(
            image.texels[&(1, 1)],
            [0.0, 0.0, -0.6, -1.0],
            &format!("{v} pixel (1, 1)"),
        );
    }
}

/// A 4 by 4 dispatch at a non-opaque triangle (0,0,5), (1,0,5), (0,1,5): the hit group's any-hit program ignores a
/// candidate whose first barycentric weight is under 0.25 and otherwise counts it, so the pixels of the column x = 0
/// (ox = 0.125, b1 = 0.125) miss although they are inside, and pixel (1, 0) (ox = 0.375, oy = 0.125) hits with the
/// any-hit's count added to the closest-hit's (7 + 255 + 1 = 263), a count the callable and raygen do not show; the
/// closest-hit's distance and the barycentrics show in the image.
#[test]
fn an_any_hit_program_ignores_or_accepts_the_candidates_of_non_opaque_geometry() {
    for v in ["", ".od"] {
        let pipeline = pipeline(v);
        let resources = PipelineResources {
            constants: BTreeMap::new(),
            scene: Scene {
                bottoms: vec![vec![triangle(
                    [[0.0, 0.0, 5.0], [1.0, 0.0, 5.0], [0.0, 1.0, 5.0]],
                    false,
                )]],
                instances: vec![instance(IDENTITY, 0)],
            },
        };
        let mut image = Image::default();
        pipeline
            .dispatch_rays("raygen", [4, 4, 1], &resources, &mut image)
            .unwrap_or_else(|e| panic!("{v}: {e}"));
        let missed = [0.0, 0.0, -0.6, -1.0];
        // inside the triangle but with b1 < 0.25: ignored, so a miss
        for y in 0..2 {
            close(image.texels[&(0, y)], missed, &format!("{v} pixel (0, {y})"));
        }
        // (1, 0): ox = 0.375, oy = 0.125, b = (0.375, 0.125): accepted
        let t = 5.0f32 + 0.001;
        let (ox, oy, b1, b2) = (0.375f32, 0.125f32, 0.375f32, 0.125f32);
        let color = [b1 + 2.0 * ox, b2 + 2.0 * oy + 1.0, 3.0 - b1 - b2];
        close(
            image.texels[&(1, 0)],
            [color[0] * 2.0 * t, color[1] * 2.0 * t, color[2] * 2.0 * t, t],
            &format!("{v} pixel (1, 0)"),
        );
        // outside the triangle
        close(image.texels[&(3, 3)], missed, &format!("{v} pixel (3, 3)"));
    }
}

/// One ray from (0.5, 0.5, 0) along +z at a procedural primitive: a box (-1,-1,2) to (1,1,3) of an instance translated
/// by (0.5, 0.5, 0), so that the object-space ray starts at the origin. The group's intersection program reports a hit
/// at RayTCurrent + length(ObjectRayOrigin) = 1000 + 0 (the ray's t max, a hit at the very end of the range, which a
/// procedural hit may be: both ends are included), kind 1, attributes (0.5, 0.5); the non-opaque geometry offers it to
/// the any-hit program (which counts it: neither barycentric limit is met) and the closest-hit runs on it: distance
/// 1000 + 0.001, the color made of the attributes (0.5, 0.5) as barycentrics: (0.5 + 0.5, 0.5 + 0.5 + 1,
/// 1 - 0.5 - 0.5 + 1 + 1) = (1, 2, 2).
#[test]
fn an_intersection_program_reports_a_procedural_hit_that_is_offered_to_the_any_hit_and_closest_hit_programs()
{
    for v in ["", ".od"] {
        let pipeline = pipeline(v);
        let box_geometry = Geometry {
            positions: Vec::new(),
            boxes: vec![[[-1.0, -1.0, 2.0], [1.0, 1.0, 3.0]]],
            indices: None,
            opaque: false,
        };
        let translate = [[1.0, 0.0, 0.0, 0.5], [0.0, 1.0, 0.0, 0.5], [0.0, 0.0, 1.0, 0.0]];
        let resources = PipelineResources {
            constants: BTreeMap::new(),
            scene: Scene {
                bottoms: vec![vec![box_geometry]],
                // the instance's contribution 1 selects the procedural group
                instances: vec![instance(translate, 1)],
            },
        };
        let mut image = Image::default();
        pipeline
            .dispatch_rays("raygen", [1, 1, 1], &resources, &mut image)
            .unwrap_or_else(|e| panic!("{v}: {e}"));
        let t = 1000.0f32 + 0.001;
        // the closest-hit program saw the instance's transform: ObjectRayOrigin is (0, 0, 0), the object ray's
        // direction (0, 0, 1), ObjectToWorld row 1 (0, 1, 0), WorldToObject row 2 (0, 0, 1), WorldRayOrigin
        // (0.5, 0.5, 0)
        let color = [0.5 + 0.5, 0.5 + 0.5 + 1.0, (1.0 - 0.5 - 0.5) + 1.0 + 1.0];
        close(
            image.texels[&(0, 0)],
            [color[0] * 2.0 * t, color[1] * 2.0 * t, color[2] * 2.0 * t, t],
            v,
        );
    }
}
