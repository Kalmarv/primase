//! `encode-textures`: every texture with its complete mip chain, in the form the program asks for (`format=bc7`: BC7
//! blocks, lossy and counted; `format=none`: 8-bit RGBA as decoded, lossless), the levels a texture does not store
//! built by the filter the program names (`mips=box`).
//!
//! What a texture stores is kept: its stored levels are the chain's first levels (decoded exactly, then encoded as
//! asked), and only the levels below the last stored one are built, each from the one above it. The filter
//! (`mips=box`, the only one so far): each texel of a level the mean of the 2 x 2 texels above it (an odd side's last
//! texel also takes the leftover row or column, as [`crate::half`] does), computed in f32 from the last stored level
//! down and rounded to 8 bits once per level:
//! - colour (any role but normal) in linear light: an sRGB texture's channels decoded to linear light first, a linear
//!   one's taken as stored; alpha as stored, straight (colour not weighted by it);
//! - a normal map's vectors as directions: each texel's vector decoded by its convention (X from red or alpha, Z
//!   stored or rebuilt as `sqrt(1 - x^2 - y^2)`), normalized, averaged and normalized again; a mean shorter than
//!   `sqrt(3) / 255` (no direction to keep) becomes (0, 0, 1), counted; the channels the convention leaves unused are
//!   averaged as stored.
//!
//! Left as stored, each counted by why: data textures (`TexRole::Data`, kept whole and lossless), formats 8-bit RGBA
//! does not hold exactly ([`crate::rgba8_exact`]), block-compressed textures (their stored blocks are the game's; a
//! missing level could only be built by encoding them again), a normal map that states no convention, and a texture
//! already complete in the form asked. With `format=bc7`, a texture whose base level is not a whole number of 4 x 4
//! blocks keeps 8-bit RGBA (a block-compressed texture's base level is whole blocks), with its chain, counted.
//!
//! Every BC7 encode is an approximation on its texture, named with its RMS error over every level against the 8-bit
//! texels it encoded, in bands. With `cache=on` each texture's BC7 levels are looked up in, and stored to, the caller's
//! memo store (`core.pass_memo`), keyed by the texels, their description, the options and this crate's source.

use primase_cache::{KeyBuilder, source_hash};
use primase_core::pass::memo::{Key as MemoKey, Memo};
use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{
    ColorSpace, Component, Entity, NormalConvention, NormalX, NormalZ, TexFormat, TexRole, Texture,
    TextureFile, World,
};
use primase_foundations::color::{linear_to_srgb8_f32, srgb8_to_linear_f32};
use primase_texcodec::{Decoder, Encoder, TexDecoder, TexEncoder};

use crate::{Error, Result, rgba8_exact};

/// The form `encode-textures` leaves each texture in (`format=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// BC7 blocks (lossy, counted with its error).
    Bc7,
    /// 8-bit RGBA as decoded (lossless).
    None,
}

impl Choice for Encoding {
    const VALUES: &'static [(&'static str, Self)] = &[("bc7", Encoding::Bc7), ("none", Encoding::None)];
}

/// How `encode-textures` builds the levels a texture does not store (`mips=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MipFilter {
    /// The 2 x 2 box, colour in linear light and normal maps renormalized (the module documentation).
    Box,
}

impl Choice for MipFilter {
    const VALUES: &'static [(&'static str, Self)] = &[("box", MipFilter::Box)];
}

/// Whether `encode-textures` uses the caller's memo store (`cache=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cache {
    /// No storage is used.
    Off,
    /// Each texture's BC7 levels are looked up in, and stored to, the caller's `core.pass_memo` service.
    On,
}

impl Choice for Cache {
    const VALUES: &'static [(&'static str, Self)] = &[("off", Cache::Off), ("on", Cache::On)];
}

/// `encode-textures{format=bc7|none,mips=box[,cache=on|off]}` (the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncodeTextures {
    pub format: Encoding,
    pub mips: MipFilter,
    pub cache: Cache,
}

/// `encode-textures`' declaration ([`primase_core::pass::PassRegistry::add`]): the form and the filter are the
/// program's, required.
pub const ENCODE_TEXTURES: PassSpec = PassSpec {
    name: "encode-textures",
    options: &[
        OptionSpec::choice::<Encoding>(
            "format",
            "the form every texture is left in: BC7 blocks (lossy, each encode counted with its error), or 8-bit RGBA \
             as decoded",
        ),
        OptionSpec::choice::<MipFilter>(
            "mips",
            "how the levels a texture does not store are built: the 2 x 2 box, colour in linear light, normal maps \
             renormalized",
        ),
        OptionSpec::optional_choice::<Cache>(
            "cache",
            "use the caller's memo store for each texture's BC7 levels",
            "off: no storage is used",
        ),
    ],
    make: |g| Ok(Box::new(EncodeTextures::from_given(g))),
};

