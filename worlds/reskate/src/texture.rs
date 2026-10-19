//! Textures of a map: a source texture (a texture file's bytes, as the reference writer reads them, or a texture of
//! the world), and the library that writes the map's texture files (DXT levels passed through as `.dds`, everything
//! else as `.png`), every file named after its content. Nothing here reads a file: a reader hands in the bytes.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Mutex;

use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
use primase_hash::{content_bytes, content_finish, content_string, content_word};
use primase_texcodec::{Decoder, TexDecoder};

use crate::{Error, Result};

/// Hash64's initial state.
pub const HASH_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// The reference writer's Hash64, word by word.
#[derive(Clone, Copy, Debug)]
pub struct Hash64(u64);

impl Hash64 {
    pub fn new() -> Self {
        Hash64(HASH_SEED)
    }
    pub fn word(&mut self, w: u64) -> &mut Self {
        self.0 = content_word(self.0, w);
        self
    }
    /// `Add(int)` / `Add(uint)`: the value as a 32-bit unsigned word.
    pub fn int(&mut self, v: i32) -> &mut Self {
        self.word(u64::from(v as u32))
    }
    pub fn uint(&mut self, v: u32) -> &mut Self {
        self.word(u64::from(v))
    }
    pub fn string(&mut self, s: &str) -> &mut Self {
        self.0 = content_string(self.0, s);
        self
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0 = content_bytes(self.0, b);
        self
    }
    pub fn value(&self) -> u64 {
        content_finish(self.0)
    }
}

impl Default for Hash64 {
    fn default() -> Self {
        Self::new()
    }
}

/// How a source file's pixels are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Png,
    Dxt,
    Bc5,
    Rgba,
    Rgbx,
    Bgra,
    Bgrx,
}

/// Where a [`TexSource`]'s stored levels are.
#[derive(Debug)]
enum Origin {
    /// A texture file's bytes (its levels from `data_offset` on), labelled by `name`.
    File { name: String, bytes: Vec<u8> },
    /// A texture of the world: its levels as stored, labelled by `name`.
    Memory { name: String, levels: Vec<Vec<u8>> },
}

/// A source texture: a texture file's bytes ([`TexSource::from_file_bytes`]: a PNG, one level, or a DDS: DXT1/3/5
/// passed through, BC5 and 32-bit RGBA/BGRA decoded) or a texture of the world (`core.texture`, [`TexSource::from_texture`]), the chain of smaller
/// levels completed with the 2 x 2 box filter where the source does not hold them.
#[derive(Debug)]
pub struct TexSource {
    origin: Origin,
    pub width: u32,
    pub height: u32,
    /// The full chain down to 1 x 1.
    pub levels: u32,
    /// `DXT1` / `DXT3` / `DXT5` when the file's levels are passed through.
    pub fourcc: Option<&'static str>,
    pub format_name: String,
    pub source_width: u32,
    pub source_height: u32,
    kind: Kind,
    file_levels: u32,
    data_offset: usize,
    made: Mutex<HashMap<u32, Vec<u8>>>,
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"))
}

fn level_bytes(w: u32, h: u32, block: u64) -> u64 {
    u64::from(w.div_ceil(4).max(1)) * u64::from(h.div_ceil(4).max(1)) * block
}

