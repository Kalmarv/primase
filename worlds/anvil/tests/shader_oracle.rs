//! The shader oracle: each template `lower-anvil-material` lowers has its own pixel shader run in primase-dxbc (the
//! container its MaterialTemplate body holds, unmodified) on random inputs, and its outputs compared with the formulas
//! the pass makes its textures of ([`primase_anvil::passes::formulas`]). The method is the Riders Republic notes'
//! (docs/material-spec.md §8: uniform textures, chosen constants and interpolants, decoys in the constants the shader
//! shouldn't read), the executor primase's own; the permutations are the notes' (their opaque G-buffer one per
//! template). Each test asserts its largest differences under 1e-6 and prints them.

use primase_anvil::passes::formulas::{Rough, emission, metalness, occlusion, roughness, sat, tint, tinted};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::template;
use primase_core::layered::height_blend;
use primase_shader_exec::ProgramShader;
use primase_shader_exec::{Load, PixelShader, Sample, Sampler, Store};
use primase_testdata::{Game, install};

/// A small deterministic generator (xorshift64*), so every run checks the same cases.
struct Random(u64);

impl Random {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform in `lo..hi`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
    fn texel(&mut self) -> [f64; 4] {
        [self.next(), self.next(), self.next(), self.next()]
    }
    /// `rows` rows of `cb3`, every lane a decoy; the test sets the ones its shader reads.
    fn decoys(&mut self, rows: usize) -> Vec<[f64; 4]> {
        (0..rows).map(|_| self.texel()).collect()
    }
}

/// The program lifted and ready to run.
fn executor(container: &[u8]) -> ProgramShader {
    let program = primase_dxbc::lift(container).unwrap_or_else(|e| panic!("the shader lifts: {e}"));
    ProgramShader::new(program).unwrap_or_else(|e| panic!("the shader runs: {e}"))
}

/// The pixel shader at body offset `offset` of the MaterialTemplate at entry `entry` of the forge file `forge`.
fn shader(forge: &str, entry: usize, offset: usize) -> Option<ProgramShader> {
    let dir = install(Game::Riders)?;
    let mut archive = Archive::open(&[dir.join(forge)]).expect("archive");
    let body = archive.resource(At { forge: 0, entry }).expect("template").body;
    let all = template::shaders(&body);
    let (_, container) = all.iter().find(|(at, _)| *at == offset).unwrap_or_else(|| {
        panic!(
            "no container at {offset:#x}; at {:x?}",
            all.iter().map(|(a, _)| a).collect::<Vec<_>>()
        )
    });
    Some(executor(container))
}

fn bits(v: [f64; 4]) -> [u32; 4] {
    v.map(|x| (x as f32).to_bits())
}

fn float(v: u32) -> f64 {
    f64::from(f32::from_bits(v))
}

/// A scalar in a cb3 row's first lane, the rest decoys kept.
fn set(cb3: &mut [[f64; 4]], row: usize, v: f64) {
    cb3[row][0] = v;
}

/// Identity UV rows at `row` and `row + 1`.
fn uv_rows(cb3: &mut [[f64; 4]], row: usize) {
    cb3[row] = [1.0, 0.0, 0.0, 0.0];
    cb3[row + 1] = [0.0, 1.0, 0.0, 0.0];
}

/// Uniform textures by slot: every sample and load of one the value given; one not given (an engine texture the case
/// leaves unset, whose value its outputs mustn't depend on) samples a fixed decoy and loads zeros; structured words
/// zero, stores dropped (they reach no output).
struct Uniform<'a>(&'a [(u32, [f64; 4])]);

impl Sampler for Uniform<'_> {
    fn sample(&mut self, r: Sample) -> Result<[f32; 4], String> {
        self.0
            .iter()
            .find(|(t, _)| *t == r.texture)
            .map_or(Ok([0.37, 0.61, 0.19, 0.83]), |(_, v)| Ok(v.map(|x| x as f32)))
    }
    fn load(&mut self, r: Load) -> Result<[u32; 4], String> {
        Ok(self
            .0
            .iter()
            .find(|(t, _)| *t == r.texture)
            .map_or([0; 4], |(_, v)| bits(*v)))
    }
    fn structured(&mut self, _slot: u32, _element: u32, _offset: u32) -> Result<u32, String> {
        Ok(0)
    }
    fn store(&mut self, _request: Store) -> Result<(), String> {
        Ok(())
    }
}

/// The shader's outputs for one case: uniform textures by slot, `cb3`'s rows, the vertex colour, and `engine`'s rows
/// (slot, row, value) of the other constant buffers, all else zeros (no LOD bias, no instance position); the frame
/// inputs a fixed plausible frame.
fn run_with(
    s: &ProgramShader,
    textures: &[(u32, [f64; 4])],
    cb3: &[[f64; 4]],
    colour: [f64; 4],
    engine: &[(u32, usize, [f64; 4])],
) -> Vec<[f64; 4]> {
    // by register: the interface lists the whole signature
    let mut inputs = vec![[0u32; 4]; s.input_count()];
    for i in &s.interface().inputs {
        let register = i.provenance.d3d.expect("a D3D register").register;
        inputs[register as usize] = match (i.label.name.as_str(), register) {
            ("SV_Position", _) => bits([0.5, 0.5, 0.5, 1.0]),
            ("COLOR", _) if i.label.index == 0 => bits(colour),
            // a second vertex colour (MAD_Grounds' tire scalar, COLOR1.x): 1s
            ("COLOR", _) => bits([1.0; 4]),
            ("SV_IsFrontFace", _) => [u32::MAX; 4],
            (_, 1) => bits([0.3, 0.6, 0.0, 0.0]),
            (_, r) if r % 2 == 1 => bits([1.0, 0.0, 0.0, 0.0]),
            _ => bits([0.0, 0.0, 1.0, 1.0]),
        };
    }
    let buffers: Vec<(u32, Vec<[u32; 4]>)> = s
        .constant_buffers()
        .iter()
        .map(|&(slot, size)| {
            let rows = (0..size as usize)
                .map(|r| {
                    if let Some((_, _, v)) = engine.iter().find(|(b, row, _)| *b == slot && *row == r) {
                        bits(*v)
                    } else if slot == 3 {
                        cb3.get(r).copied().map_or([0; 4], bits)
                    } else {
                        [0; 4]
                    }
                })
                .collect();
            (slot, rows)
        })
        .collect();
    let views: Vec<(u32, &[[u32; 4]])> = buffers.iter().map(|(s, v)| (*s, v.as_slice())).collect();
    let e = s
        .execute(&inputs, &views, &mut Uniform(textures))
        .expect("the shader runs");
    assert!(!e.discarded, "the case discarded");
    e.outputs.into_iter().map(|o| o.map(float)).collect()
}

/// [`run_with`] with no engine constants.
fn run(s: &ProgramShader, textures: &[(u32, [f64; 4])], cb3: &[[f64; 4]], colour: [f64; 4]) -> Vec<[f64; 4]> {
    run_with(s, textures, cb3, colour, &[])
}

