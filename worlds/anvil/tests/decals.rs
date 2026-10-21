//! `lower-anvil-decals` on hand-made ground (a level-1 terrain of 128 m nodes, 1 m samples), against the shader's
//! formulas worked here (material-spec §23.2, §23.8): on a plane, each decal's pieces cover its box's footprint
//! exactly (area 4 |X x Y|, its four corners), on the plane, lifted; each vertex's U and V the engine's packing of its
//! window; the bitangent along -Y; the alpha factor `(1 - fade) m / k`; the textures composed. On random heights, every
//! vertex lies on the ground mesh `lower-anvil-terrain` draws with the same choices. And every DDecalModel of the
//! Riders Republic forge (build 25394987) through the pass: 69 by name (the patch's one model replaces the base's
//! MAD_DD_Orange_Line_01A: 70 with it), 16 drawn, 53 of the terrain family left out.
use primase_anvil::components::{
    AnvilDecalModel, AnvilDeferredDecals, AnvilTerrain, AnvilTextureMap, DeferredDecal, TerrainTile,
};
use primase_anvil::passes::{
    CellPoint, FamilyRule, GroundColour, LowerAnvilDecals, LowerAnvilTerrain, Pieces,
};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::ddecal::{DDECAL_MODEL, StoredDecalModel};
use primase_anvil::read::textures::read_texture_map;
use primase_core::frame::SourceFrame;
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{
    ColorSpace, Entity, Instance, Mesh, NormalGreen, Semantic, Stream, TexFormat, TexRole, Texture,
    Transform, World,
};
use primase_testdata::real_data;

const FRAME: SourceFrame = SourceFrame::ENU;
/// A height value's metres: range 2^23 at level 1 (2^23 x 2^(1 - 28)).
const UNIT: f64 = 1.0 / 16.0;
const LIFT: f64 = 0.01;
const CUTOFF: f64 = 0.4;

/// A level-1 terrain whose sample (gx, gy) has value `height(gx, gy)`.
fn terrain(w: &mut World, height: impl Fn(i64, i64) -> i32) {
    let mut tiles = Vec::new();
    for row in 0..2u32 {
        for column in 0..2u32 {
            let mut heights = vec![0i32; 132 * 132];
            for i in 0..132 {
                for j in 0..132 {
                    // core sample (i - 2, j - 2) at index (i, j)
                    let gy = i64::from(row * 128) + i as i64 - TerrainTile::CORE_AT as i64;
                    let gx = i64::from(column * 128) + j as i64 - TerrainTile::CORE_AT as i64;
                    heights[i * 132 + j] = height(gx, gy);
                }
            }
            tiles.push(TerrainTile {
                row,
                column,
                heights,
                colour: vec![0; 33 * 33 * 8],
                material: Vec::new(),
                block_8: Vec::new(),
            });
        }
    }
    let e = w.spawn();
    w.insert(
        e,
        AnvilTerrain {
            frame: FRAME,
            level: 1,
            origin: [0.0, 0.0],
            side: 256.0,
            tiles,
            materials: Vec::new(),
            details: Vec::new(),
            noise: None,
        },
    );
}

fn texture(w: &mut World, rgba: Vec<u8>, srgb: bool) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Texture {
            width: 4,
            height: 2,
            format: TexFormat::Rgba8Unorm,
            levels: vec![rgba],
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        },
    );
    w.insert(
        e,
        AnvilTextureMap {
            index_id: 0,
            code: 0,
            field_25: u32::from(srgb),
        },
    );
    e
}

/// A model of the box family (its FOR_TERRAIN bytes 1, 1) or the terrain's (0, 0), its maps' texels distinct.
fn model(w: &mut World, terrain: bool) -> (Entity, [Entity; 3]) {
    let diffuse = texture(w, (0..32).map(|k| 10 + k as u8).collect(), true);
    let normal = texture(w, (0..32).map(|k| 100 + k as u8).collect(), false);
    let mask = texture(w, (0..32).map(|k| 200 + k as u8).collect(), false);
    let mut flags = [1u8; 20];
    if terrain {
        (flags[6], flags[7]) = (0, 0);
    }
    let e = w.spawn();
    w.insert(
        e,
        AnvilDecalModel {
            textures: [Some(diffuse), Some(normal), None, Some(mask)],
            opacity: 1.0,
            byte: 0,
            flags,
            parameters: vec![0; 53],
        },
    );
    (e, [diffuse, normal, mask])
}