impl EncodeTextures {
    fn from_given(g: &Given) -> EncodeTextures {
        EncodeTextures {
            format: g.choice("format"),
            mips: g.choice("mips"),
            cache: g.optional_choice("cache").unwrap_or(Cache::Off),
        }
    }
}

/// Bump whenever the same texels and options can give other levels or another reported error: the filter, the
/// rounding, the encoder's settings, the error's measure. It is also the memo payload's version.
const ENCODE_VERSION: u32 = 2;

/// The textures converted at once: enough to keep every thread busy, few enough that the new levels held beside the
/// old ones stay a small part of the world's textures.
const BATCH_PER_THREAD: usize = 2;

impl Pass for EncodeTextures {
    fn name(&self) -> &'static str {
        "encode-textures"
    }
    fn options(&self) -> Options {
        Options::from([
            ("format".to_owned(), self.format.text().to_owned()),
            ("mips".to_owned(), self.mips.text().to_owned()),
            ("cache".to_owned(), self.cache.text().to_owned()),
        ])
    }
    // no `requires`: a world without textures has nothing to encode
    fn settings(&self) -> &[primase_core::pass::Setting] {
        &[primase_core::parallel::THREADS]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME]
    }
    /// The file link of a texture whose levels it remade: the file no longer holds them.
    fn removes(&self) -> &[&'static str] {
        &[TextureFile::NAME]
    }
    fn approximates(&self) -> &'static str {
        "with format=bc7, textures encoded to BC7 blocks (lossy, each named with its RMS error); the levels a texture \
         does not store built by the 2 x 2 box, colour in linear light and normal maps renormalized"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let threads = primase_core::parallel::threads(world).map_err(|e| e.to_string())?;
        let memo = match self.cache {
            Cache::On => world.resource::<Memo>().cloned(),
            Cache::Off => None,
        };
        if self.format == Encoding::Bc7 && self.cache == Cache::On {
            log.count(HITS, 0);
            log.count(MISSES, 0);
        }
        let before: u64 = world.iter::<Texture>().map(|(_, t)| t.payload_bytes()).sum();
        let mut todo = Vec::new();
        for (e, t) in world.iter::<Texture>() {
            match plan(t, self.format) {
                Plan::Left(why) => log.count(&format!("textures left as stored: {why}"), 1),
                Plan::Remake { encode, not_blocks } => todo.push((e, encode, not_blocks)),
            }
        }
        // a batch converted on the stated threads, then put in the world before the next is made
        let batch = threads.get().saturating_mul(BATCH_PER_THREAD).max(1);
        for chunk in todo.chunks(batch) {
            let jobs: Vec<(Entity, &Texture, Encoding, bool)> = chunk
                .iter()
                .filter_map(|&(e, encode, not_blocks)| {
                    world.get::<Texture>(e).map(|t| (e, t, encode, not_blocks))
                })
                .collect();
            let made =
                primase_foundations::parallel::map(threads, &jobs, |_, &(_, t, encode, not_blocks)| {
                    remake(t, encode, not_blocks, memo.as_ref().map(|m| (m, self)))
                })
                .map_err(|e| e.to_string())?;
            let mut done = Vec::with_capacity(jobs.len());
            for (&(e, t, _, _), made) in jobs.iter().zip(made) {
                let made = made.map_err(|error| format!("texture {e:?}: {error}"))?;
                made.report(e, t, log);
                done.push((e, made.texture));
            }
            for (e, texture) in done {
                world.insert(e, texture);
                world.remove::<TextureFile>(e);
            }
        }
        let after: u64 = world.iter::<Texture>().map(|(_, t)| t.payload_bytes()).sum();
        log.count("texture bytes before", before);
        log.count("texture bytes after", after);
        Ok(())
    }
}

const HITS: &str = "texture encode cache hits";
const MISSES: &str = "texture encode cache misses";

/// What becomes of one texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// Left as stored, and why.
    Left(&'static str),
    /// Its chain completed and its levels in `encode`'s form; `not_blocks`: BC7 was asked, and its base is not whole
    /// 4 x 4 blocks.
    Remake { encode: Encoding, not_blocks: bool },
}

/// The levels a full chain of `width` x `height` has, down to 1 x 1.
fn full_levels(width: u32, height: u32) -> usize {
    (u32::BITS - width.max(height).leading_zeros()) as usize
}

fn plan(t: &Texture, format: Encoding) -> Plan {
    if t.role == TexRole::Data {
        return Plan::Left("data, kept whole and lossless");
    }
    if !rgba8_exact(t.format) {
        return Plan::Left("a format 8-bit RGBA does not hold exactly");
    }
    let complete = t.levels.len() == full_levels(t.width, t.height);
    if t.format.block_dims() != (1, 1) {
        return Plan::Left(if complete {
            "block-compressed, every level stored"
        } else {
            "block-compressed with levels missing, which only encoding its stored blocks again could build"
        });
    }
    if t.role == TexRole::Normal && t.normal.is_none() {
        return Plan::Left("a normal map that states no convention");
    }
    let not_blocks = format == Encoding::Bc7 && !(t.width.is_multiple_of(4) && t.height.is_multiple_of(4));
    let encode = if not_blocks { Encoding::None } else { format };
    if complete && encode == Encoding::None && t.format == TexFormat::Rgba8Unorm {
        return Plan::Left(if not_blocks {
            "8-bit RGBA with every level stored, its base not whole 4 x 4 blocks"
        } else {
            "8-bit RGBA with every level stored"
        });
    }
    Plan::Remake { encode, not_blocks }
}