impl TexSource {
    /// A texture file's `bytes` (a PNG or a DDS), labelled by `name` (its path, in messages).
    pub fn from_file_bytes(
        name: &str,
        bytes: Vec<u8>,
        source_format: Option<&str>,
        source_size: Option<[i32; 2]>,
    ) -> Result<TexSource> {
        let bad = |m: String| Error::Texture(format!("{name}: {m}"));
        let h = &bytes[..bytes.len().min(148)];
        let (width, height, kind, fourcc, file_levels, data_offset);
        let format_name: &str;
        if h.len() >= 8 && h[..4] == [0x89, b'P', b'N', b'G'] {
            if h.len() < 33 {
                return Err(bad("truncated PNG".into()));
            }
            width = u32::from_be_bytes(h[16..20].try_into().expect("4"));
            height = u32::from_be_bytes(h[20..24].try_into().expect("4"));
            let (depth, ty, interlace) = (h[24], h[25], h[28]);
            if depth != 8 || !matches!(ty, 0 | 2 | 3 | 4 | 6) || interlace != 0 {
                return Err(bad(format!(
                    "PNG of bit depth {depth}, colour type {ty}, interlace {interlace} (8-bit grey, RGB, palette, grey+alpha or RGBA, not interlaced)"
                )));
            }
            (kind, fourcc, format_name, file_levels, data_offset) = (Kind::Png, None, "PNG", 1, 0);
        } else {
            if h.len() < 128 || &h[..4] != b"DDS " {
                return Err(bad("neither a PNG nor a DDS".into()));
            }
            height = u32le(h, 12);
            width = u32le(h, 16);
            let flags = u32le(h, 8);
            let mips = u32le(h, 28);
            file_levels = if flags & 0x20000 != 0 || mips > 1 {
                mips.max(1)
            } else {
                1
            };
            let pf = u32le(h, 80);
            let four = &h[84..88];
            let bits = u32le(h, 88);
            let rmask = u32le(h, 92);
            let caps2 = u32le(h, 112);
            if caps2 & 0x200 != 0 || caps2 & 0x200000 != 0 {
                return Err(bad("a cube map or volume (2D only)".into()));
            }
            if pf & 4 != 0 && four == b"DX10" {
                if h.len() < 148 {
                    return Err(bad("truncated DX10 header".into()));
                }
                let dxgi = u32le(h, 128);
                let (dim, array) = (u32le(h, 132), u32le(h, 140));
                if dim != 3 || array > 1 {
                    return Err(bad(format!(
                        "not one 2D texture (dimension {dim}, array {array})"
                    )));
                }
                data_offset = 148;
                (kind, fourcc, format_name) = match dxgi {
                    70..=72 => (Kind::Dxt, Some("DXT1"), "D3DFMT_DXT1"),
                    73..=75 => (Kind::Dxt, Some("DXT3"), "D3DFMT_DXT3"),
                    76..=78 => (Kind::Dxt, Some("DXT5"), "D3DFMT_DXT5"),
                    82 | 83 => (Kind::Bc5, None, "D3DFMT_ATI2"),
                    28 | 29 => (Kind::Rgba, None, "R8G8B8A8"),
                    87 | 91 => (Kind::Bgra, None, "B8G8R8A8"),
                    _ => return Err(bad(format!("DXGI format {dxgi} is not read (decode it to PNG)"))),
                };
            } else if pf & 4 != 0 {
                data_offset = 128;
                (kind, fourcc, format_name) = match four {
                    b"DXT1" => (Kind::Dxt, Some("DXT1"), "D3DFMT_DXT1"),
                    b"DXT3" => (Kind::Dxt, Some("DXT3"), "D3DFMT_DXT3"),
                    b"DXT5" => (Kind::Dxt, Some("DXT5"), "D3DFMT_DXT5"),
                    b"ATI2" | b"BC5U" => (Kind::Bc5, None, "D3DFMT_ATI2"),
                    _ => {
                        return Err(bad(format!(
                            "DDS FourCC {} is not read (decode it to PNG)",
                            String::from_utf8_lossy(four)
                        )));
                    }
                };
            } else if pf & 0x40 != 0 && bits == 32 {
                data_offset = 128;
                fourcc = None;
                let alpha = pf & 1 != 0;
                (kind, format_name) = match rmask {
                    0x0000_00FF => (
                        if alpha { Kind::Rgba } else { Kind::Rgbx },
                        if alpha { "A8B8G8R8" } else { "X8B8G8R8" },
                    ),
                    0x00FF_0000 => (
                        if alpha { Kind::Bgra } else { Kind::Bgrx },
                        if alpha { "A8R8G8B8" } else { "X8R8G8B8" },
                    ),
                    _ => return Err(bad(format!("32-bit DDS with red mask {rmask:08x}"))),
                };
            } else {
                return Err(bad(format!(
                    "DDS pixel format not read (flags {pf:x}, {bits} bits)"
                )));
            }
            let format_name = format_name.to_owned();
            return Ok(Self::finish(
                Origin::File {
                    name: name.to_owned(),
                    bytes,
                },
                width,
                height,
                kind,
                fourcc,
                format_name,
                file_levels,
                data_offset,
                source_format,
                source_size,
            ));
        }
        Ok(Self::finish(
            Origin::File {
                name: name.to_owned(),
                bytes,
            },
            width,
            height,
            kind,
            fourcc,
            format_name.to_owned(),
            file_levels,
            data_offset,
            source_format,
            source_size,
        ))
    }