/// A decal at `centre` (x, y; z on the plane) with half-axes X = (1.2, 0.9, 0) and Y = (-0.3, 0.4, 0), the window
/// (0.25, 0.5), its fade.
fn decal(centre: [f32; 3], fade: f32) -> DeferredDecal {
    let (x, y) = ([1.2f32, 0.9, 0.0], [-0.3f32, 0.4, 0.0]);
    let reach = [1.5f32, 1.3, 1.0];
    DeferredDecal {
        axis_x: x,
        axis_y: y,
        centre,
        bounds: [
            std::array::from_fn(|k| centre[k] - reach[k]),
            std::array::from_fn(|k| centre[k] + reach[k]),
        ],
        window: [0.25, 0.5],
        fade,
        record: [0; 100],
    }
}

fn decals(w: &mut World, model: Entity, decals: Vec<DeferredDecal>) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        AnvilDeferredDecals {
            frame: FRAME,
            model: Some(model),
            decals,
        },
    );
    e
}

fn pass() -> LowerAnvilDecals {
    LowerAnvilDecals {
        rect: [0.0, 0.0, 200.0, 200.0],
        step: 1,
        range: f64::from(1u32 << 23),
        offset: 0.0,
        sample: CellPoint::Corner,
        family: FamilyRule::WriteGBufferRoughness,
        cutoff: CUTOFF,
        lift: LIFT,
    }
}

/// The set's drawn vertices in the source frame (its placement added), with their streams.
struct Drawn {
    points: Vec<[f64; 3]>,
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
    colours: Option<Vec<[f32; 4]>>,
    indices: Vec<u32>,
    material: Entity,
}