/// One texture remade, and what the report says of it.
#[derive(Debug)]
struct Made {
    texture: Texture,
    /// Levels built below the stored ones.
    built: u64,
    /// Texels of built normal levels whose mean had no direction, made (0, 0, 1).
    flat: u64,
    /// The BC7 encode's RMS error over every level, in 8-bit steps (BC7 only).
    error: Option<f32>,
    /// BC7 wanted, but the base is not whole 4 x 4 blocks.
    not_blocks: bool,
    /// Looked up in the memo store: `Some(true)` found.
    cached: Option<bool>,
}

impl Made {
    fn report(&self, e: Entity, before: &Texture, log: &mut PassLog) {
        log.count("textures remade", 1);
        if self.built > 0 {
            log.count("textures given the levels they do not store", 1);
            log.count("mip levels built", self.built);
            if before.role == TexRole::Normal {
                log.count("normal map mip levels built renormalized", self.built);
            } else if before.color_space == ColorSpace::Srgb {
                log.count("sRGB mip levels built in linear light", self.built);
            } else {
                log.count("linear mip levels built", self.built);
            }
        }
        if self.flat > 0 {
            log.count(
                "normal texels whose mean had no direction, made (0, 0, 1)",
                self.flat,
            );
        }
        if self.not_blocks {
            log.count("textures kept as 8-bit RGBA: base not whole 4 x 4 blocks", 1);
        }
        if before.format != TexFormat::Rgba8Unorm && self.texture.format == TexFormat::Rgba8Unorm {
            log.count("textures decoded to 8-bit RGBA (exact)", 1);
        }
        match self.cached {
            Some(true) => log.count(HITS, 1),
            Some(false) => log.count(MISSES, 1),
            None => {}
        }
        if let Some(error) = self.error {
            log.count("textures encoded to BC7", 1);
            log.approximate_on(e, &format!("texture encoded to BC7, {}", error_band(error)), 1);
        }
    }
}

/// The band a BC7 encode's RMS error falls in, as its approximation names it.
fn error_band(rms: f32) -> &'static str {
    match rms {
        r if r <= 1. => "RMS error at most 1 of 255",
        r if r <= 2. => "RMS error 1 to 2 of 255",
        r if r <= 4. => "RMS error 2 to 4 of 255",
        r if r <= 8. => "RMS error 4 to 8 of 255",
        _ => "RMS error over 8 of 255",
    }
}

/// `t` with its chain completed ([`chain`]) and its levels in `encode`'s form ([`Plan::Remake`]); BC7 levels from
/// `memo` where it holds them.
fn remake(
    t: &Texture,
    encode: Encoding,
    not_blocks: bool,
    memo: Option<(&Memo, &EncodeTextures)>,
) -> Result<Made> {
    let key = match (encode, memo) {
        (Encoding::Bc7, Some((memo, pass))) => Some((memo, memo_key(&memo.build, pass, t))),
        _ => None,
    };
    if let Some((memo, key)) = &key
        && let Some(value) = memo.store.get(key)
        && let Some(mut made) = unframe(t, &value)
    {
        made.cached = Some(true);
        return Ok(made);
    }
    let MipChain { levels, built, flat } = complete_mip_chain(t, 0)?;
    let mut texture = Texture {
        width: t.width,
        height: t.height,
        format: TexFormat::Rgba8Unorm,
        levels,
        color_space: t.color_space,
        role: t.role,
        normal: t.normal,
    };
    let mut error = None;
    if encode == Encoding::Bc7 {
        let (blocks, rms) = encode_bc7(&texture)?;
        texture.format = TexFormat::Bc7;
        texture.levels = blocks;
        error = Some(rms);
    }
    let mut made = Made {
        texture,
        built,
        flat,
        error,
        not_blocks,
        cached: None,
    };
    if let Some((memo, key)) = &key {
        made.cached = Some(false);
        // a failed advisory write cannot change the texture: the program reads its store's own statistics
        memo.store.put(key, &frame(&made));
    }
    Ok(made)
}

/// A texture's levels as 8-bit RGBA ([`complete_mip_chain`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MipChain {
    /// Largest first, from the level asked for down to 1 x 1.
    pub levels: Vec<Vec<u8>>,
    /// How many of them were built (the texture does not store them).
    pub built: u64,
    /// Texels of built normal levels whose mean had no direction, made (0, 0, 1).
    pub flat: u64,
}

