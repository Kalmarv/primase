//! The game's normal maps, read from level 01: which channels hold X and Y, and which way the mesh's tangent frame
//! points, checked against the stored pixels and vertices rather than assumed.
//!
//! A normal map baked from a height field tilts away from the slope: `x = -dh/dx`, `y = -dh/dy` (the surface's own
//! up and right in the image), and a crease or stitch is dark in the diffuse map where it is low. So where the stored
//! channel for X holds `-dh/dx`, it correlates negatively with the diffuse luminance's horizontal gradient and not
//! with its vertical one, and likewise for Y. The vertex data says the other half: the mesh's tangent follows +u and
//! its bitangent `cross(N, T) * w` follows -v (up the image), the frame the shader multiplies the map's X and Y by.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    reason = "a test reads real pixels and vertices at checked sizes"
)]
use primase_core::{Mesh, Semantic, Stream, TexRole, Texture};
use primase_shaunwhite::components::MaterialRef;
use primase_shaunwhite::read::texture::NORMAL_CONVENTION;
use primase_testdata::{Game, install};
use primase_texcodec::{Decoder, TexDecoder};

fn corr(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut ab, mut aa, mut bb) = (0., 0., 0.);
    for (x, y) in a.iter().zip(b) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma).powi(2);
        bb += (y - mb).powi(2);
    }
    ab / (aa * bb).sqrt()
}

/// RGBA8 of level 0, and the same texel looked up in a texture of another size.
fn rgba(t: &Texture) -> Vec<u8> {
    Decoder::default()
        .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
        .unwrap()
}

