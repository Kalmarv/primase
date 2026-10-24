//! `bleed-alpha-colors` through the pass manager, and the distance transform it rests on against a brute-force
//! nearest-texel search written here.
#![allow(clippy::unwrap_used)]

use primase_core::ledger::Conservation;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{ColorSpace, Entity, Name, TexFormat, TexRole, Texture, TextureFile, World};
use primase_image::{BleedAlphaColors, mip_chain_rgba8, nearest_visible};

fn run(w: &mut World) -> Result<RunReport, String> {
    let mut registry = PassRegistry::new();
    primase_image::register(&mut registry);
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_pass_checks(true)
        .with_conservation(Conservation::Enforce)
        .run(
            w,
            &Pipeline {
                passes: vec![Box::new(BleedAlphaColors)],
            },
            &mut report,
        )
        .map_err(|e| e.to_string())?;
    Ok(report)
}

/// A small deterministic generator (no randomness crate): a mask of `density` percent visible texels.
fn pseudo_mask(seed: u64, n: usize, density: u64) -> Vec<bool> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (s >> 33) % 100 < density
        })
        .collect()
}

#[test]
fn the_nearest_visible_texel_is_the_euclidean_nearest() {
    for (seed, w, h, density) in [
        (1, 17, 13, 5),
        (2, 1, 30, 10),
        (3, 30, 1, 10),
        (4, 24, 24, 1),
        (5, 40, 31, 30),
    ] {
        let visible = pseudo_mask(seed, w * h, density);
        let nearest = nearest_visible(&visible, w, h);
        let sites: Vec<(i64, i64)> = (0..w * h)
            .filter(|&i| visible[i])
            .map(|i| ((i % w) as i64, (i / w) as i64))
            .collect();
        for y in 0..h {
            for x in 0..w {
                let n = nearest[y * w + x];
                if sites.is_empty() {
                    assert_eq!(n, u32::MAX);
                    continue;
                }
                let best = sites
                    .iter()
                    .map(|&(sx, sy)| (sx - x as i64).pow(2) + (sy - y as i64).pow(2))
                    .min()
                    .unwrap();
                let n = n as usize;
                assert!(
                    visible[n],
                    "({x}, {y}) of {w}x{h} seed {seed}: {n} is not visible"
                );
                let d = ((n % w) as i64 - x as i64).pow(2) + ((n / w) as i64 - y as i64).pow(2);
                assert_eq!(d, best, "({x}, {y}) of {w}x{h} seed {seed}");
            }
        }
    }
}

/// An RGBA8 texture of `n` by `n` texels with `levels` mip levels (box mips of the first): a visible (alpha 255)
/// vertical band at columns `band`, hidden (alpha 10) elsewhere, the colour red on the band and white where hidden.
fn banded(w: &mut World, n: usize, band: std::ops::Range<usize>, levels: usize) -> Entity {
    let mut px = Vec::new();
    for _ in 0..n {
        for x in 0..n {
            px.extend(if band.contains(&x) {
                [200, 20, 20, 255]
            } else {
                [255, 255, 255, 10]
            });
        }
    }
    let chain = mip_chain_rgba8(n as u32, n as u32, &px).unwrap();
    let e = w.spawn();
    w.insert(
        e,
        Texture {
            width: n as u32,
            height: n as u32,
            format: TexFormat::Rgba8Unorm,
            levels: chain.into_iter().take(levels).map(|(_, _, p)| p).collect(),
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    w.insert(e, Name("hair".into()));
    w.insert(
        e,
        TextureFile {
            path: "hair.png".into(),
        },
    );
    e
}

fn material(w: &mut World, tex: Entity, mode: AlphaMode, factor: f32) -> Entity {
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            base_color: [1.0, 1.0, 1.0, factor],
            base_color_texture: Some(TextureRef::new(tex)),
            alpha_mode: mode,
            ..PbrMaterial::default()
        },
    );
    m
}

fn texel(t: &Texture, level: usize, x: usize, y: usize) -> [u8; 4] {
    let n = (t.width as usize >> level).max(1);
    let i = (y * n + x) * 4;
    t.levels[level][i..i + 4].try_into().unwrap()
}

#[test]
fn hidden_colour_becomes_the_nearest_visible_and_alpha_is_untouched() {
    let mut w = World::new();
    let tex = banded(&mut w, 8, 3..5, 1);
    material(&mut w, tex, AlphaMode::Mask { cutoff: 0.5 }, 1.0);
    let before = w.get::<Texture>(tex).unwrap().clone();
    let report = run(&mut w).unwrap();
    let t = w.get::<Texture>(tex).unwrap();
    for y in 0..8 {
        for x in 0..8 {
            let b = texel(&before, 0, x, y);
            let a = texel(t, 0, x, y);
            assert_eq!(a[3], b[3], "alpha at ({x}, {y})");
            if (3..5).contains(&x) {
                assert_eq!(a, b, "visible texel ({x}, {y}) changed");
            } else {
                assert_eq!(a[..3], [200, 20, 20], "hidden texel ({x}, {y})");
            }
        }
    }
    let log = &report.steps[0].log;
    assert_eq!(log.counts["hidden texels"], 8 * 6);
    assert_eq!(log.counts["hidden texels given another colour"], 8 * 6);
    assert_eq!(log.counts["textures changed in place"], 1);
    assert!(
        !w.has::<TextureFile>(tex),
        "the file link goes with the changed pixels"
    );
    assert!(w.verify_components().is_empty());
}