/// The largest difference of each named quantity.
#[derive(Default)]
struct Worst(Vec<(&'static str, f64)>);

impl Worst {
    fn see(&mut self, what: &'static str, got: f64, want: f64) {
        let d = (got - want).abs();
        match self.0.iter_mut().find(|(w, _)| *w == what) {
            Some((_, m)) => *m = m.max(d),
            None => self.0.push((what, d)),
        }
    }
    fn rgb(&mut self, what: &'static str, got: [f64; 4], want: [f64; 3]) {
        for k in 0..3 {
            self.see(what, got[k], want[k]);
        }
    }
    /// Prints them, and asserts each under 1e-6.
    fn check(&self, template: &str) {
        println!("{template}: {:?}", self.0);
        for (what, d) in &self.0 {
            assert!(*d < 1e-6, "{template} {what}: {d}");
        }
    }
}

fn rgb(t: [f64; 4]) -> [f64; 3] {
    [t[0], t[1], t[2]]
}

/// MAD_Tex1 (DataPC 61047, at 0x0B8247): t75 normal map, t76 diffuse, t77 mask; cb3[1..7] Metalness, Hue,
/// Saturation, Luminosity, Roughness Offset, On Off AO, Vertex Color Intensity (0 or 1, the values the pass lowers);
/// cb3[8..9] the UV rows. o0 = (albedo, roughness), o1.w metalness, o3.x AO.
#[test]
fn mad_tex1() {
    let Some(s) = shader("DataPC.forge", 61047, 0x0B_8247) else {
        return;
    };
    let mut r = Random(0x0061_0470_000B_8247);
    let mut worst = Worst::default();
    for case in 0..40 {
        let (normal, diffuse, mask, colour) = (r.texel(), r.texel(), r.texel(), r.texel());
        let (metal, offset, ao_on) = (r.next(), r.range(-0.5, 0.5), r.next());
        let hsl = [r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)];
        let vci = f64::from(case % 2);
        let mut cb3 = r.decoys(10);
        for (row, v) in [metal, hsl[0], hsl[1], hsl[2], offset, ao_on, vci]
            .into_iter()
            .enumerate()
        {
            set(&mut cb3, row + 1, v);
        }
        uv_rows(&mut cb3, 8);
        let out = run(&s, &[(75, normal), (76, diffuse), (77, mask)], &cb3, colour);
        let albedo = tinted(rgb(diffuse), mask[0], hsl);
        let factor = |k: usize| if vci == 1.0 { colour[k] } else { 1.0 };
        worst.rgb("albedo", out[0], [0, 1, 2].map(|k| albedo[k] * factor(k)));
        worst.see(
            "roughness",
            out[0][3],
            roughness(normal[3], Rough::Offset(offset)),
        );
        worst.see("metalness", out[1][3], metalness(normal[2], Some(metal)));
        worst.see("occlusion", out[3][0], occlusion(mask[2], ao_on));
    }
    worst.check("MAD_Tex1");
}

/// MAD_Fake (DataPC 62269, at 0x0436A1): t75 albedo, t76 normal map; no constants. o0 = (albedo, the normal map's
/// alpha), o1.w its blue, o3.x 1.
#[test]
fn mad_fake() {
    let Some(s) = shader("DataPC.forge", 62269, 0x04_36A1) else {
        return;
    };
    let mut r = Random(0x0062_2690_0004_36A1);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (albedo, normal) = (r.texel(), r.texel());
        let out = run(&s, &[(75, albedo), (76, normal)], &[], r.texel());
        worst.rgb("albedo", out[0], rgb(albedo));
        worst.see("roughness", out[0][3], roughness(normal[3], Rough::Raw));
        worst.see("metalness", out[1][3], metalness(normal[2], None));
        worst.see("occlusion", out[3][0], 1.0);
    }
    worst.check("MAD_Fake");
}

/// MAD_Decals1 (DataPC 42665, at 0x0405CC): t75 normal map, t76 diffuse; cb3[1] Metalness, [2..4] Hue, Saturation,
/// Luminosity, [5] Roughness Offset (its CB3 6 rows). The tint unmasked.
#[test]
fn mad_decals1() {
    let Some(s) = shader("DataPC.forge", 42665, 0x04_05CC) else {
        return;
    };
    let mut r = Random(0x0042_6650_0004_05CC);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (normal, diffuse) = (r.texel(), r.texel());
        let (metal, offset) = (r.next(), r.range(-0.5, 0.5));
        let hsl = [r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)];
        let mut cb3 = r.decoys(6);
        for (row, v) in [metal, hsl[0], hsl[1], hsl[2], offset].into_iter().enumerate() {
            set(&mut cb3, row + 1, v);
        }
        let out = run(&s, &[(75, normal), (76, diffuse)], &cb3, r.texel());
        worst.rgb("albedo", out[0], tinted(rgb(diffuse), 1.0, hsl));
        worst.see(
            "roughness",
            out[0][3],
            roughness(normal[3], Rough::Offset(offset)),
        );
        worst.see("metalness", out[1][3], metalness(normal[2], Some(metal)));
    }
    worst.check("MAD_Decals1");
}

/// MAD_Tex1_Wind (DataPC 62255, at 0x0B96A2): t75 normal map, t76 diffuse, t77 mask; cb3[1..13] the vertex shader's
/// wind (decoys here), [14..16] Hue, Saturation, Luminosity, [17] On Off AO, [18] Translucency, [19..20] the UV rows.
/// Roughness and metalness the normal map's alpha and blue raw.
#[test]
fn mad_tex1_wind() {
    let Some(s) = shader("DataPC.forge", 62255, 0x0B_96A2) else {
        return;
    };
    let mut r = Random(0x0062_2550_000B_96A2);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (normal, diffuse, mask) = (r.texel(), r.texel(), r.texel());
        let hsl = [r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)];
        let ao_on = r.next();
        let mut cb3 = r.decoys(21);
        for (row, v) in [hsl[0], hsl[1], hsl[2], ao_on].into_iter().enumerate() {
            set(&mut cb3, row + 14, v);
        }
        uv_rows(&mut cb3, 19);
        let out = run(&s, &[(75, normal), (76, diffuse), (77, mask)], &cb3, r.texel());
        worst.rgb("albedo", out[0], tinted(rgb(diffuse), mask[0], hsl));
        worst.see("roughness", out[0][3], roughness(normal[3], Rough::Raw));
        worst.see("metalness", out[1][3], metalness(normal[2], None));
        worst.see("occlusion", out[3][0], occlusion(mask[2], ao_on));
    }
    worst.check("MAD_Tex1_Wind");
}

/// MAD_Bark (USNAT 16714, at 0x052F93; layout B): t75 wind noise, t76 normal map, t77 diffuse, t78 mask; cb3[15]
/// Metalness (read by no pixel shader), [16] On Off AO, [17] Wind Debug (0), [18..19] the UV rows. o0 = (albedo,
/// the normal map's alpha), o1.w AO times the vertex alpha: 1 here, the factor the pass leaves out.
#[test]
fn mad_bark() {
    let Some(s) = shader("DataPC_MAD_USNAT.forge", 16714, 0x05_2F93) else {
        return;
    };
    let mut r = Random(0x0016_7140_0005_2F93);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (noise, normal, diffuse, mask) = (r.texel(), r.texel(), r.texel(), r.texel());
        let ao_on = r.next();
        let mut cb3 = r.decoys(20);
        set(&mut cb3, 16, ao_on);
        cb3[17] = [0.0; 4];
        uv_rows(&mut cb3, 18);
        let colour = [r.next(), r.next(), r.next(), 1.0];
        let out = run(
            &s,
            &[(75, noise), (76, normal), (77, diffuse), (78, mask)],
            &cb3,
            colour,
        );
        worst.rgb("albedo", out[0], rgb(diffuse));
        worst.see("roughness", out[0][3], roughness(normal[3], Rough::Raw));
        worst.see("occlusion", out[1][3], occlusion(mask[2], ao_on));
    }
    worst.check("MAD_Bark");
}

