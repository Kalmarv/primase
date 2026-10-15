use crate::{Error, Image, Result, Samples, read_png, write_png};
use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    ColorSpace, Entity, NormalConvention, NormalGreen, NormalZ, TexFormat, TexRole, Texture, TextureFile,
    World,
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
            let texture = if bytes.starts_with(b"DDS ") {
                primase_dds::read(&bytes)?
            } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                let png = read_png(&bytes)?;
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
                return Err(Error::Invalid(format!(
                    "{}: unsupported texture signature",
                    path.display()
                )));
            };
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
/// Declared loader pass. Empty base_dir resolves relative paths from the caller's
/// working directory; readers should prefer fully resolved TextureFile paths.
#[derive(Debug, Clone, Default)]
pub struct LoadTextures {
    pub base_dir: PathBuf,
}
impl Pass for LoadTextures {
    fn name(&self) -> &'static str {
        "load-textures"
    }
    fn options(&self) -> Options {
        if self.base_dir.as_os_str().is_empty() {
            Options::new()
        } else {
            Options::from([("base_dir".into(), self.base_dir.to_string_lossy().into_owned())])
        }
    }
    fn requires(&self) -> &[&'static str] {
        &["core.texture_file"]
    }
    fn provides(&self) -> &[&'static str] {
        &["core.texture"]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = load_textures(world, &self.base_dir).map_err(|e| e.to_string())?;
        log.count("textures", r.textures);
        log.count("files", r.files);
        for (k, n) in r.ignored_ancillary {
            log.approximate(&format!("PNG metadata {k} not represented"), n);
        }
        Ok(())
    }
}
fn level_dimensions(t: &Texture, level: usize) -> Result<(u32, u32)> {
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
    let (w, h) = level_dimensions(t, level)?;
    let rgba = Decoder::default().decode_rgba8(t.format, w, h, &t.levels[level])?;
    write_png(&Image {
        width: w,
        height: h,
        samples: Samples::U8(rgba),
    })
}
/// Explicitly convert normal maps to green-up with stored positive-hemisphere Z.
/// Green flips as 255-G; Z=sqrt(max(0,1-x*x-y*y)), x/y=2*byte/255-1,
/// encoded with nearest rounding. Every stored mip is decoded to RGBA8; alpha
/// and colour-space metadata stay unchanged. Already-conforming textures remain
/// byte-identical and keep file provenance. Missing normal metadata is an error.
pub fn normalize_normals(world: &mut World) -> Result<PassLog> {
    let mut log = PassLog::default();
    let mut changes = Vec::<(Entity, Texture)>::new();
    for (e, t) in world.iter::<Texture>().filter(|(_, t)| t.role == TexRole::Normal) {
        let convention = t
            .normal
            .ok_or_else(|| Error::Invalid(format!("normal texture {e:?} has no convention")))?;
        if convention
            == (NormalConvention {
                green: NormalGreen::Up,
                z: NormalZ::Stored,
            })
        {
            continue;
        }
        let mut levels = Vec::new();
        for i in 0..t.levels.len() {
            let (w, h) = level_dimensions(t, i)?;
            let mut rgba = Decoder::default().decode_rgba8(t.format, w, h, &t.levels[i])?;
            for p in rgba.as_chunks_mut::<4>().0 {
                if convention.green == NormalGreen::Down {
                    p[1] = 255 - p[1];
                }
                if convention.z == NormalZ::Rebuild {
                    let x = p[0] as f64 / 127.5 - 1.;
                    let y = p[1] as f64 / 127.5 - 1.;
                    let z = (1. - x * x - y * y).max(0.).sqrt();
                    p[2] = ((z * 0.5 + 0.5) * 255.).round() as u8;
                }
            }
            levels.push(rgba);
        }
        // Validate even an invalid empty input chain before mutating anything.
        level_dimensions(t, 0)?;
        let mut converted = t.clone();
        converted.format = TexFormat::Rgba8Unorm;
        converted.levels = levels;
        converted.normal = Some(NormalConvention {
            green: NormalGreen::Up,
            z: NormalZ::Stored,
        });
        changes.push((e, converted));
        log.count("normal textures converted", 1);
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
#[derive(Debug, Clone, Copy, Default)]
pub struct NormalizeNormals;
impl Pass for NormalizeNormals {
    fn name(&self) -> &'static str {
        "normalize-normals"
    }
    fn requires(&self) -> &[&'static str] {
        &["core.texture"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.texture"]
    }
    fn removes(&self) -> &[&'static str] {
        &["core.texture_file"]
    }
    fn approximates(&self) -> &'static str {
        "positive-hemisphere Z reconstruction; changed normal levels decoded to RGBA8"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let r = normalize_normals(world).map_err(|e| e.to_string())?;
        for (k, n) in r.counts {
            log.count(&k, n);
        }
        for (k, n) in r.approximations {
            log.approximate(&k, n);
        }
        Ok(())
    }
}