    /// A texture of the world, its levels as stored: BC1/2/3 passed through as DXT1/3/5, BC5 and 8-bit RGBA/BGRA
    /// decoded as a DDS file's are, any other format decoded to RGBA8 level by level. `name` labels its errors.
    pub fn from_texture(
        name: &str,
        t: &Texture,
        source_format: Option<&str>,
        source_size: Option<[i32; 2]>,
    ) -> Result<TexSource> {
        let stored =
            |kind, fourcc, format_name: &str| (kind, fourcc, format_name.to_owned(), t.levels.clone());
        let (kind, fourcc, format_name, levels) = match t.format {
            TexFormat::Bc1 => stored(Kind::Dxt, Some("DXT1"), "D3DFMT_DXT1"),
            TexFormat::Bc2 => stored(Kind::Dxt, Some("DXT3"), "D3DFMT_DXT3"),
            TexFormat::Bc3 => stored(Kind::Dxt, Some("DXT5"), "D3DFMT_DXT5"),
            TexFormat::Bc5Unorm => stored(Kind::Bc5, None, "D3DFMT_ATI2"),
            TexFormat::Rgba8Unorm => stored(Kind::Rgba, None, "R8G8B8A8"),
            TexFormat::Bgra8Unorm => stored(Kind::Bgra, None, "B8G8R8A8"),
            TexFormat::Bgrx8Unorm => stored(Kind::Bgrx, None, "X8R8G8B8"),
            other => {
                let dec = Decoder::default();
                let (mut w, mut h) = (t.width, t.height);
                let mut levels = Vec::with_capacity(t.levels.len());
                for level in &t.levels {
                    levels.push(
                        dec.decode_rgba8(other, w, h, level)
                            .map_err(|e| Error::Texture(format!("{name}: {e}")))?,
                    );
                    w = (w >> 1).max(1);
                    h = (h >> 1).max(1);
                }
                (Kind::Rgba, None, format!("{other:?}"), levels)
            }
        };
        let file_levels = levels.len() as u32;
        Ok(Self::finish(
            Origin::Memory {
                name: name.to_owned(),
                levels,
            },
            t.width,
            t.height,
            kind,
            fourcc,
            format_name,
            file_levels,
            0,
            source_format,
            source_size,
        ))
    }

