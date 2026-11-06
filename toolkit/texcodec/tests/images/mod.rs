//! Synthetic images for the encoder tests: straight RGBA, rows top first. LDR images are bytes, HDR ones floats.
#![allow(dead_code)]

/// An image and its name.
pub struct Image<T> {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub texels: Vec<T>,
}

/// A xorshift stream, the same everywhere.
pub struct Random(u32);

impl Random {
    pub fn new(seed: u32) -> Random {
        Random(seed.max(1))
    }
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    /// Uniform in 0..1.
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1 << 24) as f32
    }
}

fn bytes(name: &'static str, width: u32, height: u32, f: impl Fn(u32, u32) -> [f32; 4]) -> Image<u8> {
    let mut texels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            texels.extend(f(x, y).map(|v| (v.clamp(0., 1.) * 255.).round() as u8));
        }
    }
    Image {
        name,
        width,
        height,
        texels,
    }
}

fn floats(name: &'static str, width: u32, height: u32, f: impl Fn(u32, u32) -> [f32; 4]) -> Image<f32> {
    let mut texels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            texels.extend(f(x, y));
        }
    }
    Image {
        name,
        width,
        height,
        texels,
    }
}

/// A smooth field in 0..1: a few sinusoids, different per channel.
fn smooth(x: u32, y: u32, c: usize) -> f32 {
    let (x, y) = (x as f32, y as f32);
    let k = c as f32 + 1.;
    0.5 + 0.25 * (x * 0.11 * k + y * 0.07).sin() + 0.2 * (y * 0.13 / k - x * 0.05 * k).cos()
}

/// The LDR set: a ramp (one line in RGBA), a gradient (a plane), noise, smooth fields, alpha edges, colour blocks
/// (each block one colour), and sizes that are not whole blocks.
pub fn ldr() -> Vec<Image<u8>> {
    let mut noise = Random::new(7);
    let noise_texels: Vec<[f32; 4]> = (0..64 * 64)
        .map(|_| std::array::from_fn(|_| noise.unit()))
        .collect();
    let mut blocks = Random::new(11);
    let colours: Vec<[f32; 4]> = (0..16 * 16)
        .map(|_| std::array::from_fn(|_| blocks.unit()))
        .collect();
    vec![
        bytes("ramp", 64, 64, |x, _| {
            let t = x as f32 / 63.;
            [t, 0.16 + 0.6 * t, 0.8 - 0.7 * t, 1. - 0.4 * t]
        }),
        bytes("gradient", 64, 64, |x, y| {
            [
                x as f32 / 63.,
                y as f32 / 63.,
                (x + y) as f32 / 126.,
                1. - x as f32 / 126.,
            ]
        }),
        bytes("noise", 64, 64, |x, y| noise_texels[(y * 64 + x) as usize]),
        bytes("smooth", 64, 64, |x, y| {
            [smooth(x, y, 0), smooth(x, y, 1), smooth(x, y, 2), smooth(x, y, 3)]
        }),
        bytes("alpha-edges", 64, 64, |x, y| {
            // a disc, opaque inside, a two-texel soft rim, a graded band at the bottom
            let d = ((x as f32 - 31.5).powi(2) + (y as f32 - 27.5).powi(2)).sqrt();
            let a = if y >= 56 {
                x as f32 / 63.
            } else {
                (1. - (d - 20.) / 2.).clamp(0., 1.)
            };
            [smooth(x, y, 0), smooth(x, y, 1), smooth(x, y, 2), a]
        }),
        bytes("blocks", 64, 64, |x, y| colours[((y / 4) * 16 + x / 4) as usize]),
        bytes("odd", 37, 23, |x, y| {
            [smooth(x, y, 2), x as f32 / 36., y as f32 / 22., smooth(x, y, 1)]
        }),
        bytes("tiny", 3, 2, |x, y| {
            [x as f32 / 2., y as f32, 0.5, 1. - x as f32 / 4.]
        }),
    ]
}

