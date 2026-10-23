//! A skate. clip read as the game stores it (`skate.clip`), and `lower-skate-clips`, the pass that makes it a
//! `core.clip` under a stated interpolation.
//!
//! A clip is a `ClipControllerAsset` (its `FPS`, `IsLooping`, and its `Anim`, an `EclipseAnimationAsset` in the same
//! document: its `EndFrame`, its `ChannelToDofAsset` (an import from the clip's bundle) and its ECLP `Resource` in the
//! clip's bundle, decoded by [`super::eclipse`]). The ECLP tracks are channels in the order rotations, vectors,
//! scalars, and `ChannelToDofIds` names each channel's DOF in that order. Each track is kept as stored: its DOF, its
//! key frames, its values. What a DOF drives is the rig's ([`super::rig::SkateAnimRig`]).
//!
//! How the game evaluates between keys is not established (the M4 in-game check), so reading takes no choice: the
//! lowering pass states one (`interpolation=linear`, rotations by slerp), reported as an approximation.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    Channel, Clip, ClipTarget, Component, Entity, Extrapolation, Interpolation, JsonError, Name, Property,
    Skeleton, World,
};
use primase_frostbite::ebx::{Object, Pointer, Value};

use serde::Serialize;
use serde_json::Value as Json;

use super::eclipse;
use super::rig::{DofTarget, Imports, SkateAnimRig};

/// The resource type of an ECLP animation.
const ECLP_RESOURCE: u32 = 0x4088_BF7E;
/// `EclipseAnimationAsset.CodecType` of an ECLP animation (`ECLP`).
const ECLP_CODEC: u64 = 0x4543_4C50;

/// One track of a clip as stored: its DOF, key frames and values (`width` a key: 4 a rotation, 3 a vector, 1 a
/// scalar).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateTrack {
    pub dof: u32,
    pub width: u8,
    pub frames: Vec<u32>,
    pub values: Vec<f32>,
}

/// `skate.clip` (see the module documentation): a clip of the animation skeleton held by `skeleton` (its
/// `skate.anim_rig` names the DOFs).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateClip {
    pub name: String,
    pub skeleton: Entity,
    pub fps: f32,
    pub end_frame: u32,
    pub looping: bool,
    pub tracks: Vec<SkateTrack>,
}

impl Component for SkateClip {
    const NAME: &'static str = "skate.clip";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// The name, timing and track count.
    fn to_json(&self) -> Result<Option<Json>, JsonError> {
        Ok(Some(serde_json::json!({
            "name": self.name,
            "fps": self.fps,
            "end_frame": self.end_frame,
            "looping": self.looping,
            "tracks": self.tracks.len(),
        })))
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.skeleton);
    }
    /// Its skeleton's rig names every track's DOF, a joint DOF's track of its width (4 a rotation, 3 a translation or
    /// scale); frames strictly increasing within `0..=end_frame`, one key's values a frame; a finite, positive FPS.
    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        let rig = w
            .get::<SkateAnimRig>(self.skeleton)
            .ok_or("skate.clip's skeleton has no skate.anim_rig")?;
        if !(self.fps.is_finite() && self.fps > 0.0) {
            return Err(format!("fps {}", self.fps));
        }
        for t in &self.tracks {
            let dof = rig
                .dof(t.dof)
                .ok_or_else(|| format!("DOF {} is not the rig's", t.dof))?;
            let want = match dof.target {
                DofTarget::Rotation(_) => Some(4),
                DofTarget::Translation(_) | DofTarget::Scale(_) => Some(3),
                DofTarget::Other(_) => None,
            };
            if want.is_some_and(|w| w != t.width) {
                return Err(format!(
                    "DOF {} takes {want:?} values a key, its track {}",
                    dof.name, t.width
                ));
            }
            if t.values.len() != t.frames.len() * usize::from(t.width) {
                return Err(format!(
                    "DOF {}: {} values for {} keys",
                    dof.name,
                    t.values.len(),
                    t.frames.len()
                ));
            }
            if t.frames.windows(2).any(|p| p[0] >= p[1])
                || t.frames.last().is_some_and(|&f| f > self.end_frame)
            {
                return Err(format!(
                    "DOF {}: frames not increasing within 0..={}",
                    dof.name, self.end_frame
                ));
            }
        }
        Ok(())
    }
}

