//! The Havok animation container of a tagfile (`hkaAnimationContainer`, the class Sekiro's skeleton and animation files
//! hold as their one named variant), as typed values: skeletons, animations (their headers and, for a spline
//! compressed one, the compressed data as stored) and the bindings that name which bone each track drives.
//!
//! This is reading: every number is the file's. How a spline compressed animation decodes to bone transforms at a time
//! is not here (its data is kept as stored). Fields are read by the names of the class metadata the file carries
//! (`hkaSkeleton`, `hkaBone`, `hkQsTransform`, `hkaSplineCompressedAnimation`, `hkaDefaultAnimatedReferenceFrame`,
//! `hkaAnimationBinding`); a class or field the file does not have is an error naming it, never a default.

use crate::{Result, Tagfile, Value, invalid};

/// `hkQsTransform`: translation, rotation (a quaternion, `x y z w`) and scale, each four floats as stored (the fourth
/// of a translation or scale is padding).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QsTransform {
    pub translation: [f32; 4],
    pub rotation: [f32; 4],
    pub scale: [f32; 4],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bone {
    pub name: String,
    pub lock_translation: bool,
}

/// `hkaSkeleton`.
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    pub name: String,
    /// One a bone: the parent's index, or -1 for a root.
    pub parents: Vec<i16>,
    pub bones: Vec<Bone>,
    /// One a bone: its local transform at rest.
    pub reference_pose: Vec<QsTransform>,
    pub reference_floats: Vec<f32>,
    pub float_slots: Vec<String>,
}

/// `hkaAnimation::AnimationType`, as the file's `type` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationType {
    Unknown,
    Interleaved,
    Mirrored,
    SplineCompressed,
    Quantized,
    Predictive,
    ReferenceFrame,
    /// A value the SDK's enumeration does not have.
    Other(u32),
}

impl AnimationType {
    fn of(value: u32) -> AnimationType {
        match value {
            0 => AnimationType::Unknown,
            1 => AnimationType::Interleaved,
            2 => AnimationType::Mirrored,
            3 => AnimationType::SplineCompressed,
            4 => AnimationType::Quantized,
            5 => AnimationType::Predictive,
            6 => AnimationType::ReferenceFrame,
            other => AnimationType::Other(other),
        }
    }
}

/// `hkaDefaultAnimatedReferenceFrame`: the motion of the animation's reference frame (its root motion), sampled.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceFrame {
    pub frame_type: u32,
    pub up: [f32; 4],
    pub forward: [f32; 4],
    pub duration: f32,
    /// `x y z` and the rotation about `up` in the fourth, one a sample over `duration`.
    pub samples: Vec<[f32; 4]>,
}

/// What `hkaSplineCompressedAnimation` stores besides its header: the blocks of compressed tracks, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct SplineData {
    pub num_frames: u32,
    pub num_blocks: u32,
    pub max_frames_per_block: u32,
    pub mask_and_quantization_size: u32,
    pub block_duration: f32,
    pub block_inverse_duration: f32,
    pub frame_duration: f32,
    pub block_offsets: Vec<u32>,
    pub float_block_offsets: Vec<u32>,
    pub transform_offsets: Vec<u32>,
    pub float_offsets: Vec<u32>,
    pub data: Vec<u8>,
    pub endian: i32,
}

/// `hkaInterleavedUncompressedAnimation`: the transforms of every track at every sample, track-major within a sample.
#[derive(Clone, Debug, PartialEq)]
pub struct InterleavedData {
    pub transforms: Vec<QsTransform>,
    pub floats: Vec<f32>,
}

/// How an animation stores its tracks.
#[derive(Clone, Debug, PartialEq)]
pub enum Tracks {
    Spline(SplineData),
    Interleaved(InterleavedData),
    /// A class this reader does not read the tracks of (its name): the header is still read.
    Unread(String),
}

