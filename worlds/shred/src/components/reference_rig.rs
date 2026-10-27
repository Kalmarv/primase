//! Animation reference data in its authored rig space; binding to render bones is separate.
use primase_core::{
    Component, Entity, Geometry, Linear, Transform, World, frame::SourceFrame, world::Linearity,
};
use std::ops::Deref;

/// One reference rig record and the source frame its matrices are asked in. The record's fields read through `Deref`.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceRig {
    pub frame: SourceFrame,
    pub record: rme_formats::reference_rig::ReferenceRig,
}

impl Deref for ReferenceRig {
    type Target = rme_formats::reference_rig::ReferenceRig;
    fn deref(&self) -> &Self::Target {
        &self.record
    }
}

impl ReferenceRig {
    /// Global references in authored rig space, as [`rme_formats::reference_rig::ReferenceRig::source_matrices`]
    /// computes them, as core transforms.
    pub fn source_matrices(&self) -> Result<Vec<Transform>, String> {
        self.record
            .source_matrices()
            .map(|v| v.iter().map(|m| Transform { cols: m.cols }).collect())
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