    /// The source's name in messages: its file, or its name in the world.
    pub fn label(&self) -> String {
        match &self.origin {
            Origin::File { name, .. } | Origin::Memory { name, .. } => name.clone(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        origin: Origin,
        width: u32,
        height: u32,
        kind: Kind,
        fourcc: Option<&'static str>,
        format_name: String,
        file_levels: u32,
        data_offset: usize,
        source_format: Option<&str>,
        source_size: Option<[i32; 2]>,
    ) -> TexSource {
        let mut levels = 1;
        let (mut w, mut h) = (width, height);
        while w > 1 || h > 1 {
            w = (w >> 1).max(1);
            h = (h >> 1).max(1);
            levels += 1;
        }
        TexSource {
            origin,
            width,
            height,
            levels,
            fourcc,
            format_name: source_format.map_or(format_name, str::to_owned),
            source_width: source_size.map_or(width, |s| s[0] as u32),
            source_height: source_size.map_or(height, |s| s[1] as u32),
            kind,
            file_levels,
            data_offset,
            made: Mutex::new(HashMap::new()),
        }
    }

    /// Levels the passthrough data holds.
    pub fn data_levels(&self) -> u32 {
        if self.kind == Kind::Dxt {
            self.file_levels
        } else {
            0
        }
    }

    /// Block data of every level, level 0 first (DXT only).
    pub fn data(&self) -> Result<Option<Vec<u8>>> {
        if self.kind != Kind::Dxt {
            return Ok(None);
        }
        match &self.origin {
            Origin::File { bytes, .. } => Ok(Some(bytes[self.data_offset.min(bytes.len())..].to_vec())),
            Origin::Memory { levels, .. } => Ok(Some(levels.concat())),
        }
    }

    fn level_size(&self, w: u32, h: u32) -> u64 {
        match self.kind {
            Kind::Dxt => level_bytes(w, h, if self.fourcc == Some("DXT1") { 8 } else { 16 }),
            Kind::Bc5 => level_bytes(w, h, 16),
            _ => u64::from(w) * u64::from(h) * 4,
        }
    }

    /// Level `mip` (`w` x `h`) as the source stores it (a PNG decoded to RGBA8); `None` when it does not hold it.
    fn stored_level(&self, mip: u32, w: u32, h: u32) -> Result<Option<Vec<u8>>> {
        if mip >= self.file_levels {
            return Ok(None);
        }
        let len = self.level_size(w, h);
        let (name, b) = match &self.origin {
            Origin::Memory { levels, .. } => {
                return Ok(levels
                    .get(mip as usize)
                    .filter(|l| l.len() as u64 >= len)
                    .map(|l| l[..len as usize].to_vec()));
            }
            Origin::File { name, bytes } => (name, bytes),
        };
        if self.kind == Kind::Png {
            let png = primase_image::read_png(b).map_err(|e| Error::Texture(format!("{name}: {e}")))?;
            return match png.image.samples {
                primase_image::Samples::U8(v) => Ok(Some(v)),
                primase_image::Samples::U16(_) => Err(Error::Texture(format!("{name}: 16-bit PNG"))),
            };
        }
        let mut off = self.data_offset as u64;
        let (mut lw, mut lh) = (self.width, self.height);
        for _ in 0..mip {
            off += self.level_size(lw, lh);
            lw = (lw >> 1).max(1);
            lh = (lh >> 1).max(1);
        }
        if off + len > b.len() as u64 {
            return Ok(None);
        }
        Ok(Some(b[off as usize..(off + len) as usize].to_vec()))
    }

    /// A level the source holds, decoded; `None` when it is not in the data.
    fn file_level(&self, mip: u32, w: u32, h: u32) -> Result<Option<Vec<u8>>> {
        let Some(mut d) = self.stored_level(mip, w, h)? else {
            return Ok(None);
        };
        if self.kind == Kind::Png {
            return Ok(Some(d));
        }
        let dec = Decoder::default();
        let err = |e: primase_texcodec::Error| Error::Texture(format!("{}: {e}", self.label()));
        Ok(Some(match self.kind {
            Kind::Dxt => {
                let f = match self.fourcc {
                    Some("DXT1") => TexFormat::Bc1,
                    Some("DXT3") => TexFormat::Bc2,
                    _ => TexFormat::Bc3,
                };
                dec.decode_rgba8(f, w, h, &d).map_err(err)?
            }
            Kind::Bc5 => {
                let mut px = dec.decode_rgba8(TexFormat::Bc5Unorm, w, h, &d).map_err(err)?;
                for p in px.as_chunks_mut::<4>().0 {
                    p[2] = 0;
                    p[3] = 255;
                }
                px
            }
            Kind::Rgba => d,
            Kind::Rgbx => {
                for p in d.as_chunks_mut::<4>().0 {
                    p[3] = 255;
                }
                d
            }
            Kind::Bgra | Kind::Bgrx => {
                for p in d.as_chunks_mut::<4>().0 {
                    p.swap(0, 2);
                    if self.kind == Kind::Bgrx {
                        p[3] = 255;
                    }
                }
                d
            }
            Kind::Png => unreachable!(),
        }))
    }

    /// Level `mip` (`w` x `h`) as RGBA8; levels the file lacks are box-filtered from the one above.
    pub fn decode(&self, mip: u32, w: u32, h: u32) -> Result<Option<Vec<u8>>> {
        if let Some(px) = self.file_level(mip, w, h)? {
            return Ok(Some(px));
        }
        if mip == 0 {
            return Ok(None);
        }
        if let Some(m) = self.made.lock().expect("lock").get(&mip) {
            return Ok(Some(m.clone()));
        }
        let (mut pw, mut ph) = (self.width, self.height);
        for _ in 0..mip - 1 {
            pw = (pw >> 1).max(1);
            ph = (ph >> 1).max(1);
        }
        let Some(prev) = self.decode(mip - 1, pw, ph)? else {
            return Ok(None);
        };
        let half = half(pw, ph, prev);
        self.made.lock().expect("lock").insert(mip, half.clone());
        Ok(Some(half))
    }
}

/// The next smaller level by the 2 x 2 box filter (odd sizes: the last row / column takes three).
pub fn half(w: u32, h: u32, rgba: Vec<u8>) -> Vec<u8> {
    let img = primase_image::Image {
        width: w,
        height: h,
        samples: primase_image::Samples::U8(rgba),
    };
    match primase_image::half(&img).expect("valid image").samples {
        primase_image::Samples::U8(v) => v,
        primase_image::Samples::U16(_) => unreachable!("8-bit in, 8-bit out"),
    }
}

/// A source texture with what the writer needs to know about it.
#[derive(Debug)]
pub struct TexRef {
    pub src: std::sync::Arc<TexSource>,
    /// Identity of the source texture (the scene's texture key).
    pub key: u64,
    /// Output file stem and caps key (lower case).
    pub name: String,
    pub has_alpha: bool,
    pub linear: bool,
}

/// One texture file of the map.
#[derive(Clone, Debug)]
pub struct OutTexture {
    pub id: usize,
    pub content: u64,
    pub name: String,
    pub file: String,
    pub format: String,
    pub w: u32,
    pub h: u32,
    pub alpha: bool,
    pub bytes: u64,
    pub mean: u32,
    pub removed: bool,
    pub source_w: u32,
    pub source_h: u32,
    pub source_format: String,
    pub source_variants: BTreeSet<String>,
    pub sources: BTreeSet<String>,
    data: Vec<u8>,
}

/// The texture files of a map (the reference writer's TexLib): each texture as prepared (its size, its pixels), encoded
/// once per content; the writer chooses no size and changes no pixel (caps and tints are passes: `cap-textures`,
/// `prepare-reskate-textures`).
#[derive(Debug)]
pub struct TexLib {
    /// Encode the files (`false`: ids only, by the same content keys: what a pass needs of the writer's materials).
    encode: bool,
    by_key: HashMap<(u64, bool), usize>,
    by_content: HashMap<u64, usize>,
    out: Vec<OutTexture>,
    pub unsupported: u64,
}

/// Hash tags of [`TexLib::register_pixels`].
pub const NORMAL_TAG: u32 = 0x4e52_4d4c;
pub const MASK_TAG: u32 = 0x4d53_4b31;

/// `TexLib.MeanColor`: alpha-weighted mean 0xRRGGBB.
pub fn mean_color(rgba: &[u8]) -> u32 {
    let (mut r, mut g, mut b, mut wsum) = (0f64, 0f64, 0f64, 0f64);
    for p in rgba.as_chunks::<4>().0 {
        let a = f64::from(p[3]) / 255.0;
        r += f64::from(p[0]) * a;
        g += f64::from(p[1]) * a;
        b += f64::from(p[2]) * a;
        wsum += a;
    }
    if wsum < 1e-6 {
        return 0x808080;
    }
    (((r / wsum) as i32) << 16 | ((g / wsum) as i32) << 8 | (b / wsum) as i32) as u32
}

/// The largest level whose larger side is at most `cap` (or the smallest there is): the level a passed-through
/// texture's mean colour is measured on.
fn mip_for(width: u32, height: u32, levels: u32, cap: u32) -> (u32, u32, u32) {
    let (mut w, mut h, mut mip) = (width, height, 0);
    let levels = levels.max(1);
    while mip < levels - 1 && w.max(h) > cap {
        w = (w >> 1).max(1);
        h = (h >> 1).max(1);
        mip += 1;
    }
    (mip, w, h)
}

fn png_bytes(w: u32, h: u32, rgba: &[u8], alpha: bool) -> Vec<u8> {
    if alpha {
        primase_image::write_png(&primase_image::Image {
            width: w,
            height: h,
            samples: primase_image::Samples::U8(rgba.to_vec()),
        })
        .expect("valid image")
    } else {
        let rgb: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        primase_image::write_png_rgb(w, h, &rgb).expect("valid image")
    }
}

/// A file stem: letters, digits, `_` and `-` kept, the rest `_`, at most `max` characters.
fn safe_name(name: &str, keep_dash: bool, max: usize) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || (keep_dash && c == '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.chars().count() > max {
        s = s.chars().take(max).collect();
    }
    s
}

pub(crate) fn safe_label(label: &str) -> String {
    safe_name(label, false, 35)
}

fn file_of(name: &str, content: u64, ext: &str) -> String {
    format!("{name}_{:08x}{ext}", (content ^ (content >> 32)) as u32)
}

/// The decoded pixels of a level.
#[derive(Clone, Debug)]
pub struct Pixels {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl TexLib {
    /// Ids only: every texture identified by the same content keys as when encoding, its file not encoded (nothing to
    /// save).
    pub fn without_encoding(mut self) -> TexLib {
        self.encode = false;
        self
    }

