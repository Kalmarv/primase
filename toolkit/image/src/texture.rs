use crate::{Error, Image, Result, Samples, read_png, write_png};
use primase_core::parallel::Threads;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{
    ColorSpace, Entity, NormalBasis, NormalConvention, NormalGreen, NormalX, NormalZ, TexFormat, TexRole,
    Texture, TextureFile, World,
};
use primase_texcodec::{Decoder, TexDecoder};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextureLoadReport {
    pub textures: u64,
    pub files: u64,
    /// PNG ancillary metadata consumed but not represented by core::Texture.
    pub ignored_ancillary: BTreeMap<String, u64>,
}
/// Load missing core::Texture components from PNG or DDS files. Absolute paths
/// are used directly; relative paths resolve under base_dir. Existing textures
/// are authoritative. TextureFile stays as provenance while pixels are unchanged.
/// DDS retains stored levels and colour-space tags; PNG retains 8/16-bit samples,
/// defaults to sRGB, and performs no transfer. Readers/lowerings own semantic
/// role and normal metadata. Validation completes before the world is mutated.
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "toolkit I/O: a reader's load step reads the PNG and DDS files its world references"
)]
pub fn load_textures(world: &mut World, base_dir: &Path) -> Result<TextureLoadReport> {
    let jobs: Vec<_> = world
        .iter::<TextureFile>()
        .filter(|(e, _)| !world.has::<Texture>(*e))
        .map(|(e, f)| (e, f.path.clone()))
        .collect();
    let mut files = BTreeMap::<PathBuf, Texture>::new();
    let mut prepared = Vec::new();
    let mut report = TextureLoadReport::default();
    for (e, file) in jobs {
        let path = if file.is_absolute() {
            file
        } else {
            base_dir.join(file)
        };
        if !files.contains_key(&path) {
            let bytes = std::fs::read(&path).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
            let texture = decode_texture_file(&bytes, &path.display().to_string(), &mut report)?;
            files.insert(path.clone(), texture);
            report.files += 1;
        }
        prepared.push((e, files[&path].clone()));
        report.textures += 1;
    }
    for (e, t) in prepared {
        world.insert(e, t);
    }
    Ok(report)
}
/// One texture file's bytes (PNG or DDS, told by their signature) decoded as [`load_textures`] decodes them: DDS keeps
/// its stored levels and colour-space tags; PNG its 8- or 16-bit samples, sRGB, no transfer. `what` names the file in
/// errors; PNG ancillary chunks it skips are counted in `report`.
pub fn decode_texture_file(bytes: &[u8], what: &str, report: &mut TextureLoadReport) -> Result<Texture> {
    Ok(if bytes.starts_with(b"DDS ") {
        primase_dds::read(bytes)?
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let png = read_png(bytes)?;
        for kind in png.ignored_ancillary {
            *report
                .ignored_ancillary
                .entry(String::from_utf8_lossy(&kind).into_owned())
                .or_default() += 1;
        }
        let (format, level) = match png.image.samples {
            Samples::U8(v) => (TexFormat::Rgba8Unorm, v),
            Samples::U16(v) => (
                TexFormat::Rgba16Unorm,
                v.into_iter().flat_map(|v| v.to_le_bytes()).collect(),
            ),
        };
        Texture {
            width: png.image.width,
            height: png.image.height,
            format,
            levels: vec![level],
            color_space: ColorSpace::Srgb,
            role: TexRole::Other,
            normal: None,
        }
    } else {
        return Err(Error::Invalid(format!("{what}: unsupported texture signature")));
    })
}

pub(crate) fn level_dimensions(t: &Texture, level: usize) -> Result<(u32, u32)> {
    if t.width == 0
        || t.height == 0
        || t.levels.is_empty()
        || t.levels.len() > (32 - t.width.max(t.height).leading_zeros()) as usize
        || level >= t.levels.len()
    {
        return Err(Error::Invalid(
            "invalid texture dimensions, mip count or selected level".into(),
        ));
    }
    Ok(((t.width >> level).max(1), (t.height >> level).max(1)))
}
/// Decode one stored level using the default bcdec arithmetic and emit RGBA8 PNG.
/// No transfer function, normal-map conversion, or mip filtering is implicit.
/// Float/HDR values follow TexDecoder's documented clamped RGBA8 projection.
pub fn texture_level_png(t: &Texture, level: usize) -> Result<Vec<u8>> {
    write_png(&texture_level_rgba8(t, level)?)
}

/// Level `level` of `t` decoded to RGBA8, the pixels [`texture_level_png`] writes.
pub fn texture_level_rgba8(t: &Texture, level: usize) -> Result<Image> {
    texture_level_rgba8_with(Threads::ONE, t, level)
}