/// `t`'s levels from level `from` down to 1 x 1 as 8-bit RGBA, largest first: the ones it stores decoded (`t`'s format
/// must decode to RGBA8), the ones it does not built by the box from the last stored one, colour in linear light and
/// normal maps renormalized (the module documentation; a normal map that states no convention, and a data texture,
/// filtered as stored values). Refused where `t` stores no level or `from` is past its smallest.
pub fn complete_mip_chain(t: &Texture, from: usize) -> Result<MipChain> {
    let decoder = Decoder::default();
    let size = |level: usize| {
        let shift = u32::try_from(level).unwrap_or(u32::MAX);
        (
            t.width.checked_shr(shift).unwrap_or(0).max(1),
            t.height.checked_shr(shift).unwrap_or(0).max(1),
        )
    };
    let decode = |level: usize| -> Result<Vec<u8>> {
        let (w, h) = size(level);
        let stored = &t.levels[level];
        Ok(if t.format == TexFormat::Rgba8Unorm {
            stored.clone()
        } else {
            decoder.decode_rgba8(t.format, w, h, stored)?
        })
    };
    let total = full_levels(t.width, t.height);
    let stored = t.levels.len().min(total);
    if stored == 0 || from >= total {
        return Err(Error::Invalid(format!(
            "levels from {from} of a {} x {} texture storing {stored}",
            t.width, t.height
        )));
    }
    let mut levels = Vec::with_capacity(total - from);
    for level in from..stored {
        levels.push(decode(level)?);
    }
    if stored == total {
        return Ok(MipChain {
            levels,
            built: 0,
            flat: 0,
        });
    }
    // the levels below the last stored one, each from the one above it; those above `from` only to reach it
    let kind = Kind::of(t);
    let last = stored - 1;
    let mut current = kind.decode(&match levels.last() {
        Some(level) if last >= from => level.clone(),
        _ => decode(last)?,
    });
    let (mut w, mut h) = size(last);
    let mut flat = 0;
    for level in stored..total {
        let (next, nw, nh, made_flat) = kind.halve(&current, w, h)?;
        if level >= from {
            flat += made_flat;
            levels.push(kind.encode(&next));
        }
        (current, w, h) = (next, nw, nh);
    }
    Ok(MipChain {
        levels,
        built: (total - stored.max(from)) as u64,
        flat,
    })
}

/// How a texture's texels are filtered: the channels in f32 as the box averages them. A data texture's are its stored
/// values (`encode-textures` leaves data whole; a caller that builds its levels anyway averages what is stored).
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// sRGB colour: red, green and blue in linear light, alpha as stored.
    Srgb,
    /// Linear values: every channel as stored.
    Linear,
    /// A normal map: every channel as stored (`(v + 1) / 2` for a vector's), the vector averaged as a direction.
    Normal(NormalConvention),
}

/// A mean vector shorter than this has no direction to keep: the RGB8 vector quantization's uncertainty
/// ([`crate::resize_normals`]'s).
const NO_DIRECTION: f32 = 0.006_792_35; // sqrt(3) / 255

impl Kind {
    fn of(t: &Texture) -> Kind {
        match (t.role, t.normal) {
            (TexRole::Normal, Some(convention)) => Kind::Normal(convention),
            (TexRole::Data, _) => Kind::Linear,
            _ if t.color_space == ColorSpace::Srgb => Kind::Srgb,
            _ => Kind::Linear,
        }
    }

    fn decode(self, rgba: &[u8]) -> Vec<[f32; 4]> {
        rgba.as_chunks::<4>()
            .0
            .iter()
            .map(|p| match self {
                Kind::Srgb => [
                    srgb8_to_linear_f32(p[0]),
                    srgb8_to_linear_f32(p[1]),
                    srgb8_to_linear_f32(p[2]),
                    f32::from(p[3]) / 255.,
                ],
                Kind::Linear | Kind::Normal(_) => p.map(|c| f32::from(c) / 255.),
            })
            .collect()
    }

    fn encode(self, texels: &[[f32; 4]]) -> Vec<u8> {
        let byte = |c: f32| (c * 255. + 0.5).clamp(0., 255.) as u8;
        texels
            .iter()
            .flat_map(|p| match self {
                Kind::Srgb => [
                    linear_to_srgb8_f32(p[0]),
                    linear_to_srgb8_f32(p[1]),
                    linear_to_srgb8_f32(p[2]),
                    byte(p[3]),
                ],
                Kind::Linear | Kind::Normal(_) => p.map(byte),
            })
            .collect()
    }