    /// A library that encodes its files (each texture once per content), ready to give them to the writer.
    pub fn encoding() -> TexLib {
        TexLib {
            encode: true,
            by_key: HashMap::new(),
            by_content: HashMap::new(),
            out: Vec::new(),
            unsupported: 0,
        }
    }

    pub fn all(&self) -> &[OutTexture] {
        &self.out
    }

    pub fn mean_of(&self, id: i32) -> u32 {
        if id >= 0 && (id as usize) < self.out.len() {
            self.out[id as usize].mean
        } else {
            0x808080
        }
    }

    pub fn content_of(&self, id: i32) -> u64 {
        if id >= 0 && (id as usize) < self.out.len() {
            self.out[id as usize].content
        } else {
            0
        }
    }

    /// The texture as prepared (its first level), as RGBA8 (`None` counts as unsupported).
    pub fn decode(&mut self, tex: &TexSource) -> Result<Option<Pixels>> {
        self.decode_within(tex, u32::MAX)
    }

    /// The texture's level whose larger side is at most `cap`, as RGBA8 (`None` counts as unsupported).
    fn decode_within(&mut self, tex: &TexSource, cap: u32) -> Result<Option<Pixels>> {
        let (mip, w, h) = mip_for(tex.width, tex.height, tex.levels, cap);
        let px = tex.decode(mip, w, h).unwrap_or(None);
        match px {
            Some(px) if px.len() >= (w * h * 4) as usize => Ok(Some(Pixels { w, h, rgba: px })),
            _ => {
                self.unsupported += 1;
                Ok(None)
            }
        }
    }

