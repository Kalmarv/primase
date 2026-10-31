//! Named post and volumetric-fog records, exactly as one GPARAM stores them. Raw records preserve missing fields and
//! time keys. Explicit scalar composition is separate in [`composition`]; [`reinhard`] resolves steady map-bank values.
use fromsoft_formats::gparam::{Gparam, ParamExtra, TimeAxis, Value};
use fromsoft_formats::{Archive, Result, dcx::Decoder};

pub mod composition;
pub mod reinhard;

/// One authored value; the hour is absent in GPARAM versions without time keys.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample<T> {
    pub id: i32,
    pub hour: Option<f32>,
    pub value: T,
}

/// A field's samples, in stored order; an empty vector means the file states no value for this field.
pub type Track<T> = Vec<Sample<T>>;

/// Tone and lookup settings before file composition or renderer conversion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tone {
    pub mode: Track<i32>,
    pub reinhard_contrast: Track<f32>,
    pub reinhard_white_balance: Track<f32>,
    pub filmic_contrast: Track<f32>,
    pub exposure: Track<f32>,
    pub grading_enabled: Track<i32>,
    pub lut_source_id: Track<i32>,
}

/// The fog's scalar and colour settings in the file's units. These are not merged renderer constants.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fog {
    pub enabled: Track<bool>,
    pub game_scale: Track<f32>,
    pub near_depth: Track<f32>,
    pub far_depth: Track<f32>,
    pub near_offset: Track<f32>,
    pub fixed_density: Track<f32>,
    pub scattering_g: Track<f32>,
    pub local_light_scale: Track<f32>,
    pub local_light_dist_scale: Track<f32>,
    pub height_distribution: Track<f32>,
    pub height_offset: Track<f32>,
    pub height_density_scale: Track<f32>,
    pub sky_colour: Track<[f32; 4]>,
    pub sky_start_dist: Track<f32>,
    pub directional_light_bank: Track<u32>,
    pub directional_direct_colour: Track<[f32; 4]>,
    pub directional_opposite_colour: Track<[f32; 4]>,
    pub directional_start_dist: Track<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Records {
    pub version: u32,
    /// The stored keys' time axis, needed before treating a key as an hour of the game clock.
    pub time: Option<TimeAxis>,
    /// The named groups' extra records, kept as stored. Absent and present-with-no-IDs are distinct; these are not
    /// interpreted as activation flags or file precedence.
    pub tone_extra: Option<ParamExtra>,
    pub grading_extra: Option<ParamExtra>,
    pub fog_extra: Option<ParamExtra>,
    pub tone: Tone,
    pub fog: Fog,
}

fn track<T>(
    gparam: &Gparam,
    group: &str,
    key: &str,
    convert: impl Fn(Value) -> Option<T>,
) -> Result<Track<T>> {
    let Some(field) = gparam.group(group).and_then(|g| g.field(key)) else {
        return Ok(Vec::new());
    };
    field
        .ids
        .iter()
        .enumerate()
        .map(|(index, &id)| {
            let value = field.values.get(index).and_then(&convert).ok_or_else(|| {
                crate::invalid(
                    "GPARAM post",
                    index,
                    format!("{group}: {key}: unexpected stored type"),
                )
            })?;
            Ok(Sample {
                id,
                hour: field.times.get(index).copied(),
                value,
            })
        })
        .collect()
}

fn float(value: Value) -> Option<f32> {
    match value {
        Value::Float(v) if (v as f32).is_finite() => Some(v as f32),
        _ => None,
    }
}
fn int(value: Value) -> Option<i32> {
    match value {
        Value::Int(v) => i32::try_from(v).ok(),
        _ => None,
    }
}
fn uint(value: Value) -> Option<u32> {
    match value {
        Value::Uint(v) => u32::try_from(v).ok(),
        _ => None,
    }
}
fn boolean(value: Value) -> Option<bool> {
    match value {
        Value::Bool(v) => Some(v),
        _ => None,
    }
}
fn colour(value: Value) -> Option<[f32; 4]> {
    match value {
        Value::Vec4(v) if v.iter().all(|x| x.is_finite()) => Some(v),
        _ => None,
    }
}