fn drawn(w: &World, set: Entity) -> Drawn {
    let at = w
        .iter::<Instance>()
        .find(|(_, i)| i.prototype == set)
        .and_then(|(e, _)| w.get::<Transform>(e))
        .expect("the set placed")
        .cols[3];
    let p = &w.get::<Mesh>(set).expect("a mesh").primitives()[0];
    let stream = |s: Semantic| p.streams.get(&s);
    let Some(Stream::F32x3(positions)) = stream(Semantic::Position) else {
        panic!("positions")
    };
    let Some(Stream::F32x3(normals)) = stream(Semantic::Normal) else {
        panic!("normals")
    };
    let Some(Stream::F32x4(tangents)) = stream(Semantic::Tangent) else {
        panic!("tangents")
    };
    let Some(Stream::F32x2(uvs)) = stream(Semantic::TexCoord(0)) else {
        panic!("uvs")
    };
    let colours = match stream(Semantic::Color(0)) {
        Some(Stream::F32x4(c)) => Some(c.clone()),
        None => None,
        _ => panic!("colours"),
    };
    Drawn {
        points: positions
            .iter()
            .map(|q| {
                FRAME
                    .point_to_source(std::array::from_fn(|k| q[k] + at[k]))
                    .map(f64::from)
            })
            .collect(),
        normals: normals.clone(),
        tangents: tangents.clone(),
        uvs: uvs.clone(),
        colours,
        indices: p.indices.clone().expect("indices"),
        material: p.material.expect("a material"),
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[test]
fn decals_on_a_plane_cover_their_footprints_with_the_engines_coordinates() {
    let mut w = World::new();
    // the plane z = (x + 2 y) / 16
    terrain(&mut w, |gx, gy| (gx + 2 * gy) as i32);
    let plane = |x: f64, y: f64| (x + 2.0 * y) * UNIT;
    let (road, [_, normal, _]) = model(&mut w, false);
    let (trail, _) = model(&mut w, true);
    let at = |x: f32, y: f32| [x, y, plane(f64::from(x), f64::from(y)) as f32];
    let centres = [at(40.3, 30.7), at(100.6, 140.2), at(10.05, 150.55)];
    let fades = [0.0f64, 0.5, -0.125];
    let set = decals(
        &mut w,
        road,
        centres
            .iter()
            .zip(fades)
            .map(|(&c, f)| decal(c, f as f32))
            .chain([decal(at(250.0, 10.0), 0.0)])
            .collect(),
    );
    decals(&mut w, trail, vec![decal(at(60.0, 60.0), 0.0); 2]);
    // a model whose every decal is too faded for any texel to pass ((1 - 0.7) 1 = 0.3 under the cutoff): left out
    let (faint, _) = model(&mut w, false);
    decals(&mut w, faint, vec![decal(at(150.0, 50.0), 0.7)]);
    let mut log = PassLog::default();
    pass().run(&mut w, &mut log).expect("lower");
    println!("{log:?}");
    assert_eq!(log.counts["deferred decals drawn"], 3);
    assert_eq!(log.omitted["deferred decals outside the rectangle"], 1);
    assert_eq!(
        log.omitted["deferred decals whose fade leaves no texel visible ((1 - fade) m under the cutoff)"],
        1
    );
    assert!(
        !w.has::<PbrMaterial>(faint),
        "no material for a model nothing of which is drawn"
    );
    assert_eq!(
        log.omitted["deferred decals of the terrain family (PS 353: the terrain's bake)"],
        2
    );
    assert!(
        w.iter::<AnvilTerrain>().next().is_some(),
        "the terrain kept for lower-anvil-terrain"
    );
    assert!(w.iter::<AnvilDecalModel>().next().is_none());

    let d = drawn(&w, set);
    let (x, y) = ([1.2, 0.9, 0.0], [-0.3, 0.4, 0.0]);
    let (xx, yy) = (dot(x, x), dot(y, y));
    // the window (0.25, 0.5) packed by hand: lo = trunc(0.5 x 65535) = 32767, hi = trunc(0.25 x 65535) = 16383
    let (lo, hi) = (32767.0, 16383.0);
    let k = 1.125f64;
    let (mut area, mut corners_seen) = ([0.0f64; 3], [[false; 4]; 3]);
    for (t, tri) in d.indices.as_chunks::<3>().0.iter().enumerate() {
        let v = tri.map(|i| i as usize);
        // which decal: the nearest centre
        let p0 = d.points[v[0]];
        let which = (0..3)
            .min_by(|&a, &b| {
                let da = (p0[0] - f64::from(centres[a][0])).hypot(p0[1] - f64::from(centres[a][1]));
                let db = (p0[0] - f64::from(centres[b][0])).hypot(p0[1] - f64::from(centres[b][1]));
                da.total_cmp(&db)
            })
            .expect("three");
        let c = centres[which].map(f64::from);
        let [a, b, e] = v.map(|i| d.points[i]);
        let z = cross([b[0] - a[0], b[1] - a[1], 0.0], [e[0] - a[0], e[1] - a[1], 0.0])[2];
        assert!(z >= -1e-9, "triangle {t} counter-clockwise seen from up");
        area[which] += z / 2.0;
        for i in v {
            let p = d.points[i];
            // on the plane, lifted
            assert!((p[2] - plane(p[0], p[1]) - LIFT).abs() < 2e-5, "{p:?}");
            let q = [p[0] - c[0], p[1] - c[1], p[2] - LIFT - c[2]];
            let (lx, ly) = (dot(q, x) / xx, dot(q, y) / yy);
            // within the f32 positions' rounding: a few ulps of 1.5e-5 m (at 128 to 256 m) over a half-axis of 0.5 m
            assert!(
                lx.abs() <= 1.0 + 1e-4 && ly.abs() <= 1.0 + 1e-4,
                "decal {which}: lx {lx}, ly {ly} at {p:?}"
            );
            let want = [(lo - hi * lx) / 65536.0, 0.5 + ly / 2.0];
            assert!(
                (f64::from(d.uvs[i][0]) - want[0]).abs() < 1e-5
                    && (f64::from(d.uvs[i][1]) - want[1]).abs() < 1e-5,
                "{:?} against {want:?}",
                d.uvs[i]
            );
            for (n, (sx, sy)) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)]
                .into_iter()
                .enumerate()
            {
                if (lx - sx).abs() < 1e-4 && (ly - sy).abs() < 1e-4 {
                    corners_seen[which][n] = true;
                }
            }
            // the ground's normal, the tangent -X^, the bitangent cross(N, T) w along -Y^ (core frame: ENU's is a
            // rotation)
            let n_source = {
                let g = [-UNIT, -2.0 * UNIT, 1.0];
                let l = dot(g, g).sqrt();
                g.map(|v| v / l)
            };
            let n_core = FRAME.direction(n_source.map(|v| v as f32)).map(f64::from);
            let t_core = FRAME.direction([-0.8, -0.6, 0.0]).map(f64::from);
            for k in 0..3 {
                assert!((f64::from(d.normals[i][k]) - n_core[k]).abs() < 1e-6);
                assert!((f64::from(d.tangents[i][k]) - t_core[k]).abs() < 1e-6);
            }
            let bitangent = cross(n_core, t_core).map(|v| v * f64::from(d.tangents[i][3]));
            let minus_y = FRAME.direction([0.6, -0.8, 0.0]).map(f64::from);
            assert!(dot(bitangent, minus_y) > 0.99, "the bitangent along -Y^");
            // the alpha factor (1 - fade) m / k
            let colours = d.colours.as_ref().expect("COLOR_0: the fades differ");
            let want = (1.0 - fades[which]) / k;
            assert!((f64::from(colours[i][3]) - want).abs() < 1e-6);
            assert_eq!(colours[i][..3], [1.0; 3]);
        }
    }
    let footprint = cross(x, y)[2].abs() * 4.0;
    for which in 0..3 {
        assert!(
            (area[which] - footprint).abs() < 1e-3,
            "decal {which}: area {}",
            area[which]
        );
        assert_eq!(corners_seen[which], [true; 4], "decal {which}'s corners");
    }

    // the material: Diffuse with Mask1's blue, roughness Normal's alpha, the Normal map marked, cut at cutoff / k
    let m = w.get::<PbrMaterial>(d.material).expect("the model's material");
    assert_eq!(d.material, road);
    assert_eq!(
        m.alpha_mode,
        AlphaMode::Mask {
            cutoff: (CUTOFF / k) as f32
        }
    );
    assert_eq!((m.metallic, m.roughness), (0.0, 1.0));
    let base = w
        .get::<Texture>(m.base_color_texture.as_ref().expect("base").texture)
        .expect("texture");
    assert_eq!(base.color_space, ColorSpace::Srgb);
    let want: Vec<u8> = (0..8u8)
        .flat_map(|t| [10 + 4 * t, 11 + 4 * t, 12 + 4 * t, 202 + 4 * t])
        .collect();
    assert_eq!(base.levels[0], want);
    let rough = w
        .get::<Texture>(m.metallic_roughness_texture.as_ref().expect("mr").texture)
        .expect("texture");
    let want: Vec<u8> = (0..8u8).flat_map(|t| [0, 103 + 4 * t, 0, 255]).collect();
    assert_eq!(rough.levels[0], want);
    assert_eq!(m.normal_texture.as_ref().expect("normal").texture, normal);
    let n = w.get::<Texture>(normal).expect("normal");
    assert_eq!(n.role, TexRole::Normal);
    assert_eq!(n.normal.expect("a convention").green, NormalGreen::Up);
}