    /// Texture id of `r` as prepared, written on first use (with alpha or without, as the material draws it).
    pub fn register(&mut self, r: &TexRef, alpha: bool) -> Result<i32> {
        let alpha = alpha && r.has_alpha;
        if let Some(&id) = self.by_key.get(&(r.key, alpha)) {
            return Ok(id as i32);
        }
        let id = self.write(r, alpha)?;
        if id >= 0 {
            self.by_key.insert((r.key, alpha), id as usize);
        }
        Ok(id)
    }

    /// A 4 x 4 texture of one colour.
    pub fn register_flat(&mut self, rgb: u32, a: u8) -> i32 {
        let key = 0x4000_0000_0000_0000u64 | (u64::from(a) << 24) | u64::from(rgb);
        if let Some(&id) = self.by_key.get(&(key, a < 255)) {
            return id as i32;
        }
        let mut px = vec![0u8; 64];
        for p in px.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&[(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, a]);
        }
        let data = png_bytes(4, 4, &px, a < 255);
        let id = self.add(
            &format!("flat_{rgb:06x}_{a:02x}"),
            4,
            4,
            a < 255,
            key,
            "png",
            None,
            data,
            ".png",
            rgb,
        );
        self.by_key.insert((key, a < 255), id);
        id as i32
    }