/// `hkaAnimation` and the class that derives it.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    /// The class of the object (`hkaSplineCompressedAnimation`).
    pub class: String,
    pub kind: AnimationType,
    pub duration: f32,
    pub transform_tracks: usize,
    pub float_tracks: usize,
    pub extracted_motion: Option<ReferenceFrame>,
    /// The name of each annotation track (one a transform track).
    pub annotation_tracks: Vec<String>,
    pub tracks: Tracks,
}

/// `hkaAnimationBinding`: which bone of a skeleton each track of an animation drives.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub original_skeleton_name: String,
    /// Index into [`Container::animations`] of the animation it binds, when the container holds it.
    pub animation: Option<usize>,
    pub transform_track_to_bone: Vec<i16>,
    pub float_track_to_slot: Vec<i16>,
    pub partition_indices: Vec<i16>,
    pub blend_hint: u32,
}

/// `hkaAnimationContainer`.
#[derive(Clone, Debug, PartialEq)]
pub struct Container {
    pub skeletons: Vec<Skeleton>,
    pub animations: Vec<Animation>,
    pub bindings: Vec<Binding>,
}

fn named<'t, 'a>(tag: &'t Tagfile<'a>) -> Result<Value<'t, 'a>> {
    let root = tag.root()?;
    let variants = root.field("namedVariants")?.array()?;
    for i in 0..variants.len() {
        let variant = variants.get(i)?;
        if variant.field("className")?.string()? == "hkaAnimationContainer" {
            return variant.field("variant")?.dereference();
        }
    }
    Err(invalid(0, "no hkaAnimationContainer in the root container"))
}

fn qs(v: Value<'_, '_>) -> Result<QsTransform> {
    Ok(QsTransform {
        translation: v.field("translation")?.vector::<4>()?,
        rotation: v.field("rotation")?.vector::<4>()?,
        scale: v.field("scale")?.vector::<4>()?,
    })
}

fn list<'t, 'a, T>(v: Value<'t, 'a>, each: impl Fn(Value<'t, 'a>) -> Result<T>) -> Result<Vec<T>> {
    let a = v.array()?;
    (0..a.len()).map(|i| each(a.get(i)?)).collect()
}

fn i16s(v: Value<'_, '_>) -> Result<Vec<i16>> {
    list(v, |e| {
        i16::try_from(e.signed()?).map_err(|_| invalid(e.offset, "track index exceeds i16"))
    })
}

fn u32s(v: Value<'_, '_>) -> Result<Vec<u32>> {
    list(v, |e| {
        u32::try_from(e.unsigned()?).map_err(|_| invalid(e.offset, "offset exceeds u32"))
    })
}

fn count(v: Value<'_, '_>) -> Result<usize> {
    usize::try_from(v.signed()?).map_err(|_| invalid(v.offset, "negative count"))
}

/// An integer field that is not negative, as u32.
fn small(v: Value<'_, '_>) -> Result<u32> {
    u32::try_from(v.signed()?).map_err(|_| invalid(v.offset, "value outside u32"))
}

fn skeleton(v: Value<'_, '_>) -> Result<Skeleton> {
    let parents = i16s(v.field("parentIndices")?)?;
    let bones = list(v.field("bones")?, |b| {
        Ok(Bone {
            name: b.field("name")?.string()?,
            lock_translation: b.field("lockTranslation")?.unsigned()? != 0,
        })
    })?;
    let reference_pose = list(v.field("referencePose")?, qs)?;
    if parents.len() != bones.len() || reference_pose.len() != bones.len() {
        return Err(invalid(
            v.offset,
            format!(
                "{} bones, {} parents, {} reference transforms",
                bones.len(),
                parents.len(),
                reference_pose.len()
            ),
        ));
    }
    Ok(Skeleton {
        name: v.field("name")?.string()?,
        parents,
        bones,
        reference_pose,
        reference_floats: list(v.field("referenceFloats")?, |e| e.float())?,
        float_slots: list(v.field("floatSlots")?, |e| e.string())?,
    })
}