#[test]
fn x_is_alpha_y_is_green_up_the_image_and_the_bitangent_follows_it() {
    let Some(folder) = install(Game::ShaunWhite) else {
        return;
    };
    let shared = vec![folder.join("DataPC_02.forge"), folder.join("DataPC_04.forge")];
    let (w, _) =
        primase_shaunwhite::read::level::read_level(&folder.join("DataPC_01.forge"), &shared).unwrap();

    // every normal map carries the shader's convention
    let normals: Vec<_> = w
        .iter::<Texture>()
        .filter(|(_, t)| t.role == TexRole::Normal)
        .collect();
    assert_eq!(normals.len(), 17);
    assert!(normals.iter().all(|(_, t)| t.normal == Some(NORMAL_CONVENTION)));

    // the channels, against the diffuse map of the same Material
    let (mut ax, mut ay, mut gx, mut gy, mut used) = (0., 0., 0., 0., 0.);
    let mut seen = std::collections::BTreeSet::new();
    for (_, m) in w.iter::<MaterialRef>() {
        let (Some(n), Some(d)) = (m.slots.get("normal"), m.slots.get("diffuse")) else {
            continue;
        };
        // the characters' maps are baked from height (the water effects' are animated waves, no height to compare)
        if !m.name.starts_with("CH_") || !seen.insert(n.texture) {
            continue;
        }
        let (nt, dt) = (
            w.get::<Texture>(n.texture).unwrap(),
            w.get::<Texture>(d.texture).unwrap(),
        );
        if nt.width < 64 {
            continue;
        }
        let (np, dp) = (rgba(nt), rgba(dt));
        let (width, height) = (nt.width as usize, nt.height as usize);
        // red and blue are not used by the shader: constant in every map
        assert!(np.chunks(4).all(|p| p[0] == np[0] && p[2] == np[2]), "{}", m.name);
        let luma = |x: usize, y: usize| {
            let (dx, dy) = (x * dt.width as usize / width, y * dt.height as usize / height);
            let p = &dp[(dy * dt.width as usize + dx) * 4..];
            (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.
        };
        let (mut alpha, mut green, mut dx, mut dy) = (vec![], vec![], vec![], vec![]);
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let p = &np[(y * width + x) * 4..];
                alpha.push(f64::from(p[3]));
                green.push(f64::from(p[1]));
                dx.push(luma(x + 1, y) - luma(x - 1, y));
                // up the image is toward smaller rows
                dy.push(luma(x, y - 1) - luma(x, y + 1));
            }
        }
        if alpha.iter().all(|a| *a == alpha[0]) {
            continue;
        }
        let [a_x, a_y, g_x, g_y] =
            [(&alpha, &dx), (&alpha, &dy), (&green, &dx), (&green, &dy)].map(|(a, b)| corr(a, b));
        println!(
            "{}: alpha {a_x:+.3}/{a_y:+.3} green {g_x:+.3}/{g_y:+.3} (dx/dy up)",
            m.name
        );
        (ax, ay, gx, gy, used) = (ax + a_x, ay + a_y, gx + g_x, gy + g_y, used + 1.);
    }
    let [ax, ay, gx, gy] = [ax, ay, gx, gy].map(|c| c / used);
    println!(
        "{used} maps: mean corr alpha~dx {ax:+.3} alpha~dy {ay:+.3} green~dx {gx:+.3} green~dy_up {gy:+.3}"
    );
    assert!(used >= 10.);
    assert!(ax < -0.05 && ay.abs() < 0.03, "X is the alpha channel: {ax} {ay}");
    assert!(
        gy < -0.05 && gx.abs() < 0.03,
        "Y is the green channel, up the image: {gx} {gy}"
    );

    // the vertices: tangent along +u, bitangent along -v, for nearly every triangle of every normal-mapped primitive
    let (mut triangles, mut along_u, mut up_v) = (0usize, 0usize, 0usize);
    for (_, mesh) in w.iter::<Mesh>() {
        for p in mesh.primitives() {
            let mapped = p
                .material
                .and_then(|m| w.get::<MaterialRef>(m))
                .is_some_and(|m| m.slots.contains_key("normal") && !m.state.is_some_and(|s| s.water));
            let (
                Some(Stream::F32x3(pos)),
                Some(Stream::F32x3(nor)),
                Some(Stream::F32x2(uv)),
                Some(Stream::F32x4(tan)),
            ) = (
                p.streams.get(&Semantic::Position),
                p.streams.get(&Semantic::Normal),
                p.streams.get(&Semantic::TexCoord(0)),
                p.streams.get(&Semantic::Tangent(0)),
            )
            else {
                assert!(!mapped, "a normal-mapped primitive without tangents");
                continue;
            };
            if !mapped {
                continue;
            }
            for t in p.indices.as_ref().unwrap().chunks(3) {
                let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
                let sub = |p: [f32; 3], q: [f32; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                let (e1, e2) = (sub(pos[b], pos[a]), sub(pos[c], pos[a]));
                let (du1, dv1) = (uv[b][0] - uv[a][0], uv[b][1] - uv[a][1]);
                let (du2, dv2) = (uv[c][0] - uv[a][0], uv[c][1] - uv[a][1]);
                let det = du1 * dv2 - du2 * dv1;
                if det.abs() < 1e-12 {
                    continue;
                }
                let dot = |p: [f32; 3], q: [f32; 3]| p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
                // the directions of increasing u and v on the surface
                let d_u: [f32; 3] = std::array::from_fn(|k| (e1[k] * dv2 - e2[k] * dv1) / det);
                let d_v: [f32; 3] = std::array::from_fn(|k| (e2[k] * du1 - e1[k] * du2) / det);
                let (tv, n) = (tan[a], nor[a]);
                let bitangent: [f32; 3] = [
                    (n[1] * tv[2] - n[2] * tv[1]) * tv[3],
                    (n[2] * tv[0] - n[0] * tv[2]) * tv[3],
                    (n[0] * tv[1] - n[1] * tv[0]) * tv[3],
                ];
                triangles += 1;
                along_u += usize::from(dot([tv[0], tv[1], tv[2]], d_u) > 0.);
                up_v += usize::from(dot(bitangent, d_v) < 0.);
            }
        }
    }
    println!("{triangles} triangles: tangent along +u {along_u}, bitangent along -v {up_v}");
    assert!(triangles > 10_000);
    assert!(along_u * 100 >= triangles * 99 && up_v * 100 >= triangles * 99);
}