    /// Pixels made for one role (normal map, Studio MSK), written as PNG named `name`, identified by tag and pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn register_pixels(
        &mut self,
        name: &str,
        tag: u32,
        w: u32,
        h: u32,
        rgba: &[u8],
        src: &TexSource,
        mean: u32,
    ) -> i32 {
        let mut hh = Hash64::new();
        hh.uint(tag).int(w as i32).int(h as i32).bytes(rgba);
        // a Studio MSK keeps its alpha (opacity) when it has any
        let alpha = tag == MASK_TAG && rgba.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
        let data = if self.encode {
            png_bytes(w, h, rgba, alpha)
        } else {
            Vec::new()
        };
        self.add(
            name,
            w,
            h,
            alpha,
            hh.value(),
            "png",
            Some(src),
            data,
            ".png",
            mean,
        ) as i32
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        name: &str,
        w: u32,
        h: u32,
        alpha: bool,
        content: u64,
        format: &str,
        src: Option<&TexSource>,
        data: Vec<u8>,
        ext: &str,
        mean: u32,
    ) -> usize {
        let safe = safe_name(name, true, 48);
        if let Some(&known) = self.by_content.get(&content) {
            let k = &mut self.out[known];
            if safe.as_str() < k.name.as_str() {
                k.name = safe;
            }
            k.sources.insert(name.to_owned());
            if let Some(src) = src {
                let f = &src.format_name;
                let (sw, sh) = (src.source_width, src.source_height);
                k.source_variants.insert(format!("{sw}x{sh} {f}"));
                let (a0, a1) = (
                    u64::from(k.source_w) * u64::from(k.source_h),
                    u64::from(sw) * u64::from(sh),
                );
                if a1 > a0
                    || (a1 == a0
                        && (sw > k.source_w || (sw == k.source_w && f.as_str() > k.source_format.as_str())))
                {
                    k.source_w = sw;
                    k.source_h = sh;
                    k.source_format = f.clone();
                }
            }
            return known;
        }
        let id = self.out.len();
        let mut t = OutTexture {
            id,
            content,
            name: safe.clone(),
            file: file_of(&safe, content, ext),
            format: format.to_owned(),
            w,
            h,
            alpha,
            bytes: data.len() as u64,
            mean,
            removed: false,
            source_w: src.map_or(w, |s| s.source_width),
            source_h: src.map_or(h, |s| s.source_height),
            source_format: src.map_or_else(String::new, |s| s.format_name.clone()),
            source_variants: BTreeSet::new(),
            sources: BTreeSet::new(),
            data,
        };
        t.sources.insert(name.to_owned());
        if src.is_some() {
            t.source_variants
                .insert(format!("{}x{} {}", t.source_w, t.source_h, t.source_format));
        }
        self.out.push(t);
        self.by_content.insert(content, id);
        id
    }