/// The mips stored with the texture were made from the hidden colour: they are rebuilt from the changed first level.
#[test]
fn mips_are_rebuilt_from_the_changed_level() {
    let mut w = World::new();
    let tex = banded(&mut w, 8, 3..5, 4);
    material(&mut w, tex, AlphaMode::Mask { cutoff: 0.5 }, 1.0);
    let report = run(&mut w).unwrap();
    let t = w.get::<Texture>(tex).unwrap();
    assert_eq!(t.levels.len(), 4);
    let expected = mip_chain_rgba8(8, 8, &t.levels[0]).unwrap();
    for (k, (_, _, p)) in expected.iter().take(4).enumerate() {
        assert_eq!(&t.levels[k], p, "level {k}");
    }
    assert_eq!(
        report.steps[0].log.approximations["textures with their mip levels rebuilt"],
        1
    );
}

/// A texture another material draws opaque (or a second cutoff) is not changed for them: the masked users get a copy.
#[test]
fn a_shared_texture_is_copied_for_its_masked_users() {
    let mut w = World::new();
    let tex = banded(&mut w, 8, 3..5, 1);
    let masked = material(&mut w, tex, AlphaMode::Mask { cutoff: 0.5 }, 1.0);
    let opaque = material(&mut w, tex, AlphaMode::Opaque, 1.0);
    let before = w.get::<Texture>(tex).unwrap().clone();
    let report = run(&mut w).unwrap();
    assert_eq!(w.get::<Texture>(tex).unwrap(), &before, "the original is kept");
    assert_eq!(
        w.get::<PbrMaterial>(opaque)
            .unwrap()
            .base_color_texture
            .unwrap()
            .texture,
        tex
    );
    let copy = w
        .get::<PbrMaterial>(masked)
        .unwrap()
        .base_color_texture
        .unwrap()
        .texture;
    assert_ne!(copy, tex);
    assert_eq!(w.get::<Name>(copy).unwrap().0, "hair~bleed0.5");
    assert_eq!(
        texel(w.get::<Texture>(copy).unwrap(), 0, 0, 0)[..3],
        [200, 20, 20]
    );
    assert_eq!(
        report.steps[0].log.counts["textures copied for their masked uses"],
        1
    );

    // two cutoffs on one texture: a copy for each, the original untouched
    let mut w = World::new();
    let tex = banded(&mut w, 8, 3..5, 1);
    let a = material(&mut w, tex, AlphaMode::Mask { cutoff: 0.5 }, 1.0);
    let b = material(&mut w, tex, AlphaMode::Mask { cutoff: 0.25 }, 1.0);
    run(&mut w).unwrap();
    let use_of = |m: Entity| {
        w.get::<PbrMaterial>(m)
            .unwrap()
            .base_color_texture
            .unwrap()
            .texture
    };
    assert_ne!(use_of(a), use_of(b));
    assert_ne!(use_of(a), tex);
    assert_eq!(w.get::<Texture>(tex).unwrap().levels[0][..4], [255, 255, 255, 10]);
}

/// The cutoff is tested against the base-colour alpha times the texel's alpha, as the clipping does: at 0.5 the
/// band's 255 is 127.5/255 = 0.5, still visible, and at 0.4 it is hidden together with everything else.
#[test]
fn the_base_colour_alpha_moves_the_threshold() {
    let mut w = World::new();
    let tex = banded(&mut w, 8, 3..5, 1);
    material(&mut w, tex, AlphaMode::Mask { cutoff: 0.5 }, 0.4);
    let before = w.get::<Texture>(tex).unwrap().clone();
    let report = run(&mut w).unwrap();
    assert_eq!(w.get::<Texture>(tex).unwrap(), &before);
    assert_eq!(
        report.steps[0].log.counts["textures under the cutoff everywhere, left as they are"],
        1
    );
}

#[test]
fn nothing_hidden_changes_nothing_and_other_modes_are_left() {
    let mut w = World::new();
    let tex = banded(&mut w, 8, 0..8, 1);
    material(&mut w, tex, AlphaMode::Mask { cutoff: 0.5 }, 1.0);
    let tex2 = banded(&mut w, 8, 3..5, 1);
    material(&mut w, tex2, AlphaMode::Blend, 1.0);
    let before = (
        w.get::<Texture>(tex).unwrap().clone(),
        w.get::<Texture>(tex2).unwrap().clone(),
    );
    let report = run(&mut w).unwrap();
    assert_eq!(w.get::<Texture>(tex).unwrap(), &before.0);
    assert_eq!(w.get::<Texture>(tex2).unwrap(), &before.1);
    assert_eq!(
        report.steps[0].log.counts["textures with no texel under the cutoff, left as they are"],
        1
    );
}

#[test]
fn refusals() {
    let mut w = World::new();
    let tex = w.spawn();
    material(&mut w, tex, AlphaMode::Mask { cutoff: 0.5 }, 1.0);
    assert!(run(&mut w).unwrap_err().contains("has no pixels"));
    let mut w = World::new();
    let tex = banded(&mut w, 8, 3..5, 1);
    let mut t = w.get::<Texture>(tex).unwrap().clone();
    t.format = TexFormat::Rgba16Unorm;
    t.levels = vec![vec![0; 8 * 8 * 8]];
    w.insert(tex, t);
    material(&mut w, tex, AlphaMode::Mask { cutoff: 0.5 }, 1.0);
    assert!(run(&mut w).unwrap_err().contains("not exact in 8-bit RGBA"));
}