/// The HDR set: a 0..1 ramp, a range of exponents (2^-10 to 2^15) by colour, log-uniform noise, colour blocks, and a
/// smooth signed field (-1000..1000) for SF16; UF16 takes its magnitude.
/// Half-float RGB bit patterns of 4x4 blocks that the encoder writes in BC6H's rarest modes, 0x06 and 0x0a (11-bit
/// endpoints, one channel's deltas a bit wider), found by a search over two-region blocks: UF16, then SF16.
pub const RARE_MODES: [[[u16; 48]; 2]; 2] = [
    [
        [
            0x3dbe, 0x3f32, 0x40c9, 0x3dbe, 0x3f32, 0x40c9, 0x3d6c, 0x3e3c, 0x409f, 0x3d6c, 0x3e3c, 0x409f,
            0x3dc4, 0x3f38, 0x40df, 0x3dc4, 0x3f38, 0x40df, 0x3d6f, 0x3e3d, 0x40b7, 0x3d6f, 0x3e3d, 0x40b7,
            0x3dc9, 0x3f3e, 0x40f6, 0x3dc9, 0x3f3e, 0x40f6, 0x3d73, 0x3e3e, 0x40cf, 0x3d73, 0x3e3e, 0x40cf,
            0x3dce, 0x3f44, 0x410c, 0x3dce, 0x3f44, 0x410c, 0x3d76, 0x3e3f, 0x40e6, 0x3d76, 0x3e3f, 0x40e6,
        ],
        [
            0x21b5, 0x22b0, 0x1c84, 0x21b5, 0x22b0, 0x1c84, 0x221e, 0x2266, 0x1cf6, 0x221e, 0x2266, 0x1cf6,
            0x21ba, 0x22c8, 0x1ccd, 0x21ba, 0x22c8, 0x1ccd, 0x221f, 0x227e, 0x1d03, 0x221f, 0x227e, 0x1d03,
            0x21c0, 0x22df, 0x1d16, 0x21c0, 0x22df, 0x1d16, 0x2220, 0x2295, 0x1d11, 0x2220, 0x2295, 0x1d11,
            0x21c5, 0x22f7, 0x1d5e, 0x21c5, 0x22f7, 0x1d5e, 0x2221, 0x22ad, 0x1d1e, 0x2221, 0x22ad, 0x1d1e,
        ],
    ],
    [
        [
            0xa770, 0xa724, 0x9f92, 0xa770, 0xa724, 0x9f92, 0xa6bb, 0xa90a, 0xa02a, 0xa6bb, 0xa90a, 0xa02a,
            0xa774, 0xa73e, 0x9fa3, 0xa774, 0xa73e, 0x9fa3, 0xa6bd, 0xa90f, 0xa02c, 0xa6bd, 0xa90f, 0xa02c,
            0xa778, 0xa758, 0x9fb4, 0xa778, 0xa758, 0x9fb4, 0xa6c0, 0xa913, 0xa02f, 0xa6c0, 0xa913, 0xa02f,
            0xa77c, 0xa771, 0x9fc6, 0xa77c, 0xa771, 0x9fc6, 0xa6c2, 0xa917, 0xa032, 0xa6c2, 0xa917, 0xa032,
        ],
        [
            0xc058, 0xbedd, 0xc431, 0xc058, 0xbedd, 0xc431, 0xc0a4, 0xbfa0, 0xc337, 0xc0a4, 0xbfa0, 0xc337,
            0xc059, 0xbedd, 0xc432, 0xc059, 0xbedd, 0xc432, 0xc0a6, 0xbfa5, 0xc352, 0xc0a6, 0xbfa5, 0xc352,
            0xc05b, 0xbedd, 0xc433, 0xc05b, 0xbedd, 0xc433, 0xc0a8, 0xbfaa, 0xc36c, 0xc0a8, 0xbfaa, 0xc36c,
            0xc05d, 0xbedd, 0xc434, 0xc05d, 0xbedd, 0xc434, 0xc0aa, 0xbfaf, 0xc387, 0xc0aa, 0xbfaf, 0xc387,
        ],
    ],
];

pub fn hdr(signed: bool) -> Vec<Image<f32>> {
    let mut noise = Random::new(5);
    let noise_texels: Vec<[f32; 4]> = (0..64 * 64)
        .map(|_| {
            let mut v: [f32; 4] = std::array::from_fn(|_| (noise.unit() * 20. - 8.).exp2());
            v[3] = 1.;
            v
        })
        .collect();
    let mut blocks = Random::new(3);
    let colours: Vec<[f32; 4]> = (0..16 * 16)
        .map(|_| {
            let mut v: [f32; 4] = std::array::from_fn(|_| (blocks.unit() * 24. - 10.).exp2());
            v[3] = 1.;
            v
        })
        .collect();
    vec![
        floats("hdr-ramp", 64, 64, |x, y| {
            let t = x as f32 / 63.;
            [t, 1. - t, y as f32 / 63., 1.]
        }),
        floats("hdr-range", 64, 64, |x, y| {
            let e = (x as f32 / 63. * 25. - 10.).exp2();
            [e, e * (y as f32 / 63.), e * smooth(x, y, 2), 1.]
        }),
        floats("hdr-noise", 64, 64, |x, y| noise_texels[(y * 64 + x) as usize]),
        floats("hdr-blocks", 64, 64, |x, y| {
            colours[((y / 4) * 16 + x / 4) as usize]
        }),
        floats("hdr-signed", 64, 64, |x, y| {
            let v: [f32; 3] = std::array::from_fn(|c| (smooth(x, y, c) * 2. - 1.) * 1000.);
            let v = if signed { v } else { v.map(f32::abs) };
            [v[0], v[1], v[2], 1.]
        }),
        floats("hdr-odd", 19, 13, |x, y| {
            [x as f32 * 3.5, y as f32 * 0.01, smooth(x, y, 1) * 40., 1.]
        }),
        floats("hdr-rare-modes", 8, 4, |x, y| {
            let block = &RARE_MODES[usize::from(signed)][(x / 4) as usize];
            let i = ((y * 4 + x % 4) * 3) as usize;
            let v: [f32; 3] = std::array::from_fn(|c| half::f16::from_bits(block[i + c]).to_f32());
            [v[0], v[1], v[2], 1.]
        }),
    ]
}