/// MAD_Tex1_Leaves (USNAT 16742, at 0x08E817; layout B): t75 wind noise, t76 normal map, t77 diffuse, t78 Mask 1;
/// cb3[22] Saturate, [23] Hue, [25] Vertex AO, [26] Color Tint Debug (0: the tint everywhere), [27] On Off AO, [28]
/// Luminosity, [32] Wind Debug (0), [33..34] the UV rows. Vertex AO at least 1, so `sat(COLOR0.a + Vertex AO)` is 1
/// whatever the vertex alpha (the pass's claim); o1.w's vertex-alpha factor, which the pass leaves out, at alpha 1.
#[test]
fn mad_tex1_leaves() {
    let Some(s) = shader("DataPC_MAD_USNAT.forge", 16742, 0x08_E817) else {
        return;
    };
    let mut r = Random(0x0016_7420_0008_E817);
    let mut worst = Worst::default();
    for case in 0..40 {
        let (noise, normal, diffuse, mask) = (r.texel(), r.texel(), r.texel(), r.texel());
        let (saturation, hue, luminosity) = (r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5));
        let (vertex_ao, ao_on) = (r.range(1.0, 2.0), r.next());
        let mut cb3 = r.decoys(35);
        for (row, v) in [
            (22, saturation),
            (23, hue),
            (25, vertex_ao),
            (26, 0.0),
            (27, ao_on),
            (28, luminosity),
        ] {
            set(&mut cb3, row, v);
        }
        cb3[32] = [0.0; 4];
        uv_rows(&mut cb3, 33);
        // the vertex alpha: any (albedo) on even cases, 1 (AO compared) on odd ones
        let alpha = if case % 2 == 0 { r.next() } else { 1.0 };
        let out = run(
            &s,
            &[(75, noise), (76, normal), (77, diffuse), (78, mask)],
            &cb3,
            [0.0, 0.0, 0.0, alpha],
        );
        worst.rgb("albedo", out[0], tint(rgb(diffuse), hue, saturation, luminosity));
        worst.see("roughness (mask green)", out[0][3], mask[1]);
        if alpha == 1.0 {
            worst.see("occlusion", out[1][3], occlusion(mask[2], ao_on));
        }
    }
    worst.check("MAD_Tex1_Leaves");
}

/// MAD_Clutters_Grass (USNAT 256868, at 0x054B67; layout B): t75 mask, t76 alpha, t77 diffuse, t78 normal map, t79
/// the tint noise; cb3[1..9] the vertex shader's (decoys), [13] Value, [14] Saturation, [15] the noise tint's On-Off
/// (0: the pass's exact case), [16] On Off AO, [17] Hue.
#[test]
fn mad_clutters_grass() {
    let Some(s) = shader("DataPC_MAD_USNAT.forge", 256_868, 0x05_4B67) else {
        return;
    };
    let mut r = Random(0x0025_6868_0005_4B67);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (mask, alpha, diffuse, normal, noise) = (r.texel(), r.texel(), r.texel(), r.texel(), r.texel());
        let ao_on = r.next();
        let mut cb3 = r.decoys(18);
        set(&mut cb3, 15, 0.0);
        set(&mut cb3, 16, ao_on);
        let out = run(
            &s,
            &[(75, mask), (76, alpha), (77, diffuse), (78, normal), (79, noise)],
            &cb3,
            r.texel(),
        );
        worst.rgb("albedo", out[0], rgb(diffuse));
        worst.see("roughness", out[0][3], roughness(normal[3], Rough::Raw));
        worst.see("occlusion", out[1][3], occlusion(mask[2], ao_on));
    }
    worst.check("MAD_Clutters_Grass");
}

/// MAD_Tex2 (WhiteRoom 211, at 0x07FD79): layers A = (t78 diffuse, t75 normal map, t80 mask) and B = (t76, t77,
/// t79); cb3[1] Vertex Color Intensity (0: the pass doesn't lower it), [2] Height Contrast, [3] Alpha Offset, [4]
/// Alpha Contrast, [5..7] and [8..10] the layers' HSL, [11..12] their Roughness Offsets, [13] On Off AO, [14..15] and
/// [17..18] their UV rows. The weights the pass gives its LayeredMaterial: B `sat((a - 0.5) AC + 0.5 + AC AO)`, A one
/// minus that, by the core height blend of the diffuses' alphas; each output their weighted sum.
#[test]
fn mad_tex2() {
    let Some(s) = shader("DataPC_MAD_WhiteRoom.forge", 211, 0x07_FD79) else {
        return;
    };
    let mut r = Random(0x0002_1100_0007_FD79);
    let mut worst = Worst::default();
    for _ in 0..50 {
        let (d_a, d_b, n_a, n_b, m_a, m_b) =
            (r.texel(), r.texel(), r.texel(), r.texel(), r.texel(), r.texel());
        let (contrast, alpha_offset, alpha_contrast) =
            (r.range(0.1, 0.99), r.range(-0.5, 0.5), r.range(0.0, 4.0));
        let hsl_a = [r.range(-0.2, 0.2), r.range(-0.3, 0.3), r.range(-0.2, 0.2)];
        let hsl_b = [r.range(-0.2, 0.2), r.range(-0.3, 0.3), r.range(-0.2, 0.2)];
        let (off_a, off_b, ao_on) = (r.range(-0.2, 0.2), r.range(-0.2, 0.2), r.next());
        let colour = r.texel();
        let mut cb3 = r.decoys(20);
        let scalars = [0.0, contrast, alpha_offset, alpha_contrast];
        for (row, v) in scalars
            .into_iter()
            .chain(hsl_a)
            .chain(hsl_b)
            .chain([off_a, off_b, ao_on])
            .enumerate()
        {
            set(&mut cb3, row + 1, v);
        }
        uv_rows(&mut cb3, 14);
        uv_rows(&mut cb3, 17);
        let textures = [(75, n_a), (76, d_b), (77, n_b), (78, d_a), (79, m_b), (80, m_a)];
        let out = run(&s, &textures, &cb3, colour);
        let w = sat((colour[3] - 0.5) * alpha_contrast + 0.5 + alpha_contrast * alpha_offset);
        let mapped = [1.0 - w, w, 0.0, 0.0].map(|x| x as f32);
        let heights = [d_a[3], d_b[3], 0.0, 0.0].map(|x| x as f32);
        let k = height_blend(mapped, heights, 2, contrast as f32).map(f64::from);
        let (a, b) = (tinted(rgb(d_a), m_a[0], hsl_a), tinted(rgb(d_b), m_b[0], hsl_b));
        worst.rgb("albedo", out[0], [0, 1, 2].map(|i| k[0] * a[i] + k[1] * b[i]));
        let (ra, rb) = (
            roughness(n_a[3], Rough::Tex2(off_a)),
            roughness(n_b[3], Rough::Tex2(off_b)),
        );
        worst.see("roughness", out[0][3], k[0] * ra + k[1] * rb);
        worst.see(
            "metalness",
            out[1][3],
            k[0] * metalness(n_a[2], None) + k[1] * metalness(n_b[2], None),
        );
        let (oa, ob) = (occlusion(m_a[2], ao_on), occlusion(m_b[2], ao_on));
        worst.see("occlusion", out[3][0], k[0] * oa + k[1] * ob);
    }
    worst.check("MAD_Tex2");
}