fn object(doc: &primase_frostbite::ebx::Document, i: usize) -> Result<&Object, String> {
    doc.instances
        .get(i)
        .and_then(|x| x.object.as_ref())
        .ok_or_else(|| format!("instance {i} is not read"))
}

fn number(o: &Object, field: &str) -> Result<f64, String> {
    match o.field(field) {
        Some(Value::Float(x)) => Ok(*x),
        Some(Value::UInt(u)) => Ok(*u as f64),
        Some(Value::Int(i)) => Ok(*i as f64),
        other => Err(format!("{field} is {other:?}")),
    }
}

/// The clip the `ClipControllerAsset` document `doc` (named `name`) states, a clip of the animation skeleton `skeleton`
/// (an entity [`super::rig::insert_rig`] made): its resources fetched by `resource` (by id: type and bytes) and the
/// documents it imports by `imported` (the reader's I/O, [`crate::read::game`]).
pub fn clip_from(
    name: &str,
    doc: &primase_frostbite::ebx::Document,
    resource: &dyn Fn(u64) -> Result<(u32, Vec<u8>), String>,
    imported: &Imports<'_>,
    skeleton: Entity,
) -> Result<SkateClip, String> {
    if doc.root_type != "ClipControllerAsset" {
        return Err(format!(
            "{name} is a {}, not a ClipControllerAsset",
            doc.root_type
        ));
    }
    let controller = object(doc, 0)?;
    let Some(Value::Pointer(Pointer::Internal(k))) = controller.field("Anim") else {
        return Err(format!("{name}: its Anim is not in its document"));
    };
    if doc.type_name(*k) != Some("EclipseAnimationAsset") {
        return Err(format!("{name}: its Anim is a {:?}", doc.type_name(*k)));
    }
    let anim = object(doc, *k)?;
    if number(anim, "CodecType")? as u64 != ECLP_CODEC {
        return Err(format!(
            "{name}: codec {} is not ECLP",
            number(anim, "CodecType")?
        ));
    }
    let Some(Value::Resource(rid)) = anim.field("Resource") else {
        return Err(format!("{name}: its animation has no resource"));
    };
    let (rtype, bytes) = resource(*rid)?;
    if rtype != ECLP_RESOURCE {
        return Err(format!("{name}: resource type {rtype:#x}, not ECLP's"));
    }
    let tracks = eclipse::decode(&bytes)?;
    // the channels' DOFs
    let c2d = imported(
        doc,
        anim.field("ChannelToDofAsset")
            .ok_or_else(|| format!("{name}: no ChannelToDofAsset"))?,
        "ChannelToDofAsset",
    )?;
    let ids: Vec<u32> = match object(&c2d, 0)?.field("ChannelToDofIds") {
        Some(Value::Array(a, _)) => a
            .iter()
            .map(|v| match v {
                Value::UInt(u) => u32::try_from(*u).map_err(|_| format!("DOF id {u} is not 32-bit")),
                other => Err(format!("a ChannelToDofIds entry is {other:?}")),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(format!("{name}: ChannelToDofAsset has no ChannelToDofIds")),
    };
    let n = tracks.rotations.len() + tracks.translations.len() + tracks.scalars.len();
    if ids.len() != n {
        return Err(format!("{name}: {} DOF ids for {n} channels", ids.len()));
    }
    let mut ids = ids.into_iter();
    let mut out = Vec::with_capacity(n);
    for t in &tracks.rotations {
        out.push(SkateTrack {
            dof: ids.next().ok_or("ids")?,
            width: 4,
            frames: t.frames.clone(),
            values: t.values.iter().flatten().copied().collect(),
        });
    }
    for t in &tracks.translations {
        out.push(SkateTrack {
            dof: ids.next().ok_or("ids")?,
            width: 3,
            frames: t.frames.clone(),
            values: t.values.iter().flatten().copied().collect(),
        });
    }
    for t in &tracks.scalars {
        out.push(SkateTrack {
            dof: ids.next().ok_or("ids")?,
            width: 1,
            frames: t.frames.clone(),
            values: t.values.clone(),
        });
    }
    let end_frame = number(anim, "EndFrame")?;
    Ok(SkateClip {
        name: name.to_owned(),
        skeleton,
        fps: number(controller, "FPS")? as f32,
        end_frame: end_frame as u32,
        looping: matches!(controller.field("IsLooping"), Some(Value::Bool(true))),
        tracks: out,
    })
}

/// `clip` into `world` as one entity, with `core.name` its name.
pub fn insert_clip(clip: SkateClip, world: &mut World) -> Entity {
    let e = world.spawn();
    world.insert(e, Name(clip.name.clone()));
    world.insert(e, clip);
    e
}

/// `lower-skate-clips{interpolation=linear}` (required: the game's own rule is not established): every `skate.clip`
/// becomes a `core.clip` on its entity: each track of a joint DOF a channel of that joint (rotation, translation or
/// scale), each scalar track of another DOF a control channel named by the DOF, keys at `frame / fps` seconds,
/// interpolated linearly (rotations by slerp: an approximation, reported); its duration `end_frame / fps`, looping or
/// holding as the clip states. A track of a non-joint DOF with vector values has no core target: left out, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerSkateClips;

/// The pass factory.
pub fn lower_skate_clips(o: &Options) -> Result<Box<dyn Pass>, String> {
    match o.get("interpolation").map(String::as_str) {
        Some("linear") => {}
        Some(v) => return Err(format!("interpolation={v}: only linear is offered")),
        None => {
            return Err("interpolation=linear is required (the game's own rule is not established)".into());
        }
    }
    if let Some(k) = o.keys().find(|k| *k != "interpolation") {
        return Err(format!("unknown option {k}"));
    }
    Ok(Box::new(LowerSkateClips))
}

impl Pass for LowerSkateClips {
    fn name(&self) -> &'static str {
        "lower-skate-clips"
    }
    fn options(&self) -> Options {
        Options::from([("interpolation".to_owned(), "linear".to_owned())])
    }
    fn requires(&self) -> &[&'static str] {
        &[SkateClip::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Clip::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[SkateClip::NAME]
    }
    fn approximates(&self) -> &'static str {
        "ECLP keys interpolated linearly (rotations by slerp): the game's own interpolation is not established"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let clips: Vec<(Entity, SkateClip)> = w.iter::<SkateClip>().map(|(e, c)| (e, c.clone())).collect();
        for (e, c) in clips {
            let rig = w
                .get::<SkateAnimRig>(c.skeleton)
                .ok_or("a skate.clip's skeleton has no skate.anim_rig")?;
            if w.get::<Skeleton>(c.skeleton).is_none() {
                return Err("a skate.clip's skeleton has no core.skeleton".into());
            }
            let fps = f64::from(c.fps);
            let mut channels = Vec::with_capacity(c.tracks.len());
            let mut left_out = 0u64;
            let mut keys = 0u64;
            for t in &c.tracks {
                let dof = rig
                    .dof(t.dof)
                    .ok_or_else(|| format!("DOF {} is not the rig's", t.dof))?;
                let (target, property) = match &dof.target {
                    DofTarget::Rotation(j) => (Some(*j), Property::Rotation),
                    DofTarget::Translation(j) => (Some(*j), Property::Translation),
                    DofTarget::Scale(j) => (Some(*j), Property::Scale),
                    DofTarget::Other(_) if t.width == 1 => (None, Property::Scalar),
                    DofTarget::Other(_) => {
                        left_out += 1;
                        continue;
                    }
                };
                keys += t.frames.len() as u64;
                channels.push(Channel {
                    target: match target {
                        Some(joint) => ClipTarget::Joint {
                            skeleton: c.skeleton,
                            joint,
                        },
                        None => ClipTarget::Control {
                            name: dof.name.clone(),
                        },
                    },
                    property,
                    interpolation: Interpolation::Linear,
                    times: t.frames.iter().map(|&f| (f64::from(f) / fps) as f32).collect(),
                    values: t.values.clone(),
                });
            }
            w.remove::<SkateClip>(e);
            w.insert(
                e,
                Clip {
                    name: c.name.clone(),
                    duration: (f64::from(c.end_frame) / fps) as f32,
                    extrapolation: if c.looping {
                        Extrapolation::Loop
                    } else {
                        Extrapolation::Hold
                    },
                    channels,
                    root_motion: None,
                    additive: None,
                },
            );
            log.count("skate clips lowered", 1);
            log.approximate(
                "ECLP keys interpolated linearly (the game's rule not established)",
                keys,
            );
            if left_out > 0 {
                log.omit(
                    "tracks of non-joint DOFs with vector values (no core target)",
                    left_out,
                );
            }
        }
        Ok(())
    }
}