/// [`texture_level_rgba8`] decoded on `threads` threads (a row of blocks each at a time): the same pixels.
pub fn texture_level_rgba8_with(threads: Threads, t: &Texture, level: usize) -> Result<Image> {
    let (w, h) = level_dimensions(t, level)?;
    let rgba = Decoder::with_threads(threads).decode_rgba8(t.format, w, h, &t.levels[level])?;
    Ok(Image {
        width: w,
        height: h,
        samples: Samples::U8(rgba),
    })
}
/// Explicitly convert normal maps to glTF's convention: X in red, green up, stored
/// positive-hemisphere Z. X kept in alpha moves to red (alpha unchanged); green flips as 255-G; Z=sqrt(max(0,1-x*x-y*y)), x/y=2*byte/255-1,
/// encoded with nearest, ties-even rounding. Bitangent/tangent source coordinates swap
/// after source-channel extraction and sign decoding. Mesh tangents must describe
/// the same frame; this pass does not regenerate tangents from UV coordinates.
/// Every stored mip is decoded to RGBA8; alpha
/// and colour-space metadata stay unchanged. Already-conforming textures remain
/// byte-identical and keep file provenance. Missing normal metadata is an error. The textures are converted on the
/// world's stated threads (`core.threads`, one texture per thread at a time): the same result on any count.
pub fn normalize_normals(world: &mut World) -> Result<PassLog> {
    let mut log = PassLog::default();
    let mut todo = Vec::<(Entity, &Texture, NormalConvention)>::new();
    for (e, t) in world.iter::<Texture>().filter(|(_, t)| t.role == TexRole::Normal) {
        let convention = t
            .normal
            .ok_or_else(|| Error::Invalid(format!("normal texture {e:?} has no convention")))?;
        if convention != NormalConvention::GLTF {
            todo.push((e, t, convention));
        }
    }
    // each texture converted on its own (at most the stated threads at once), the counts added in the textures' order
    let converted = primase_core::parallel::map(Threads::of(world), &todo, |_, &(_, t, convention)| {
        convert_normals(t, convention)
    })
    .map_err(|e| Error::Invalid(e.to_string()))?;
    let mut changes = Vec::<(Entity, Texture)>::with_capacity(todo.len());
    for (&(e, t, convention), converted) in todo.iter().zip(converted) {
        let (converted, projected) = converted?;
        if projected > 0 {
            log.count("normal pixels projected onto unit disk", projected);
        }
        changes.push((e, converted));
        log.count("normal textures converted", 1);
        if convention.basis == NormalBasis::BitangentTangent {
            log.count("normal tangent-plane axes swapped", 1);
        }
        if convention.x == NormalX::Alpha {
            log.count("x moved from alpha to red", 1);
        }
        if convention.green == NormalGreen::Down {
            log.count("green flipped", 1);
        }
        if convention.z == NormalZ::Rebuild {
            log.approximate("positive-hemisphere Z reconstructed", 1);
        }
        if t.format != TexFormat::Rgba8Unorm {
            log.approximate("normal levels decoded to RGBA8", t.levels.len() as u64);
        }
    }
    for (e, t) in changes {
        world.insert(e, t);
        world.remove::<TextureFile>(e);
    }
    Ok(log)
}