/// MAD_Fake_Sediment (USNAT 59822, at 0x03F958): as MAD_Fake (t75 albedo, t76 normal map) with its two terrain switches
/// (cb3[1] Use Terrain Albedo Mul, [2] Use Terrain Sediments) at 0, the engine's terrain textures unset.
#[test]
fn mad_fake_sediment() {
    let Some(s) = shader("DataPC_MAD_USNAT.forge", 59822, 0x03_F958) else {
        return;
    };
    let mut r = Random(0x0059_8220_0003_F958);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (albedo, normal) = (r.texel(), r.texel());
        let mut cb3 = r.decoys(3);
        cb3[1] = [0.0; 4];
        cb3[2] = [0.0; 4];
        let out = run(&s, &[(75, albedo), (76, normal)], &cb3, r.texel());
        worst.rgb("albedo", out[0], rgb(albedo));
        worst.see("roughness", out[0][3], roughness(normal[3], Rough::Raw));
        worst.see("metalness", out[1][3], metalness(normal[2], None));
    }
    worst.check("MAD_Fake_Sediment");
}

/// MAD_Tex1_TerrainBlend (USNAT 61916, at 0x081AB2; layout B: no metalness output, `3 - Metalness` cb3[1] a decoy):
/// MAD_Tex1's cb3[2..7] and textures, EnableTerrainBlend (cb3[14]) 0, the UV rows at 15. The engine's terrain
/// constants are the notes' (cb1[3..5].w 1000, cb0[47], [48], [50]), its terrain textures unset. o0 = (albedo,
/// roughness), o1.w AO, o1.z's byte the shading class 3 in its upper five bits (metalness 0 in the lighting pass).
#[test]
fn mad_tex1_terrain_blend() {
    let Some(s) = shader("DataPC_MAD_USNAT.forge", 61916, 0x08_1AB2) else {
        return;
    };
    let mut r = Random(0x0061_9160_0008_1AB2);
    let mut worst = Worst::default();
    let engine = [
        (1, 3, [0.0, 0.0, 0.0, 1000.0]),
        (1, 4, [0.0, 0.0, 0.0, 1000.0]),
        (1, 5, [0.0, 0.0, 0.0, 1000.0]),
        (0, 47, [1.0, 0.0, 0.0, 0.0]),
        (0, 48, [1.0, 10.0, 0.0, 0.0]),
        (0, 50, [0.0, 0.0, 0.0, 1.0]),
    ];
    for case in 0..40 {
        let (normal, diffuse, mask) = (r.texel(), r.texel(), r.texel());
        let colour = [r.next(), r.next(), r.next(), 1.0];
        let hsl = [r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)];
        let (offset, ao_on) = (r.range(-0.5, 0.5), r.next());
        let vci = f64::from(case % 2);
        let mut cb3 = r.decoys(17);
        for (row, v) in [hsl[0], hsl[1], hsl[2], offset, ao_on, vci]
            .into_iter()
            .enumerate()
        {
            set(&mut cb3, row + 2, v);
        }
        set(&mut cb3, 14, 0.0);
        uv_rows(&mut cb3, 15);
        let out = run_with(
            &s,
            &[(75, normal), (76, diffuse), (77, mask)],
            &cb3,
            colour,
            &engine,
        );
        let albedo = tinted(rgb(diffuse), mask[0], hsl);
        let factor = |k: usize| if vci == 1.0 { colour[k] } else { 1.0 };
        worst.rgb("albedo", out[0], [0, 1, 2].map(|k| albedo[k] * factor(k)));
        worst.see(
            "roughness",
            out[0][3],
            roughness(normal[3], Rough::Offset(offset)),
        );
        worst.see("occlusion", out[1][3], occlusion(mask[2], ao_on));
        let class = ((sat(out[1][2]) * 255.0).round() as u32) >> 3;
        worst.see("class", f64::from(class), 3.0);
    }
    worst.check("MAD_Tex1_TerrainBlend");
}

/// MAD_Tex1_Emissive's and MAD_Tex1_UGE's opaque G-buffer case: t75 normal map, t76 albedo, t77 emissive mask, t23
/// the exposure (loaded); cb3[1..7] Metalness, Emissive Scale, Hue, Saturate, Luminosity, Alpha, Vertex Color (0 or
/// 1), the UV rows at `uv`. o0 = (albedo tinted everywhere, roughness raw), o1.w metalness, o2.z the emission: the
/// mask's lane `lane` times the scale over the exposure.
fn emissive(s: &ProgramShader, seed: u64, uv: usize, lane: usize, template: &str) {
    let mut r = Random(seed);
    let mut worst = Worst::default();
    for case in 0..40 {
        let (normal, albedo, mask, colour) = (r.texel(), r.texel(), r.texel(), r.texel());
        let (metal, scale, exposure) = (r.next(), r.range(0.0, 50.0), r.range(0.25, 8.0));
        let hsl = [r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)];
        let vci = f64::from(case % 2);
        let mut cb3 = r.decoys(uv + 2);
        for (row, v) in [metal, scale, hsl[0], hsl[1], hsl[2], r.next(), vci]
            .into_iter()
            .enumerate()
        {
            set(&mut cb3, row + 1, v);
        }
        uv_rows(&mut cb3, uv);
        let out = run(
            s,
            &[
                (75, normal),
                (76, albedo),
                (77, mask),
                (23, [exposure, 0.0, 0.0, 0.0]),
            ],
            &cb3,
            colour,
        );
        let tinted = tinted(rgb(albedo), 1.0, hsl);
        let factor = |k: usize| if vci == 1.0 { colour[k] } else { 1.0 };
        worst.rgb("albedo", out[0], [0, 1, 2].map(|k| tinted[k] * factor(k)));
        worst.see("roughness", out[0][3], roughness(normal[3], Rough::Raw));
        worst.see("metalness", out[1][3], metalness(normal[2], Some(metal)));
        let e = emission(mask[lane], scale, exposure);
        // relative: the emission reaches 200
        worst.see("emission (relative)", out[2][2] / e.max(1.0), e / e.max(1.0));
    }
    worst.check(template);
}

/// MAD_Tex1_Emissive (DataPC 61043, at 0x0692CE): the UV rows at 8, emission the mask's red.
#[test]
fn mad_tex1_emissive() {
    let Some(s) = shader("DataPC.forge", 61043, 0x06_92CE) else {
        return;
    };
    emissive(&s, 0x0061_0430_0006_92CE, 8, 0, "MAD_Tex1_Emissive");
}

/// MAD_Tex1_UGE (DataPC 42674, at 0x09798B): `Overriden Emissive Scale` cb3[8] a decoy (read only by the instanced
/// permutations), the UV rows at 9, emission the mask's alpha.
#[test]
fn mad_tex1_uge() {
    let Some(s) = shader("DataPC.forge", 42674, 0x09_798B) else {
        return;
    };
    emissive(&s, 0x0042_6740_0009_798B, 9, 3, "MAD_Tex1_UGE");
}