    /// The next level of `texels` (`w` x `h`): each side halved (at least 1), each texel the box over the texels it
    /// covers; with how many normal means had no direction.
    fn halve(self, texels: &[[f32; 4]], w: u32, h: u32) -> Result<(Vec<[f32; 4]>, u32, u32, u64)> {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut out = Vec::new();
        out.try_reserve_exact(nw as usize * nh as usize)
            .map_err(|_| Error::Allocation)?;
        let mut flat = 0;
        for y in 0..nh {
            let (ya, yb) = (y * h / nh, (y + 1) * h / nh);
            for x in 0..nw {
                let (xa, xb) = (x * w / nw, (x + 1) * w / nw);
                let mut sum = [0f32; 4];
                let mut direction = [0f32; 3];
                for sy in ya..yb {
                    for sx in xa..xb {
                        let p = texels[sy as usize * w as usize + sx as usize];
                        for c in 0..4 {
                            sum[c] += p[c];
                        }
                        if let Kind::Normal(convention) = self {
                            let v = unit(vector(p, convention));
                            for c in 0..3 {
                                direction[c] += v[c];
                            }
                        }
                    }
                }
                let n = ((xb - xa) * (yb - ya)) as f32;
                let mut texel = sum.map(|s| s / n);
                if let Kind::Normal(convention) = self {
                    let mean = direction.map(|d| d / n);
                    let length = mean.iter().map(|c| c * c).sum::<f32>().sqrt();
                    let v = if length < NO_DIRECTION {
                        flat += 1;
                        [0., 0., 1.]
                    } else {
                        mean.map(|c| c / length)
                    };
                    store(&mut texel, v, convention);
                }
                out.push(texel);
            }
        }
        Ok((out, nw, nh, flat))
    }
}

/// The vector a normal texel holds by `convention`: X from red or alpha, Y from green, Z from blue or rebuilt on the
/// positive hemisphere. Green's direction and the axes' order are the same for every texel, so averaging and
/// renormalizing do not depend on them.
fn vector(p: [f32; 4], convention: NormalConvention) -> [f32; 3] {
    let x = match convention.x {
        NormalX::Red => p[0],
        NormalX::Alpha => p[3],
    } * 2.
        - 1.;
    let y = p[1] * 2. - 1.;
    let z = match convention.z {
        NormalZ::Stored => p[2] * 2. - 1.,
        NormalZ::Rebuild => (1. - x * x - y * y).max(0.).sqrt(),
    };
    [x, y, z]
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let length = v.iter().map(|c| c * c).sum::<f32>().sqrt();
    if length > 0. {
        v.map(|c| c / length)
    } else {
        [0., 0., 1.]
    }
}

/// `v` written into the channels `convention` holds it in; the others keep their mean.
fn store(texel: &mut [f32; 4], v: [f32; 3], convention: NormalConvention) {
    let channel = |c: f32| (c + 1.) / 2.;
    match convention.x {
        NormalX::Red => texel[0] = channel(v[0]),
        NormalX::Alpha => texel[3] = channel(v[0]),
    }
    texel[1] = channel(v[1]);
    if convention.z == NormalZ::Stored {
        texel[2] = channel(v[2]);
    }
}

/// `t`'s 8-bit RGBA levels as BC7, and the RMS error of the encode over every level, in 8-bit steps.
fn encode_bc7(t: &Texture) -> Result<(Vec<Vec<u8>>, f32)> {
    let (encoder, decoder) = (Encoder::default(), Decoder::default());
    let mut blocks = Vec::with_capacity(t.levels.len());
    let (mut squared, mut count) = (0f64, 0u64);
    for (i, rgba) in t.levels.iter().enumerate() {
        let shift = u32::try_from(i).unwrap_or(u32::MAX);
        let (w, h) = (
            t.width.checked_shr(shift).unwrap_or(0).max(1),
            t.height.checked_shr(shift).unwrap_or(0).max(1),
        );
        let level = encoder.encode_rgba8(TexFormat::Bc7, w, h, rgba)?;
        let decoded = decoder.decode_rgba8(TexFormat::Bc7, w, h, &level)?;
        for (a, b) in rgba.iter().zip(&decoded) {
            let d = f64::from(*a) - f64::from(*b);
            squared += d * d;
        }
        count += rgba.len() as u64;
        blocks.push(level);
    }
    Ok((blocks, (squared / count.max(1) as f64).sqrt() as f32))
}

/// The memo key of `t`'s BC7 levels under `pass`: everything its levels and reported counts follow from.
fn memo_key(build: &str, pass: &EncodeTextures, t: &Texture) -> MemoKey {
    let mut k = KeyBuilder::new("primase-image.encode-textures")
        .str("build", build)
        .bytes("source", &source_hash(crate::CACHE_SOURCES))
        .u32("version", ENCODE_VERSION)
        .str("format", pass.format.text())
        .str("mips", pass.mips.text())
        .u32("width", t.width)
        .u32("height", t.height)
        .str("texture-format", &format!("{:?}", t.format))
        .str("color-space", &format!("{:?}", t.color_space))
        .str("role", &format!("{:?}", t.role))
        .str("normal", &format!("{:?}", t.normal))
        .u64("levels", t.levels.len() as u64);
    for level in &t.levels {
        k = k.bytes("level", level);
    }
    MemoKey(k.finish().bytes())
}

