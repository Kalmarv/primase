//! Decoded source display-mask facts. Playback evaluation belongs to a lowering/toolkit,
//! never to these components; raw event bytes remain beside them in fromsoft.clip_events.
use primase_core::{Component, Entity, World};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaskChange {
    pub kind: i32,
    pub start: f32,
    pub end: f32,
    pub show: u32,
    pub hide: u32,
    /// Source event 233 is latched; 711/713 state intervals without a proven end-instant convention.
    pub latched: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClipVisibility {
    pub character: String,
    /// Source record order is priority order, including equal start times.
    pub changes: Vec<MaskChange>,
    pub refusals: Vec<String>,
}
impl Component for ClipVisibility {
    const NAME:&'static str="fromsoft.clip_visibility";
    const GEOMETRY:primase_core::Geometry=primase_core::Geometry::None;
    fn to_json(&self)->Result<Option<serde_json::Value>,primase_core::JsonError> {primase_core::json::to_value(self).map(Some)}
    fn apply_linear(&self,_:&primase_core::Linear)->primase_core::world::Linearity<Self> {primase_core::world::Linearity::Invariant}
    fn verify(&self,_:Entity,_:&World)->Result<(),String> {
        if self.character.trim().is_empty() {return Err("clip visibility has no character identity".into());}
        if self.refusals.iter().any(|reason|reason.trim().is_empty()) {return Err("empty visibility read refusal".into());}
        for change in &self.changes {
            if !change.start.is_finite() || change.start<0. || change.end.is_nan() || change.start>change.end || change.show & change.hide!=0 {
                return Err("invalid clip visibility change".into());
            }
        }
        Ok(())
    }
}

/// Playback candidates preserved before static state selection can discard an NPC-hidden primitive.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PrimitiveDisplayMasks {
    pub character:String,
    pub groups:Vec<Option<u8>>,
    pub default:u32,
}
impl Component for PrimitiveDisplayMasks {
    const NAME:&'static str="fromsoft.primitive_display_masks";
    const GEOMETRY:primase_core::Geometry=primase_core::Geometry::PerPrimitive;
    fn to_json(&self)->Result<Option<serde_json::Value>,primase_core::JsonError> {primase_core::json::to_value(self).map(Some)}
    fn apply_linear(&self,_:&primase_core::Linear)->primase_core::world::Linearity<Self> {primase_core::world::Linearity::Invariant}
    fn remap_primitives(&self,from:&[usize])->Option<Result<Self,String>> {
        Some(primase_core::remap_list(&self.groups,from).map(|groups|Self {groups,..self.clone()}))
    }
    fn verify(&self,e:Entity,w:&World)->Result<(),String> {
        let mesh=w.get::<primase_core::Mesh>(e).ok_or("display masks on an entity with no mesh")?;
        if self.character.trim().is_empty() || mesh.primitives().len()!=self.groups.len() || self.groups.iter().flatten().any(|&g|g>=32) {
            return Err("display mask groups do not match the mesh's primitive list".into());
        }
        Ok(())
    }
}