/// MAD_Tex1_Graffiti_Mode (USNAT 181398, at 0x07B7D4): t75 mask, t76 normal map, t77 diffuse; cb3[1] On Off AO,
/// [3] Color - Logo 0, [4..6] Hue, Saturation, Luminosity, [7] and [8] the team and custom colours with alpha 0 (as
/// all four Materials ship them; their colour, the blend ratio [9], the shutter tiling [10] and the state value [2]
/// decoys), the UV rows at 11. o0 = (Tex1's albedo, roughness raw), o1.w metalness raw, o3.x AO, o2.z no emission.
#[test]
fn mad_tex1_graffiti_mode() {
    let Some(s) = shader("DataPC_MAD_USNAT.forge", 181_398, 0x07_B7D4) else {
        return;
    };
    let mut r = Random(0x0181_3980_0007_B7D4);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (mask, normal, diffuse) = (r.texel(), r.texel(), r.texel());
        let hsl = [r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)];
        let ao_on = r.next();
        let mut cb3 = r.decoys(13);
        set(&mut cb3, 1, ao_on);
        set(&mut cb3, 3, 0.0);
        for (row, v) in hsl.into_iter().enumerate() {
            set(&mut cb3, row + 4, v);
        }
        cb3[7][3] = 0.0;
        cb3[8][3] = 0.0;
        uv_rows(&mut cb3, 11);
        let out = run(&s, &[(75, mask), (76, normal), (77, diffuse)], &cb3, r.texel());
        worst.rgb("albedo", out[0], tinted(rgb(diffuse), mask[0], hsl));
        worst.see("roughness", out[0][3], roughness(normal[3], Rough::Raw));
        worst.see("metalness", out[1][3], metalness(normal[2], None));
        worst.see("occlusion", out[3][0], occlusion(mask[2], ao_on));
        worst.see("emission", out[2][2], 0.0);
    }
    worst.check("MAD_Tex1_Graffiti_Mode");
}

/// The G-buffer pixel program (key 0x400000, docs/shaders.md §6.2) of the MaterialTemplate named `template` in the
/// forge file `forge`, found through its permutation table.
fn gbuffer(forge: &str, template: &str) -> Option<ProgramShader> {
    let dir = install(Game::Riders)?;
    let mut archive = Archive::open(&[dir.join(forge)]).expect("archive");
    let names = &archive.forge(0).expect("forge").names;
    let entry = names
        .iter()
        .position(|n| n.type_id == template::MATERIAL_TEMPLATE && n.name == template)
        .unwrap_or_else(|| panic!("no template {template} in {forge}"));
    let body = archive.resource(At { forge: 0, entry }).expect("template").body;
    let p = template::permutations(&body).expect("permutations");
    let program = p
        .iter()
        .find(|x| x.stage == template::Stage::Pixel && x.key == 0x40_0000)
        .expect("the G-buffer permutation");
    Some(executor(program.container))
}

/// MAD_Tex1_TeamColoured (§27.9): MAD_Tex1's formulas with its rows moved: cb3[1] Metalness, [2] Roughness Offset,
/// [3] On Off AO, [4] Vertex Color Intensity, [5..7] the Neutral hue, saturation and luminosity, [8..13] Teammate and
/// Opponent (decoys: not read), [14..15] the UV rows; t75 normal map, t76 diffuse, t77 mask. Hues from 0 up (a
/// negative one wraps where Tex1's tint doesn't; the lowering refuses it).
#[test]
fn mad_tex1_team_coloured() {
    let Some(s) = gbuffer("DataPC_MAD_USNAT.forge", "MAD_Tex1_TeamColoured") else {
        return;
    };
    let mut r = Random(0x0044_5380_0000_0001);
    let mut worst = Worst::default();
    for case in 0..40 {
        let (normal, diffuse, mask, colour) = (r.texel(), r.texel(), r.texel(), r.texel());
        let (metal, offset, ao_on) = (r.next(), r.range(-0.5, 0.5), r.next());
        let hsl = [r.range(0.0, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)];
        let vci = f64::from(case % 2);
        let mut cb3 = r.decoys(16);
        for (row, v) in [metal, offset, ao_on, vci, hsl[0], hsl[1], hsl[2]]
            .into_iter()
            .enumerate()
        {
            set(&mut cb3, row + 1, v);
        }
        uv_rows(&mut cb3, 14);
        let out = run(&s, &[(75, normal), (76, diffuse), (77, mask)], &cb3, colour);
        let albedo = tinted(rgb(diffuse), mask[0], hsl);
        let factor = |k: usize| if vci == 1.0 { colour[k] } else { 1.0 };
        worst.rgb("albedo", out[0], [0, 1, 2].map(|k| albedo[k] * factor(k)));
        worst.see(
            "roughness",
            out[0][3],
            roughness(normal[3], Rough::Offset(offset)),
        );
        worst.see("metalness", out[1][3], metalness(normal[2], Some(metal)));
        worst.see("occlusion", out[3][0], occlusion(mask[2], ao_on));
    }
    worst.check("MAD_Tex1_TeamColoured");
}

/// MAD_Tex2_TintBFromShaderConstants (§27.5): MAD_Tex2's formulas with layer B tinted by the Neutral constants
/// cb3[11..13], the roughness offsets at [8], [9], On Off AO at [10], Teammate and Opponent [14..19] decoys, the UV
/// rows at 20 and 23; MAD_Tex2's textures.
#[test]
fn mad_tex2_tint_b() {
    let Some(s) = gbuffer("DataPC_MAD_USNAT.forge", "MAD_Tex2_TintBFromShaderConstants") else {
        return;
    };
    let mut r = Random(0x0020_7000_0000_0002);
    let mut worst = Worst::default();
    for _ in 0..50 {
        let (d_a, d_b, n_a, n_b, m_a, m_b) =
            (r.texel(), r.texel(), r.texel(), r.texel(), r.texel(), r.texel());
        let (contrast, alpha_offset, alpha_contrast) =
            (r.range(0.1, 0.99), r.range(-0.5, 0.5), r.range(0.0, 4.0));
        let hsl_a = [r.range(-0.2, 0.2), r.range(-0.3, 0.3), r.range(-0.2, 0.2)];
        let neutral = [r.range(-0.2, 0.2), r.range(-0.3, 0.3), r.range(-0.2, 0.2)];
        let (off_a, off_b, ao_on) = (r.range(-0.2, 0.2), r.range(-0.2, 0.2), r.next());
        let colour = r.texel();
        let mut cb3 = r.decoys(26);
        for (row, v) in [0.0, contrast, alpha_offset, alpha_contrast]
            .into_iter()
            .chain(hsl_a)
            .chain([off_a, off_b, ao_on])
            .chain(neutral)
            .enumerate()
        {
            set(&mut cb3, row + 1, v);
        }
        uv_rows(&mut cb3, 20);
        uv_rows(&mut cb3, 23);
        let textures = [(75, n_a), (76, d_b), (77, n_b), (78, d_a), (79, m_b), (80, m_a)];
        let out = run(&s, &textures, &cb3, colour);
        let w = sat((colour[3] - 0.5) * alpha_contrast + 0.5 + alpha_contrast * alpha_offset);
        let mapped = [1.0 - w, w, 0.0, 0.0].map(|x| x as f32);
        let heights = [d_a[3], d_b[3], 0.0, 0.0].map(|x| x as f32);
        let k = height_blend(mapped, heights, 2, contrast as f32).map(f64::from);
        let (a, b) = (tinted(rgb(d_a), m_a[0], hsl_a), tinted(rgb(d_b), m_b[0], neutral));
        worst.rgb("albedo", out[0], [0, 1, 2].map(|i| k[0] * a[i] + k[1] * b[i]));
        let (ra, rb) = (
            roughness(n_a[3], Rough::Tex2(off_a)),
            roughness(n_b[3], Rough::Tex2(off_b)),
        );
        worst.see("roughness", out[0][3], k[0] * ra + k[1] * rb);
        worst.see(
            "metalness",
            out[1][3],
            k[0] * metalness(n_a[2], None) + k[1] * metalness(n_b[2], None),
        );
        let (oa, ob) = (occlusion(m_a[2], ao_on), occlusion(m_b[2], ao_on));
        worst.see("occlusion", out[3][0], k[0] * oa + k[1] * ob);
    }
    worst.check("MAD_Tex2_TintBFromShaderConstants");
}