impl Records {
    /// Read known fields without inventing any missing samples. A present field of an unexpected type is refused by name.
    pub fn from_gparam(data: &Gparam) -> Result<Self> {
        let tone = |key, f| track(data, "Tone Map[Yebis]", key, f);
        let grading = |key| track(data, "ColorGrading[Yebis]", key, int);
        let fog = |key, f| track(data, "VolumetricFog ParamEditor", key, f);
        let extra = |key| {
            data.groups
                .iter()
                .position(|group| group.key == key)
                .and_then(|index| data.extra(index as u32))
                .cloned()
        };
        Ok(Self {
            version: data.version,
            time: data.time,
            tone_extra: extra("Tone Map[Yebis]"),
            grading_extra: extra("ColorGrading[Yebis]"),
            fog_extra: extra("VolumetricFog ParamEditor"),
            tone: Tone {
                mode: track(data, "Tone Map[Yebis]", "GS Presents - ToneMap-ToneMapMode", int)?,
                reinhard_contrast: tone("GS Presents - ToneMap-ReinhardContrast", float)?,
                reinhard_white_balance: tone("GS Presents - ToneMap-ReinhardWhiteBalance", float)?,
                filmic_contrast: tone("GS Presents - ToneMap-Contrast", float)?,
                exposure: tone("GS Presents - ToneMap-Exposure", float)?,
                grading_enabled: grading("Yebis-ColorGrading-Enable")?,
                lut_source_id: grading("Yebis-LutSourceId")?,
            },
            fog: Fog {
                enabled: track(data, "VolumetricFog ParamEditor", "VolumetricFog Enable", boolean)?,
                game_scale: fog("VolumetricFog GameScale", float)?,
                near_depth: fog("VolumetricFog NearDepth", float)?,
                far_depth: fog("VolumetricFog FarDepth", float)?,
                near_offset: fog("VolumetricFog NearOffset", float)?,
                fixed_density: fog("VolumetricFog Global FixedDensity", float)?,
                scattering_g: fog("VolumetricFog Global ScatteringG", float)?,
                local_light_scale: fog("VolumetricFog Global LocalLightScale", float)?,
                local_light_dist_scale: fog("VolumetricFog Global LocalLightDistScale", float)?,
                height_distribution: fog("VolumetricFog Global HeightFogDistribution", float)?,
                height_offset: fog("VolumetricFog Global HeightFogOffset", float)?,
                height_density_scale: fog("VolumetricFog Global HeightFogDensity Scale", float)?,
                sky_colour: track(
                    data,
                    "VolumetricFog ParamEditor",
                    "VolumetricFog SkyLight SkyColor",
                    colour,
                )?,
                sky_start_dist: fog("VolumetricFog SkyLight StartDist", float)?,
                directional_light_bank: track(
                    data,
                    "VolumetricFog ParamEditor",
                    "VolumetricFog DirLight LightBankId",
                    uint,
                )?,
                directional_direct_colour: track(
                    data,
                    "VolumetricFog ParamEditor",
                    "VolumetricFog DirLight DirectColor",
                    colour,
                )?,
                directional_opposite_colour: track(
                    data,
                    "VolumetricFog ParamEditor",
                    "VolumetricFog DirLight OppositeColor",
                    colour,
                )?,
                directional_start_dist: fog("VolumetricFog DirLight StartDist", float)?,
            },
        })
    }
}

/// Read one named file, including `default` or `MapBase`, without imposing precedence between them.
pub fn read(archive: &Archive, decoder: &Decoder<'_>, stem: &str) -> Result<Records> {
    let data = archive.read(&format!("/param/drawparam/{stem}.gparam.dcx"), decoder)?;
    Records::from_gparam(&Gparam::read(&data)?)
}