#[test]
fn decals_lie_on_the_ground_lower_anvil_terrain_draws() {
    let mut w = World::new();
    // heights jumping by up to 0.5 m between samples
    let hash = |gx: i64, gy: i64| ((gx * 7919 + gy * 104_729).rem_euclid(9) * 3 + 1000) as i32;
    terrain(&mut w, hash);
    let (road, _) = model(&mut w, false);
    let z = |x: i64, y: i64| f64::from(hash(x, y)) * UNIT;
    let set = decals(
        &mut w,
        road,
        [[30.3f32, 20.2], [77.7, 128.1], [128.0, 64.5]]
            .iter()
            .map(|&[x, y]| decal([x, y, z(x as i64, y as i64) as f32], 0.0))
            .collect(),
    );
    let mut log = PassLog::default();
    let p = pass();
    p.run(&mut w, &mut log).expect("decals");
    LowerAnvilTerrain {
        rect: p.rect,
        step: p.step,
        range: p.range,
        offset: p.offset,
        sample: p.sample,
        colour: ColorSpace::Srgb,
        collision: None,
        surface: String::new(),
        ground: GroundColour::Tile,
        pieces: Pieces::Whole,
    }
    .run(&mut w, &mut log)
    .expect("terrain");
    assert_eq!(log.counts["deferred decals drawn"], 3);
    // the ground mesh: its vertices by sample, in the source frame
    let ground = w
        .iter::<Instance>()
        .find(|(_, i)| i.prototype != set)
        .map(|(e, i)| (i.prototype, w.get::<Transform>(e).expect("transform").cols[3]))
        .expect("the ground placed");
    let Some(Stream::F32x3(g)) = w.get::<Mesh>(ground.0).expect("ground").primitives()[0]
        .streams
        .get(&Semantic::Position)
        .cloned()
    else {
        panic!("ground positions")
    };
    let side = 200usize + 1;
    let sample = |c: usize, r: usize| -> [f64; 3] {
        let q = g[r * side + c];
        FRAME
            .point_to_source(std::array::from_fn(|k| q[k] + ground.1[k]))
            .map(f64::from)
    };
    let d = drawn(&w, set);
    for p in &d.points {
        // the ground triangle under p: its square, then the side of its diagonal (a, f)
        let (c, r) = (p[0].floor() as usize, p[1].floor() as usize);
        let (fx, fy) = (p[0] - c as f64, p[1] - r as f64);
        let (c, r, fx, fy) = match (fx < 1e-9 && c > 0, fy < 1e-9 && r > 0) {
            // on a grid line: either neighbour's plane holds it; take the square before
            (true, _) => (c - 1, r, 1.0, fy),
            (_, true) => (c, r - 1, fx, 1.0),
            _ => (c, r, fx, fy),
        };
        let [a, b, dd, f] = [
            sample(c, r),
            sample(c + 1, r),
            sample(c, r + 1),
            sample(c + 1, r + 1),
        ];
        // [a, b, f] below the diagonal (fx >= fy), [a, f, d] above
        let height = if fx >= fy {
            a[2] + (b[2] - a[2]) * fx + (f[2] - b[2]) * fy
        } else {
            a[2] + (dd[2] - a[2]) * fy + (f[2] - dd[2]) * fx
        };
        assert!(
            (p[2] - LIFT - height).abs() < 2e-5,
            "{p:?}: the ground at {height}"
        );
    }
}