/// MAD_Grounds (§27.7, corrected) with the neutral vertex the lowering states (COLOR0 white; the scalar `c` 1, the
/// harness's second colour), `Use Blend` (cb3[11]) 0, `Tire Albedo` (8) at most 1 and `Tire Vertex Color Impact` (10) at
/// least 0: layer A tinted, its normal map's alpha and blue raw, whatever its masks t79 and t80 (random here) and the
/// other rows (Vertex Color Mask Intensity 4, On Off 5, Tires Intensity 6 and Roughness 7).
#[test]
fn mad_grounds() {
    let Some(s) = gbuffer("DataPC_MAD_USNAT.forge", "MAD_Grounds") else {
        return;
    };
    let mut r = Random(0x0018_2108_0000_0003);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (d_a, d_b, n_a, n_b) = (r.texel(), r.texel(), r.texel(), r.texel());
        let (mask, blend_mask) = (r.texel(), r.texel());
        let hsl = [r.range(-0.2, 0.2), r.range(-0.3, 0.3), r.range(-0.2, 0.2)];
        let mut cb3 = r.decoys(24);
        set(&mut cb3, 1, hsl[0]);
        set(&mut cb3, 2, hsl[1]);
        set(&mut cb3, 3, hsl[2]);
        set(&mut cb3, 4, r.range(0.5, 10.0));
        set(&mut cb3, 5, r.next());
        set(&mut cb3, 6, r.next());
        set(&mut cb3, 7, r.next());
        set(&mut cb3, 8, r.next());
        // UV World - on Off, Tire Vertex Color Impact, Use Blend
        set(&mut cb3, 9, 0.0);
        set(&mut cb3, 10, r.next());
        set(&mut cb3, 11, 0.0);
        for row in [12, 15, 18, 21] {
            uv_rows(&mut cb3, row);
        }
        let textures = [
            (75, n_a),
            (76, d_a),
            (77, n_b),
            (78, d_b),
            (79, mask),
            (80, blend_mask),
        ];
        let out = run(&s, &textures, &cb3, [1.0; 4]);
        worst.rgb("albedo", out[0], tinted(rgb(d_a), 1.0, hsl));
        worst.see("roughness", out[0][3], roughness(n_a[3], Rough::Raw));
        worst.see("metalness", out[1][3], metalness(n_a[2], None));
        worst.see("occlusion", out[3][0], 1.0);
    }
    worst.check("MAD_Grounds");
}

/// MAD_Tex1_Graffiti_Mode_DecalFX (§27.10), its alpha-tested permutation 0x404000, as the lowering reduces it: with
/// `TeamBlendRatio` (cb3[6]) at least 1 and an opaque team colour (cb3[4]), the albedo is the team colour to the 2.2
/// whatever the atlas (t75) and the custom colour (cb3[3]) hold; roughness 1, metalness 0. The fade and the alpha test
/// pass (cb2[16] = (0, 1), cb2[18].y 0); the object's screen origin finite (cb2[11].w 1).
#[test]
fn mad_tex1_graffiti_mode_decal_fx() {
    let Some(s) = gbuffer_key(
        "DataPC_MAD_USNAT_patch_01.forge",
        "MAD_Tex1_Graffiti_Mode_DecalFX",
        0x40_4000,
    ) else {
        return;
    };
    let mut r = Random(0x0015_6678_0040_4000);
    let mut worst = Worst::default();
    for case in 0..40 {
        let (atlas, mask) = (r.texel(), r.texel());
        let mut cb3 = r.decoys(13);
        cb3[1] = [f64::from(case % 9), 0.0, 0.0, 0.0];
        cb3[2] = [f64::from(case % 2), 0.0, 0.0, 0.0];
        cb3[3] = r.texel();
        let team = [r.next(), r.next(), r.next(), 1.0];
        cb3[4] = team;
        cb3[5] = [3.0, 3.0, 0.0, 0.0];
        cb3[6] = [r.range(1.0, 3.0), 0.0, 0.0, 0.0];
        uv_rows(&mut cb3, 8);
        uv_rows(&mut cb3, 11);
        let engine = [
            (2, 16, [0.0, 1.0, 0.0, 0.0]),
            (2, 18, [0.0; 4]),
            (2, 11, [0.0, 0.0, 0.0, 1.0]),
        ];
        let out = run_with(
            &s,
            &[(75, atlas), (76, mask), (23, [1.0; 4])],
            &cb3,
            r.texel(),
            &engine,
        );
        worst.rgb("albedo", out[0], [0, 1, 2].map(|c| team[c].powf(2.2)));
        // the lowering's emission: EmissiveMult (cb3[7]) g^2, the exposure t23 at 1, the team colour's alpha 1
        worst.see("emission", out[2][2], cb3[7][0] * mask[1] * mask[1]);
        worst.see("roughness", out[0][3], 1.0);
        worst.see("metalness", out[1][3], 0.0);
    }
    worst.check("MAD_Tex1_Graffiti_Mode_DecalFX");
}

/// The pixel program of key `key` of the MaterialTemplate named `template` in the forge file `forge`.
fn gbuffer_key(forge: &str, template: &str, key: u32) -> Option<ProgramShader> {
    let dir = install(Game::Riders)?;
    let mut archive = Archive::open(&[dir.join(forge)]).expect("archive");
    let names = &archive.forge(0).expect("forge").names;
    let entry = names
        .iter()
        .position(|n| n.type_id == template::MATERIAL_TEMPLATE && n.name == template)
        .unwrap_or_else(|| panic!("no template {template} in {forge}"));
    let body = archive.resource(At { forge: 0, entry }).expect("template").body;
    let p = template::permutations(&body).expect("permutations");
    let program = p
        .iter()
        .find(|x| x.stage == template::Stage::Pixel && x.key == key)
        .expect("the permutation");
    Some(executor(program.container))
}

/// The pixel program of key `key` of the MaterialTemplate named `template`, in the first of Riders Republic's forges
/// (patch first) that holds it.
fn template_key(template: &str, key: u32) -> Option<ProgramShader> {
    let dir = install(Game::Riders)?;
    for forge in [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ] {
        let mut archive = Archive::open(&[dir.join(forge)]).expect("archive");
        let names = &archive.forge(0).expect("forge").names;
        let Some(entry) = names
            .iter()
            .position(|n| n.type_id == template::MATERIAL_TEMPLATE && n.name == template)
        else {
            continue;
        };
        let body = archive.resource(At { forge: 0, entry }).expect("template").body;
        let p = template::permutations(&body).expect("permutations");
        let program = p
            .iter()
            .find(|x| x.stage == template::Stage::Pixel && x.key == key)
            .expect("the permutation");
        return Some(executor(program.container));
    }
    panic!("no template {template}")
}