/// A remade BC7 texture as the memo stores it: the version, the counts, the error's bits, then each level's length
/// and bytes.
fn frame(made: &Made) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&ENCODE_VERSION.to_le_bytes());
    out.extend_from_slice(&made.built.to_le_bytes());
    out.extend_from_slice(&made.flat.to_le_bytes());
    out.extend_from_slice(&made.error.unwrap_or(f32::NAN).to_bits().to_le_bytes());
    out.extend_from_slice(&(made.texture.levels.len() as u64).to_le_bytes());
    for level in &made.texture.levels {
        out.extend_from_slice(&(level.len() as u64).to_le_bytes());
        out.extend_from_slice(level);
    }
    out
}

/// The texture [`frame`] stored for `t`, or none where the value is not one (another version, a size that does not
/// fit `t`): it is then made again.
fn unframe(t: &Texture, value: &[u8]) -> Option<Made> {
    let mut rest = value;
    let mut take = |n: usize| -> Option<&[u8]> {
        let (head, tail) = rest.split_at_checked(n)?;
        rest = tail;
        Some(head)
    };
    let u64_of = |b: &[u8]| b.try_into().ok().map(u64::from_le_bytes);
    if u32::from_le_bytes(take(4)?.try_into().ok()?) != ENCODE_VERSION {
        return None;
    }
    let built = u64_of(take(8)?)?;
    let flat = u64_of(take(8)?)?;
    let error = f32::from_bits(u32::from_le_bytes(take(4)?.try_into().ok()?));
    let count = usize::try_from(u64_of(take(8)?)?).ok()?;
    if count != full_levels(t.width, t.height) || !error.is_finite() {
        return None;
    }
    let mut levels = Vec::with_capacity(count);
    for i in 0..count {
        let len = usize::try_from(u64_of(take(8)?)?).ok()?;
        let shift = u32::try_from(i).ok()?;
        let (w, h) = (
            t.width.checked_shr(shift)?.max(1),
            t.height.checked_shr(shift)?.max(1),
        );
        if TexFormat::Bc7.checked_level_size(w, h) != Some(len) {
            return None;
        }
        levels.push(take(len)?.to_vec());
    }
    if !rest.is_empty() {
        return None;
    }
    let texture = Texture {
        width: t.width,
        height: t.height,
        format: TexFormat::Bc7,
        levels,
        color_space: t.color_space,
        role: t.role,
        normal: t.normal,
    };
    Some(Made {
        texture,
        built,
        flat,
        error: Some(error),
        not_blocks: false,
        cached: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::PassRegistry;
    use primase_core::pass::memo::Store;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    fn texture(
        width: u32,
        height: u32,
        texels: &[[u8; 4]],
        color_space: ColorSpace,
        role: TexRole,
    ) -> Texture {
        Texture {
            width,
            height,
            format: TexFormat::Rgba8Unorm,
            levels: vec![texels.concat()],
            color_space,
            role,
            normal: (role == TexRole::Normal).then_some(NormalConvention::GLTF),
        }
    }

    fn world(textures: Vec<Texture>) -> (World, Vec<Entity>) {
        let mut w = World::new();
        w.insert_resource(primase_core::parallel::Threads::ONE);
        let entities = textures
            .into_iter()
            .map(|t| {
                let e = w.spawn();
                w.insert(e, t);
                e
            })
            .collect();
        (w, entities)
    }

    fn run(w: &mut World, format: Encoding, cache: Cache) -> PassLog {
        let mut log = PassLog::default();
        EncodeTextures {
            format,
            mips: MipFilter::Box,
            cache,
        }
        .run(w, &mut log)
        .unwrap();
        log
    }

    /// An sRGB colour texture's level is the box in linear light (half black, half white: linear 0.5, sRGB 188); a
    /// linear one's in its stored values (128). Alpha is averaged as stored.
    #[test]
    fn colour_levels_are_built_in_linear_light() {
        let texels = [
            [0, 0, 0, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 0],
            [0, 0, 0, 0],
        ];
        let (mut w, e) = world(vec![
            texture(2, 2, &texels, ColorSpace::Srgb, TexRole::Color),
            texture(2, 2, &texels, ColorSpace::Linear, TexRole::Mask),
        ]);
        let log = run(&mut w, Encoding::None, Cache::Off);
        let srgb = w.get::<Texture>(e[0]).unwrap();
        assert_eq!(srgb.levels, vec![texels.concat(), vec![188, 188, 188, 128]]);
        assert_eq!(
            w.get::<Texture>(e[1]).unwrap().levels[1],
            vec![128, 128, 128, 128]
        );
        assert_eq!(log.counts["sRGB mip levels built in linear light"], 1);
        assert_eq!(log.counts["linear mip levels built"], 1);
        assert!(log.approximations.is_empty(), "{:?}", log.approximations);
    }

    /// A normal map's level is its vectors' mean direction: two vectors leaning apart average to straight up, not to
    /// the shorter mean the stored values give; X held in alpha and Z rebuilt are read and written so, the channels
    /// the convention leaves unused averaged as stored; a mean with no direction is flat, counted.
    #[test]
    fn normal_levels_are_renormalized_in_their_convention() {
        // (0.6, 0, 0.8) and (-0.6, 0, 0.8)
        let apart = [
            [204, 128, 230, 255],
            [51, 128, 230, 255],
            [204, 128, 230, 255],
            [51, 128, 230, 255],
        ];
        let mut alpha_x = texture(
            2,
            2,
            &apart.map(|[x, y, _, _]| [10, y, 30, x]),
            ColorSpace::Linear,
            TexRole::Normal,
        );
        alpha_x.normal = Some(NormalConvention {
            x: NormalX::Alpha,
            z: NormalZ::Rebuild,
            ..NormalConvention::GLTF
        });
        // (1, 0, 0) and (-1, 0, 0): no direction
        let opposed = [
            [255, 128, 128, 255],
            [0, 128, 128, 255],
            [0, 128, 128, 255],
            [255, 128, 128, 255],
        ];
        let (mut w, e) = world(vec![
            texture(2, 2, &apart, ColorSpace::Linear, TexRole::Normal),
            alpha_x,
            texture(2, 2, &opposed, ColorSpace::Linear, TexRole::Normal),
        ]);
        let log = run(&mut w, Encoding::None, Cache::Off);
        assert_eq!(
            w.get::<Texture>(e[0]).unwrap().levels[1],
            vec![128, 128, 255, 255]
        );
        assert_eq!(w.get::<Texture>(e[1]).unwrap().levels[1], vec![10, 128, 30, 128]);
        assert_eq!(
            w.get::<Texture>(e[2]).unwrap().levels[1],
            vec![128, 128, 255, 255]
        );
        assert_eq!(log.counts["normal map mip levels built renormalized"], 3);
        assert_eq!(
            log.counts["normal texels whose mean had no direction, made (0, 0, 1)"],
            1
        );
    }

    /// The levels a texture stores are kept as they are and only the missing ones built; what the pass does not remake
    /// is counted by why and left untouched.
    #[test]
    fn stored_levels_are_kept_and_the_rest_left_by_why() {
        let mut partial = texture(4, 4, &[[90, 20, 200, 255]; 16], ColorSpace::Srgb, TexRole::Color);
        partial.levels.push([1, 2, 3, 4].repeat(4));
        let mut complete = partial.clone();
        complete.levels.push(vec![5, 6, 7, 8]);
        let data = texture(2, 2, &[[7; 4]; 4], ColorSpace::Linear, TexRole::Data);
        let bc1 = Texture {
            width: 4,
            height: 4,
            format: TexFormat::Bc1,
            levels: vec![vec![0; 8]; 3],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        };
        let float = Texture {
            format: TexFormat::Rg16Float,
            levels: vec![vec![0; 16]],
            role: TexRole::Mask,
            ..data.clone()
        };
        let mut no_convention = texture(
            2,
            2,
            &[[128, 128, 255, 255]; 4],
            ColorSpace::Linear,
            TexRole::Normal,
        );
        no_convention.normal = None;
        let left = vec![complete, data, bc1, float, no_convention];
        let (mut w, e) = world([vec![partial.clone()], left.clone()].concat());
        let log = run(&mut w, Encoding::None, Cache::Off);
        let made = w.get::<Texture>(e[0]).unwrap();
        assert_eq!(&made.levels[..2], &partial.levels[..]);
        assert_eq!(made.levels.len(), 3);
        assert_eq!(log.counts["mip levels built"], 1);
        for (e, t) in e[1..].iter().zip(&left) {
            assert_eq!(w.get::<Texture>(*e).unwrap(), t);
        }
        for why in [
            "8-bit RGBA with every level stored",
            "data, kept whole and lossless",
            "block-compressed, every level stored",
            "a format 8-bit RGBA does not hold exactly",
            "a normal map that states no convention",
        ] {
            assert_eq!(log.counts[&format!("textures left as stored: {why}")], 1, "{why}");
        }
    }

    /// From a level past what a texture stores, the chain is built from its last stored level and starts at the level
    /// asked for; from a stored level, the stored ones from there come first.
    #[test]
    fn a_chain_starts_at_the_level_asked_for() {
        let mut t = texture(8, 8, &[[0, 0, 0, 255]; 64], ColorSpace::Linear, TexRole::Color);
        t.levels.push([255, 255, 255, 255].repeat(16));
        let from_stored = complete_mip_chain(&t, 1).unwrap();
        assert_eq!(from_stored.levels.len(), 3);
        assert_eq!(from_stored.levels[0], t.levels[1]);
        assert_eq!(
            (from_stored.built, from_stored.levels[2].clone()),
            (2, vec![255; 4])
        );
        let past = complete_mip_chain(&t, 2).unwrap();
        assert_eq!((past.levels.len(), past.built), (2, 2));
        assert_eq!(past.levels[0], [255, 255, 255, 255].repeat(4));
        assert!(complete_mip_chain(&t, 4).is_err());
    }

    /// With `format=bc7` every level is BC7, the texture valid, and the encode an approximation on it named by its
    /// error; a base that is not whole 4 x 4 blocks keeps 8-bit RGBA with its chain. The same input gives the same
    /// blocks.
    #[test]
    fn bc7_encodes_every_level_and_names_its_error() {
        let texels: Vec<[u8; 4]> = (0..64u8).map(|i| [i * 4, 255 - i * 4, i, 255]).collect();
        let (mut w, e) = world(vec![
            texture(8, 8, &texels, ColorSpace::Srgb, TexRole::Color),
            texture(6, 6, &texels[..36], ColorSpace::Srgb, TexRole::Color),
        ]);
        let log = run(&mut w, Encoding::Bc7, Cache::Off);
        let bc7 = w.get::<Texture>(e[0]).unwrap().clone();
        assert_eq!((bc7.format, bc7.levels.len()), (TexFormat::Bc7, 4));
        bc7.verify(e[0], &w).unwrap();
        let named: Vec<_> = log
            .approximated_on
            .iter()
            .filter(|(_, on)| on.contains(&e[0]))
            .collect();
        assert_eq!(named.len(), 1);
        assert!(
            named[0].0.starts_with("texture encoded to BC7, RMS error"),
            "{named:?}"
        );
        let odd = w.get::<Texture>(e[1]).unwrap();
        assert_eq!((odd.format, odd.levels.len()), (TexFormat::Rgba8Unorm, 3));
        assert_eq!(
            log.counts["textures kept as 8-bit RGBA: base not whole 4 x 4 blocks"],
            1
        );
        assert_eq!(log.counts["textures encoded to BC7"], 1);
        let (mut again, e2) = world(vec![texture(8, 8, &texels, ColorSpace::Srgb, TexRole::Color)]);
        run(&mut again, Encoding::Bc7, Cache::Off);
        assert_eq!(again.get::<Texture>(e2[0]).unwrap(), &bc7);
    }

    #[derive(Debug, Default)]
    struct Memory(Mutex<BTreeMap<[u8; 32], Vec<u8>>>);
    impl Store for Memory {
        fn get(&self, key: &MemoKey) -> Option<Vec<u8>> {
            self.0.lock().unwrap().get(&key.0).cloned()
        }
        fn put(&self, key: &MemoKey, value: &[u8]) {
            self.0.lock().unwrap().insert(key.0, value.to_vec());
        }
    }

    /// With `cache=on` a texture's BC7 levels are stored, and read back by the next run: the same texture and the same
    /// report; a stored value that is not one is made again.
    #[test]
    fn the_memo_store_gives_back_what_was_encoded() {
        let texels: Vec<[u8; 4]> = (0..16u8).map(|i| [i * 16, 3, 200, 255]).collect();
        let store = Arc::new(Memory::default());
        let encoded = |store: &Arc<Memory>| {
            let (mut w, e) = world(vec![texture(4, 4, &texels, ColorSpace::Srgb, TexRole::Color)]);
            w.insert_resource(Memo::new(store.clone(), "test").unwrap());
            let log = run(&mut w, Encoding::Bc7, Cache::On);
            (w.get::<Texture>(e[0]).unwrap().clone(), log)
        };
        let (first, cold) = encoded(&store);
        let (second, warm) = encoded(&store);
        assert_eq!(first, second);
        assert_eq!((cold.counts[MISSES], cold.counts[HITS]), (1, 0));
        assert_eq!((warm.counts[MISSES], warm.counts[HITS]), (0, 1));
        assert_eq!(cold.approximations, warm.approximations);
        for value in store.0.lock().unwrap().values_mut() {
            value.truncate(value.len() - 1);
        }
        let (third, broken) = encoded(&store);
        assert_eq!((third, broken.counts[MISSES]), (first, 1));
    }

    /// The registry makes the pass from its text, and the pass prints the options it was made with.
    #[test]
    fn the_text_states_every_choice() {
        let mut r = PassRegistry::new();
        r.add(&ENCODE_TEXTURES);
        let options = |pairs: &[(&str, &str)]| -> Options {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let pass = r
            .make("encode-textures", &options(&[("format", "bc7"), ("mips", "box")]))
            .unwrap();
        assert_eq!(
            pass.options(),
            options(&[("cache", "off"), ("format", "bc7"), ("mips", "box")])
        );
        assert!(r.make("encode-textures", &options(&[("format", "bc7")])).is_err());
        assert!(
            r.make("encode-textures", &options(&[("format", "png"), ("mips", "box")]))
                .is_err()
        );
    }
}