/// `t` in glTF's normal convention from `convention`, every level decoded to RGBA8, and how many pixels were projected
/// onto the unit disk.
fn convert_normals(t: &Texture, convention: NormalConvention) -> Result<(Texture, u64)> {
    let mut projected = 0;
    let mut levels = Vec::new();
    for i in 0..t.levels.len() {
        let (w, h) = level_dimensions(t, i)?;
        let mut rgba = Decoder::default().decode_rgba8(t.format, w, h, &t.levels[i])?;
        for p in rgba.as_chunks_mut::<4>().0 {
            if convention.x == NormalX::Alpha {
                p[0] = p[3];
            }
            if convention.green == NormalGreen::Down {
                p[1] = 255 - p[1];
            }
            if convention.basis == NormalBasis::BitangentTangent {
                p.swap(0, 1);
            }
            if convention.z == NormalZ::Rebuild && crate::rebuild_normal_z(p) {
                projected += 1;
            }
        }
        levels.push(rgba);
    }
    // Validate even an invalid empty input chain before mutating anything.
    level_dimensions(t, 0)?;
    let mut converted = t.clone();
    converted.format = TexFormat::Rgba8Unorm;
    converted.levels = levels;
    converted.normal = Some(NormalConvention::GLTF);
    Ok((converted, projected))
}
/// Bake every `pbr.material`'s normal scale into its normal map ([`crate::scale_normal`], glTF's rule) and set the
/// scale to 1, for targets without a normal strength. The normal maps must be in glTF's convention
/// (`normalize-normals` first). Each stored level is decoded to RGBA8; alpha and colour space stay. A texture every
/// binding uses at one scale is baked in place; one bound at several scales gets a baked copy per scale other than 1
/// (named `<name>~normal<scale>`), the bindings repointed. Every check runs before the world changes.
pub fn bake_normal_scale(world: &mut World) -> Result<PassLog> {
    use primase_core::Name;
    use primase_core::pbr::PbrMaterial;
    let mut log = PassLog::default();
    // per normal texture, the scales its bindings use (by bits: a scale is a number, not a value to compare loosely)
    let mut scales = BTreeMap::<Entity, BTreeMap<u32, f32>>::new();
    for (_, m) in world.iter::<PbrMaterial>() {
        if let Some(r) = m.normal_texture {
            if !m.normal_scale.is_finite() {
                return Err(Error::Invalid(format!(
                    "normal scale {} of texture {:?} is not finite",
                    m.normal_scale, r.texture
                )));
            }
            scales
                .entry(r.texture)
                .or_default()
                .insert(m.normal_scale.to_bits(), m.normal_scale);
        }
    }
    // (texture, scale) -> the baked texture's entity (None: in place)
    let mut baked = BTreeMap::<(Entity, u32), Texture>::new();
    for (&t, by_bits) in &scales {
        for (&bits, &scale) in by_bits {
            if scale == 1.0 {
                continue;
            }
            let tex = world.get::<Texture>(t).ok_or_else(|| {
                Error::Invalid(format!("normal texture {t:?} has no pixels (load it first)"))
            })?;
            if tex.normal != Some(NormalConvention::GLTF) {
                return Err(Error::Invalid(format!(
                    "normal texture {t:?} is not in glTF's convention: run normalize-normals first"
                )));
            }
            let mut levels = Vec::with_capacity(tex.levels.len());
            for i in 0..tex.levels.len() {
                let (w, h) = level_dimensions(tex, i)?;
                let mut rgba = Decoder::default().decode_rgba8(tex.format, w, h, &tex.levels[i])?;
                for p in rgba.as_chunks_mut::<4>().0 {
                    if crate::scale_normal(p, scale) {
                        log.count("zero normals made flat", 1);
                    }
                }
                levels.push(rgba);
            }
            if tex.format != TexFormat::Rgba8Unorm {
                log.approximate("normal levels decoded to RGBA8", tex.levels.len() as u64);
            }
            let mut out = tex.clone();
            out.format = TexFormat::Rgba8Unorm;
            out.levels = levels;
            baked.insert((t, bits), out);
        }
    }
    // apply: in place where a texture has one scale, else a copy per scale
    let mut target = BTreeMap::<(Entity, u32), Entity>::new();
    for ((t, bits), tex) in baked {
        let e = if scales[&t].len() == 1 {
            t
        } else {
            let e = world.spawn();
            let name = world
                .get::<Name>(t)
                .map_or_else(|| format!("{t:?}"), |n| n.0.clone());
            world.insert(e, Name(format!("{name}~normal{}", f32::from_bits(bits))));
            log.count("normal textures copied for a second scale", 1);
            e
        };
        world.insert(e, tex);
        world.remove::<TextureFile>(e);
        target.insert((t, bits), e);
        log.count("normal textures baked", 1);
    }
    let materials: Vec<Entity> = world.iter::<PbrMaterial>().map(|(e, _)| e).collect();
    for e in materials {
        let m = world.get_mut::<PbrMaterial>(e).expect("listed");
        let Some(r) = m.normal_texture.as_mut() else {
            continue;
        };
        if let Some(&to) = target.get(&(r.texture, m.normal_scale.to_bits())) {
            r.texture = to;
            m.normal_scale = 1.0;
            log.count("materials with a baked normal scale", 1);
        }
    }
    Ok(log)
}

/// `bake-normal-scale`'s declaration: no options.
pub const BAKE_NORMAL_SCALE: PassSpec = PassSpec {
    name: "bake-normal-scale",
    options: &[],
    make: |_| Ok(Box::new(BakeNormalScale)),
};

/// `bake-normal-scale`: [`bake_normal_scale`] as a pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct BakeNormalScale;
impl Pass for BakeNormalScale {
    fn name(&self) -> &'static str {
        "bake-normal-scale"
    }
    // no `requires`: a world without normal maps has nothing to bake
    fn provides(&self) -> &[&'static str] {
        &["core.texture", "core.name"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.texture", "pbr.material"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.texture_file"]
    }
    fn approximates(&self) -> &'static str {
        "baked normal levels decoded to RGBA8"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = bake_normal_scale(world).map_err(|e| e.to_string())?;
        log.absorb(r);
        Ok(())
    }
}

/// `normalize-normals`' declaration: no options.
pub const NORMALIZE_NORMALS: PassSpec = PassSpec {
    name: "normalize-normals",
    options: &[],
    make: |_| Ok(Box::new(NormalizeNormals)),
};

#[derive(Debug, Clone, Copy, Default)]
pub struct NormalizeNormals;
impl Pass for NormalizeNormals {
    fn name(&self) -> &'static str {
        "normalize-normals"
    }
    // no `requires`: a world without textures (a part of a conversion with none) has nothing to convert
    fn modifies(&self) -> &[&'static str] {
        &["core.texture"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.texture_file"]
    }
    /// It rewrites normal maps in place; the file links it removes no longer match the textures.
    fn legalizes(&self) -> &[&'static str] {
        &["core.texture"]
    }
    fn approximates(&self) -> &'static str {
        "positive-hemisphere Z reconstruction; changed normal levels decoded to RGBA8"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = normalize_normals(world).map_err(|e| e.to_string())?;
        log.absorb(r);
        Ok(())
    }
}