/// MAD_Tex1_SkinningSupported (§27.14): MAD_Tex1's formulas on its own G-buffer program, its rows Tex1's.
#[test]
fn mad_tex1_skinning_supported() {
    let Some(s) = template_key("MAD_Tex1_SkinningSupported", 0x40_0000) else {
        return;
    };
    let mut r = Random(0x0019_0000_0040_0000);
    let mut worst = Worst::default();
    for case in 0..40 {
        let (normal, diffuse, mask, colour) = (r.texel(), r.texel(), r.texel(), r.texel());
        let (metal, offset, ao_on) = (r.next(), r.range(-0.5, 0.5), r.next());
        let hsl = [r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)];
        let vci = f64::from(case % 2);
        let mut cb3 = r.decoys(10);
        for (row, v) in [metal, hsl[0], hsl[1], hsl[2], offset, ao_on, vci]
            .into_iter()
            .enumerate()
        {
            set(&mut cb3, row + 1, v);
        }
        uv_rows(&mut cb3, 8);
        let out = run(&s, &[(75, normal), (76, diffuse), (77, mask)], &cb3, colour);
        let albedo = tinted(rgb(diffuse), mask[0], hsl);
        let factor = |k: usize| if vci == 1.0 { colour[k] } else { 1.0 };
        worst.rgb("albedo", out[0], [0, 1, 2].map(|k| albedo[k] * factor(k)));
        worst.see(
            "roughness",
            out[0][3],
            roughness(normal[3], Rough::Offset(offset)),
        );
        worst.see("metalness", out[1][3], metalness(normal[2], Some(metal)));
        worst.see("occlusion", out[3][0], occlusion(mask[2], ao_on));
    }
    worst.check("MAD_Tex1_SkinningSupported");
}

/// MAD_Decals1_Graffiti (§27.14.5), its placed key 0x404004 with `EmissivePower` (cb3[6]) 0: MAD_Decals1's formulas,
/// its Fresnel rows (7, 8) and team colour (9) decoys, no emission; the alpha test passing.
#[test]
fn mad_decals1_graffiti() {
    let Some(s) = template_key("MAD_Decals1_Graffiti", 0x40_4004) else {
        return;
    };
    let mut r = Random(0x0003_0000_0040_4004);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (normal, diffuse) = (r.texel(), [r.next(), r.next(), r.next(), 1.0]);
        let (metal, offset) = (r.next(), r.range(-0.5, 0.5));
        let hsl = [r.range(-0.5, 0.5), r.range(-0.5, 0.5), r.range(-0.5, 0.5)];
        let mut cb3 = r.decoys(10);
        for (row, v) in [metal, hsl[0], hsl[1], hsl[2], offset, 0.0]
            .into_iter()
            .enumerate()
        {
            set(&mut cb3, row + 1, v);
        }
        let engine = [
            (2, 16, [0.0, 1.0, 0.0, 0.0]),
            (2, 18, [0.0; 4]),
            (2, 11, [0.0, 0.0, 0.0, 1.0]),
        ];
        let out = run_with(&s, &[(75, normal), (76, diffuse)], &cb3, r.texel(), &engine);
        worst.rgb("albedo", out[0], tinted(rgb(diffuse), 1.0, hsl));
        worst.see(
            "roughness",
            out[0][3],
            roughness(normal[3], Rough::Offset(offset)),
        );
        worst.see("metalness", out[1][3], metalness(normal[2], Some(metal)));
        worst.see("emission", out[2][2], 0.0);
    }
    worst.check("MAD_Decals1_Graffiti");
}

/// MAD_Tex2_Graffiti_Mode (§27.14.6) with `State Value` 0: MAD_Tex2's layers and height blend with its own rows
/// (cb3[1] Alpha Contrast, [3..5] and [8..10] the layers' HSL, [6] Height Contrast, [7] Alpha Offset, [11] On Off AO,
/// [12..13] and [15..16] the UV rows), each roughness its normal map's alpha raw, no vertex-colour factor.
#[test]
fn mad_tex2_graffiti_mode() {
    let Some(s) = template_key("MAD_Tex2_Graffiti_Mode", 0x40_0000) else {
        return;
    };
    let mut r = Random(0x0008_0000_0040_0000);
    let mut worst = Worst::default();
    for _ in 0..50 {
        let (d_a, d_b, n_a, n_b, m_a, m_b) =
            (r.texel(), r.texel(), r.texel(), r.texel(), r.texel(), r.texel());
        let (contrast, alpha_offset, alpha_contrast) =
            (r.range(0.1, 0.99), r.range(-0.5, 0.5), r.range(0.0, 4.0));
        let hsl_a = [r.range(-0.2, 0.2), r.range(-0.3, 0.3), r.range(-0.2, 0.2)];
        let hsl_b = [r.range(-0.2, 0.2), r.range(-0.3, 0.3), r.range(-0.2, 0.2)];
        let ao_on = r.next();
        let colour = r.texel();
        let mut cb3 = r.decoys(17);
        for (row, v) in [
            (1, alpha_contrast),
            (2, 0.0),
            (3, hsl_a[0]),
            (4, hsl_a[1]),
            (5, hsl_a[2]),
            (6, contrast),
            (7, alpha_offset),
            (8, hsl_b[0]),
            (9, hsl_b[1]),
            (10, hsl_b[2]),
            (11, ao_on),
        ] {
            set(&mut cb3, row, v);
        }
        uv_rows(&mut cb3, 12);
        uv_rows(&mut cb3, 15);
        let textures = [(75, n_a), (76, d_b), (77, n_b), (78, d_a), (79, m_b), (80, m_a)];
        let out = run(&s, &textures, &cb3, colour);
        let w = sat((colour[3] - 0.5) * alpha_contrast + 0.5 + alpha_contrast * alpha_offset);
        let mapped = [1.0 - w, w, 0.0, 0.0].map(|x| x as f32);
        let heights = [d_a[3], d_b[3], 0.0, 0.0].map(|x| x as f32);
        let k = height_blend(mapped, heights, 2, contrast as f32).map(f64::from);
        let (a, b) = (tinted(rgb(d_a), m_a[0], hsl_a), tinted(rgb(d_b), m_b[0], hsl_b));
        worst.rgb("albedo", out[0], [0, 1, 2].map(|i| k[0] * a[i] + k[1] * b[i]));
        worst.see(
            "roughness",
            out[0][3],
            k[0] * roughness(n_a[3], Rough::Raw) + k[1] * roughness(n_b[3], Rough::Raw),
        );
        worst.see(
            "metalness",
            out[1][3],
            k[0] * metalness(n_a[2], None) + k[1] * metalness(n_b[2], None),
        );
        let (oa, ob) = (occlusion(m_a[2], ao_on), occlusion(m_b[2], ao_on));
        worst.see("occlusion", out[3][0], k[0] * oa + k[1] * ob);
    }
    worst.check("MAD_Tex2_Graffiti_Mode");
}

/// MAD_Tex1_Screen (§27.14.4): t75 the screen; the exposure t23 at 1. Albedo `0.6 sat(t75)^2.2`, roughness 0.1,
/// metalness 0, emission (o2.z) the pass's constant 0.666667.
#[test]
fn mad_tex1_screen() {
    let Some(s) = template_key("MAD_Tex1_Screen", 0x40_0000) else {
        return;
    };
    let mut r = Random(0x0004_0000_0040_0000);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let screen = r.texel();
        let out = run(&s, &[(75, screen), (23, [1.0; 4])], &r.decoys(4), r.texel());
        worst.rgb(
            "albedo",
            out[0],
            [0, 1, 2].map(|k| 0.6 * sat(screen[k]).powf(2.2)),
        );
        worst.see("roughness", out[0][3], 0.1);
        worst.see("metalness", out[1][3], 0.0);
        worst.see("emission", out[2][2], 0.666_667);
    }
    worst.check("MAD_Tex1_Screen");
}