    fn write(&mut self, r: &TexRef, alpha: bool) -> Result<i32> {
        let tex = &*r.src;
        let (mip, w, h) = (0, tex.width, tex.height);
        if w >= 4
            && h >= 4
            && let Some(four) = tex.fourcc
        {
            let block: u64 = if four == "DXT1" { 8 } else { 16 };
            let data = tex.data()?.unwrap_or_default();
            let mut off = 0u64;
            let (mut lw, mut lh) = (tex.width, tex.height);
            for _ in 0..mip {
                off += level_bytes(lw, lh, block);
                lw = (lw >> 1).max(1);
                lh = (lh >> 1).max(1);
            }
            // keep the levels that are really in the data, down to 4 px
            let (mut levels, mut len) = (0u32, 0u64);
            let (mut lw, mut lh) = (w, h);
            let have = tex.data_levels().max(1) as i64 - i64::from(mip);
            let mut level_data: Vec<Vec<u8>> = Vec::new();
            while i64::from(levels) < have && lw >= 4 && lh >= 4 {
                let sz = level_bytes(lw, lh, block);
                if off + len + sz > data.len() as u64 {
                    break;
                }
                level_data.push(data[(off + len) as usize..(off + len + sz) as usize].to_vec());
                len += sz;
                levels += 1;
                lw >>= 1;
                lh >>= 1;
            }
            if levels > 0 {
                let mut hh = Hash64::new();
                hh.int(w as i32).int(h as i32).string(four).int(i32::from(alpha));
                hh.bytes(&data[off as usize..(off + len) as usize]);
                let mean0 = if self.encode {
                    let small = self.decode_within(tex, 32u32.max(w.max(h) >> (levels - 1)))?;
                    small.map_or(0x808080, |p| mean_color(&p.rgba))
                } else {
                    0x808080
                };
                let format = match four {
                    "DXT1" => TexFormat::Bc1,
                    "DXT3" => TexFormat::Bc2,
                    _ => TexFormat::Bc3,
                };
                let dds = if self.encode {
                    primase_dds::write_legacy(&Texture {
                        width: w,
                        height: h,
                        format,
                        levels: level_data,
                        color_space: ColorSpace::Srgb,
                        role: TexRole::Color,
                        normal: None,
                    })
                    .map_err(|e| Error::Texture(format!("{}: {e}", tex.label())))?
                } else {
                    Vec::new()
                };
                return Ok(self.add(
                    &r.name,
                    w,
                    h,
                    alpha,
                    hh.value(),
                    four,
                    Some(tex),
                    dds,
                    ".dds",
                    mean0,
                ) as i32);
            }
        }
        let Some(p) = self.decode(tex)? else {
            return Ok(-1);
        };
        let rgba = p.rgba;
        let mut ch = Hash64::new();
        ch.int(p.w as i32).int(p.h as i32).int(i32::from(alpha));
        if alpha {
            ch.bytes(&rgba);
        } else {
            for px in rgba.as_chunks::<4>().0 {
                ch.uint(u32::from(px[0]) | u32::from(px[1]) << 8 | u32::from(px[2]) << 16);
            }
        }
        let data = if self.encode {
            png_bytes(p.w, p.h, &rgba, alpha)
        } else {
            Vec::new()
        };
        let mean = mean_color(&rgba);
        Ok(self.add(
            &r.name,
            p.w,
            p.h,
            alpha,
            ch.value(),
            "png",
            Some(tex),
            data,
            ".png",
            mean,
        ) as i32)
    }

    /// After the last register: a file whose pixels arrived under several names takes its final name.
    pub fn finish(&mut self) {
        for t in &mut self.out {
            let ext = Path::new(&t.file)
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            t.file = file_of(&t.name, t.content, &ext);
        }
    }

    /// Marks the textures whose id is not in `used` removed (not written); returns how many.
    pub fn prune(&mut self, used: &BTreeSet<i32>) -> u64 {
        let mut n = 0;
        for t in &mut self.out {
            if t.removed || used.contains(&(t.id as i32)) {
                continue;
            }
            t.removed = true;
            n += 1;
        }
        n
    }

    /// The files to write, those not removed: each one's name (under `textures/`) and bytes, moved out (the library
    /// keeps what it reports of them). A library that encodes nothing has none to give (an error).
    pub fn take_files(&mut self) -> Result<Vec<(String, Vec<u8>)>> {
        if !self.encode {
            return Err(Error::Texture(
                "a texture library that encodes nothing has no files to write".into(),
            ));
        }
        Ok(self
            .out
            .iter_mut()
            .filter(|t| !t.removed)
            .map(|t| (t.file.clone(), std::mem::take(&mut t.data)))
            .collect())
    }

    /// Texture files by name, for reports.
    pub fn by_file(&self) -> BTreeMap<&str, &OutTexture> {
        self.out
            .iter()
            .filter(|t| !t.removed)
            .map(|t| (t.file.as_str(), t))
            .collect()
    }
}