fn reference_frame(v: Value<'_, '_>) -> Result<ReferenceFrame> {
    Ok(ReferenceFrame {
        frame_type: small(v.field("frameType")?)?,
        up: v.field("up")?.vector::<4>()?,
        forward: v.field("forward")?.vector::<4>()?,
        duration: v.field("duration")?.float()?,
        samples: list(v.field("referenceFrameSamples")?, |e| e.vector::<4>())?,
    })
}

fn animation(v: Value<'_, '_>) -> Result<Animation> {
    let class = v.type_name().to_owned();
    let tracks = match class.as_str() {
        "hkaSplineCompressedAnimation" => Tracks::Spline(SplineData {
            num_frames: small(v.field("numFrames")?)?,
            num_blocks: small(v.field("numBlocks")?)?,
            max_frames_per_block: small(v.field("maxFramesPerBlock")?)?,
            mask_and_quantization_size: small(v.field("maskAndQuantizationSize")?)?,
            block_duration: v.field("blockDuration")?.float()?,
            block_inverse_duration: v.field("blockInverseDuration")?.float()?,
            frame_duration: v.field("frameDuration")?.float()?,
            block_offsets: u32s(v.field("blockOffsets")?)?,
            float_block_offsets: u32s(v.field("floatBlockOffsets")?)?,
            transform_offsets: u32s(v.field("transformOffsets")?)?,
            float_offsets: u32s(v.field("floatOffsets")?)?,
            data: v.field("data")?.array()?.raw()?.to_vec(),
            endian: i32::try_from(v.field("endian")?.signed()?).map_err(|_| invalid(v.offset, "endian"))?,
        }),
        "hkaInterleavedUncompressedAnimation" => Tracks::Interleaved(InterleavedData {
            transforms: list(v.field("transforms")?, qs)?,
            floats: list(v.field("floats")?, |e| e.float())?,
        }),
        other => Tracks::Unread(other.to_owned()),
    };
    Ok(Animation {
        kind: AnimationType::of(small(v.field("type")?)?),
        duration: v.field("duration")?.float()?,
        transform_tracks: count(v.field("numberOfTransformTracks")?)?,
        float_tracks: count(v.field("numberOfFloatTracks")?)?,
        extracted_motion: v
            .field("extractedMotion")?
            .pointer()?
            .map(reference_frame)
            .transpose()?,
        annotation_tracks: list(v.field("annotationTracks")?, |t| t.field("trackName")?.string())?,
        class,
        tracks,
    })
}

/// The animation container of `tag` (the one named variant of class `hkaAnimationContainer` its root holds).
pub fn container(tag: &Tagfile) -> Result<Container> {
    let c = named(tag)?;
    let skeletons = list(c.field("skeletons")?, |p| skeleton(p.dereference()?))?;
    let animation_values = list(c.field("animations")?, |p| p.dereference())?;
    let animations = animation_values
        .iter()
        .map(|v| animation(*v))
        .collect::<Result<Vec<_>>>()?;
    let bindings = list(c.field("bindings")?, |p| {
        let b = p.dereference()?;
        let target = b.field("animation")?.pointer()?;
        Ok(Binding {
            original_skeleton_name: b.field("originalSkeletonName")?.string()?,
            animation: target.and_then(|t| {
                animation_values
                    .iter()
                    .position(|a| a.offset == t.offset && a.type_index == t.type_index)
            }),
            transform_track_to_bone: i16s(b.field("transformTrackToBoneIndices")?)?,
            float_track_to_slot: i16s(b.field("floatTrackToFloatSlotIndices")?)?,
            partition_indices: i16s(b.field("partitionIndices")?)?,
            blend_hint: small(b.field("blendHint")?)?,
        })
    })?;
    Ok(Container {
        skeletons,
        animations,
        bindings,
    })
}
