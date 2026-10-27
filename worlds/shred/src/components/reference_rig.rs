//! Animation reference data in its authored rig space; binding to render bones is separate.
use morpheme_formats::Rig;
use primase_core::{
    Component, Entity, Geometry, Linear, Transform, World, frame::SourceFrame, world::Linearity,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceRig {
    pub pak: String,
    pub entry: Vec<u8>,
    pub record_offset: usize,
    pub asset_id: u32,
    pub guid: [u8; 16],
    pub header_word: u32,
    pub frame: SourceFrame,
    pub rig: Rig,
    /// Original type 1 payload, retaining auxiliary tables and header fields not yet assigned meanings.
    /// Padding and serialized runtime-pointer words are not required to be zero.
    pub source_payload: Vec<u8>,
}

impl ReferenceRig {
    /// Global references in authored rig space. The separate rig-header orientation/vector is not applied.
    /// Matches the native reference constructor: XYZW to an affine matrix, then parent global * local.
    /// Local entries use f32 arithmetic; parent composition uses core's f64 intermediates rounded to f32.
    /// This preserves the construction's meaning, without promising bit equality to native FMA rounding.
    pub fn source_matrices(&self) -> Result<Vec<Transform>, String> {
        let mut matrices: Vec<Option<Transform>> = vec![None; self.rig.joints.len()];
        for start in 0..self.rig.joints.len() {
            let mut chain = Vec::new();
            let mut next = Some(start);
            while let Some(index) = next {
                if matrices
                    .get(index)
                    .ok_or("reference parent outside rig")?
                    .is_some()
                {
                    break;
                }
                if chain.contains(&index) {
                    return Err("cyclic reference hierarchy".into());
                }
                chain.push(index);
                next = self
                    .rig
                    .joints
                    .get(index)
                    .ok_or("reference joint missing")?
                    .parent
                    .map(|p| p as usize);
            }
            for index in chain.into_iter().rev() {
                let joint = &self.rig.joints[index];
                let [x, y, z, w] = joint.rotation;
                // Do not normalize the authored quaternion: the native constructor uses these values directly.
                let local = Transform {
                    cols: [
                        [
                            1. - 2. * (y * y + z * z),
                            2. * (x * y + w * z),
                            2. * (x * z - w * y),
                        ],
                        [
                            2. * (x * y - w * z),
                            1. - 2. * (x * x + z * z),
                            2. * (y * z + w * x),
                        ],
                        [
                            2. * (x * z + w * y),
                            2. * (y * z - w * x),
                            1. - 2. * (x * x + y * y),
                        ],
                        joint.translation,
                    ],
                };
                let global = if let Some(parent) = joint.parent {
                    matrices
                        .get(parent as usize)
                        .and_then(|m| *m)
                        .ok_or("reference parent not resolved")?
                        .compose(&local)
                } else {
                    local
                };
                if global.cols.iter().flatten().any(|v| !v.is_finite()) {
                    return Err("nonfinite global reference".into());
                }
                matrices[index] = Some(global);
            }
        }
        matrices
            .into_iter()
            .map(|m| m.ok_or_else(|| "unresolved reference joint".into()))
            .collect()
    }

    /// The same reference maps with both sides converted through the recorded source frame.
    pub fn matrices(&self) -> Result<Vec<Transform>, String> {
        self.source_matrices()
            .map(|v| v.iter().map(|m| self.frame.transform(m)).collect())
    }
}

impl Component for ReferenceRig {
    const NAME: &'static str = "shred.reference_rig";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn payload_bytes(&self) -> u64 {
        self.source_payload.len() as u64
            + self.entry.len() as u64
            + self.pak.len() as u64
            + self
                .rig
                .joints
                .iter()
                .map(|j| 40 + j.name.len() as u64)
                .sum::<u64>()
    }
    fn verify(&self, _: Entity, _: &World) -> Result<(), String> {
        let decoded = morpheme_formats::rig_be32(&self.source_payload, 0).map_err(|e| e.to_string())?;
        if decoded != self.rig {
            return Err("reference rig differs from source provenance".into());
        }
        self.matrices().map(|_| ())
    }
}