/// MAD_Screens (§27.14.1) with `Use Videos` (cb3[5]) 0 and `Use Logos` (cb3[4]) 0, or 1 with a black `Neutral` (cb3[7]):
/// black, no emission, roughness 1, metalness 0, whatever the frame (the time cb0[0].x, the frame rate cb3[10], the
/// sheet cb3[13] and the delays drawn at random: the frame arithmetic's udiv executed), textures and other rows.
#[test]
fn mad_screens_black() {
    let Some(s) = template_key("MAD_Screens", 0x40_0000) else {
        return;
    };
    let mut r = Random(0x0058_0000_0040_0000);
    let mut worst = Worst::default();
    for case in 0..40 {
        let mut cb3 = r.decoys(16);
        set(&mut cb3, 5, 0.0);
        if case % 2 == 0 {
            set(&mut cb3, 4, 0.0);
        } else {
            set(&mut cb3, 4, 1.0);
            cb3[7] = [0.0, 0.0, 0.0, r.next()];
        }
        set(&mut cb3, 10, r.range(0.0, 30.0));
        cb3[13] = [r.range(1.0, 9.0).floor(), r.range(1.0, 33.0).floor(), 0.0, 0.0];
        set(&mut cb3, 11, r.range(0.0, 200.0));
        set(&mut cb3, 12, r.range(0.0, 200.0));
        uv_rows(&mut cb3, 14);
        let engine = [(0, 0, [r.range(0.0, 1000.0), 0.0, 0.0, 0.0])];
        let textures = [(75, r.texel()), (76, r.texel()), (77, r.texel()), (23, [1.0; 4])];
        let out = run_with(&s, &textures, &cb3, r.texel(), &engine);
        worst.rgb("albedo", out[0], [0.0; 3]);
        worst.see("roughness", out[0][3], 1.0);
        worst.see("metalness", out[1][3], 0.0);
        worst.see("emission", out[2][2], 0.0);
    }
    worst.check("MAD_Screens");
}

/// MAD_Tex1_FakeRiver (§27.14.9): its scalar `s` is its program's v2.x, the vertex colour's red (COLOR0.x, its input
/// signature read), drawn at random; t75 normal map, t76 foam; cb3[1] `3 - Metalness` (roughness and metalness both),
/// [2..3] the UV rows.
#[test]
fn mad_tex1_fake_river() {
    let Some(s) = template_key("MAD_Tex1_FakeRiver", 0x40_0000) else {
        return;
    };
    let mut r = Random(0x0002_0000_0040_0000);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let (normal, foam, value) = (r.texel(), r.texel(), r.next());
        let mut cb3 = r.decoys(4);
        set(&mut cb3, 1, value);
        uv_rows(&mut cb3, 2);
        let colour = r.texel();
        let out = run(&s, &[(75, normal), (76, foam)], &cb3, colour);
        let albedo = primase_anvil::passes::fake_river_albedo(foam[3], colour[0]);
        worst.rgb("albedo", out[0], albedo);
        worst.see("roughness", out[0][3], value);
        worst.see("metalness", out[1][3], value);
    }
    worst.check("MAD_Tex1_FakeRiver");
}

/// A sampler answering each sample of t75 with the coordinates it was asked for (`(u, v, 0, 1)`), every other
/// texture a constant.
struct Coordinates;

impl Sampler for Coordinates {
    fn sample(&mut self, r: Sample) -> Result<[f32; 4], String> {
        Ok(if r.texture == 75 {
            [r.uv[0], r.uv[1], 0.0, 1.0]
        } else {
            [0.5, 0.5, 1.0, 1.0]
        })
    }
    fn load(&mut self, _r: Load) -> Result<[u32; 4], String> {
        Ok([0; 4])
    }
    fn structured(&mut self, _slot: u32, _element: u32, _offset: u32) -> Result<u32, String> {
        Ok(0)
    }
    fn store(&mut self, _request: Store) -> Result<(), String> {
        Ok(())
    }
}

/// MAD_Outskirt (§27.14.3): its program samples the albedo at `((P, 1) . cb2[0], (P, 1) . cb2[1]) + offset) scale`
/// (P its TEXCOORD0, the object position; cb2[0..1] the placement matrix's first two rows, source frame); the lowering's
/// rows ([`world_uv_rows`]) on the same point through the placement, converted to core's frame (ENU), give the same
/// coordinates, whatever the matrix's third row.
#[test]
fn mad_outskirt_maps_by_the_world_position() {
    let Some(s) = template_key("MAD_Outskirt", 0x40_0000) else {
        return;
    };
    let frame = primase_core::frame::SourceFrame::ENU;
    let mut r = Random(0x0048_0000_0040_0000);
    let mut worst = Worst::default();
    for _ in 0..40 {
        let p = [r.range(-50.0, 50.0), r.range(-50.0, 50.0), r.range(-50.0, 50.0)];
        let rows: [[f64; 4]; 3] = std::array::from_fn(|_| {
            [
                r.range(-1.0, 1.0),
                r.range(-1.0, 1.0),
                r.range(-1.0, 1.0),
                r.range(-5000.0, 5000.0),
            ]
        });
        let (offset, scale, tiling) = (
            [r.range(-20000.0, 0.0), r.range(-20000.0, 0.0)],
            r.range(1e-5, 1e-4),
            16.0,
        );
        let mut cb3 = r.decoys(4);
        set(&mut cb3, 1, tiling);
        set(&mut cb3, 2, scale);
        cb3[3] = [offset[0], offset[1], 0.0, 0.0];
        let inputs: Vec<[u32; 4]> = s
            .interface()
            .inputs
            .iter()
            .map(|i| {
                match (
                    i.label.name.as_str(),
                    i.provenance.d3d.expect("a D3D register").register,
                ) {
                    ("SV_Position", _) => bits([0.5, 0.5, 0.5, 1.0]),
                    (_, 1) => bits([p[0], p[1], p[2], 1.0]),
                    (_, reg) if reg % 2 == 1 => bits([1.0, 0.0, 0.0, 0.0]),
                    _ => bits([0.0, 0.0, 1.0, 1.0]),
                }
            })
            .collect();
        let buffers: Vec<(u32, Vec<[u32; 4]>)> = s
            .constant_buffers()
            .iter()
            .map(|&(slot, size)| {
                let v = (0..size as usize)
                    .map(|row| match slot {
                        2 if row < 2 => bits(rows[row]),
                        3 => cb3.get(row).copied().map_or([0; 4], bits),
                        _ => [0; 4],
                    })
                    .collect();
                (slot, v)
            })
            .collect();
        let views: Vec<(u32, &[[u32; 4]])> = buffers.iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let out = s.execute(&inputs, &views, &mut Coordinates).expect("runs");
        let albedo = float(out.outputs[0][0]);
        let albedo_v = float(out.outputs[0][1]);
        // the point through the placement (source frame), in core's frame, through the lowering's rows
        let source: [f64; 3] =
            std::array::from_fn(|k| rows[k][0] * p[0] + rows[k][1] * p[1] + rows[k][2] * p[2] + rows[k][3]);
        let core = frame.point(source.map(|x| x as f32)).map(f64::from);
        let want = primase_anvil::passes::world_uv_rows(frame, offset, scale)
            .map(|row| row[0] * core[0] + row[1] * core[1] + row[2] * core[2] + row[3]);
        // the coordinates are the f32 world position's (thousands of metres) times 1e-5: a few f32 ulps of it
        worst.see("u", albedo, want[0]);
        worst.see("v", albedo_v, want[1]);
    }
    worst.check("MAD_Outskirt");
}