#[test]
fn every_decal_model_of_the_game_lowers_or_is_left_out_by_its_family() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let mut w = World::new();
    terrain(&mut w, |_, _| 0);
    // each model, read as the Placer reads it, with one decal of its own (the patch's copy of a name replacing the
    // base forge's, as the archive's order says)
    let mut names = std::collections::BTreeSet::new();
    for forge in [0, 1] {
        let entries = archive.forge(forge).expect("forge").names.clone();
        for (entry, n) in entries.iter().enumerate() {
            if n.type_id != DDECAL_MODEL || !names.insert(n.name.clone()) {
                continue;
            }
            let body = archive.resource(At { forge, entry }).expect("model").body;
            let m = StoredDecalModel::parse(&body).expect("a DDecalModel");
            assert_eq!(m.opacity(), 1.0, "m on every model (material-spec §23.8)");
            let textures = m
                .textures
                .map(|t| t.and_then(|t| read_texture_map(&mut archive, &mut w, t.id).expect("a texture")));
            let e = w.spawn();
            w.insert(
                e,
                AnvilDecalModel {
                    textures,
                    opacity: m.opacity(),
                    byte: m.byte,
                    flags: m.flags,
                    parameters: m.parameters,
                },
            );
            decals(&mut w, e, vec![decal([60.0, 60.0, 0.0], 0.0)]);
        }
    }
    assert_eq!(names.len(), 69);
    let mut log = PassLog::default();
    pass().run(&mut w, &mut log).expect("lower");
    println!("{log:?}");
    assert_eq!(log.counts["deferred decal models drawn"], 16);
    assert_eq!(log.counts["deferred decals drawn"], 16);
    assert_eq!(
        log.omitted["deferred decals of the terrain family (PS 353: the terrain's bake)"],
        53
    );
}
