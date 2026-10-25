//! Character display-mask playback shared by Sekiro and Elden Ring (TAE format SDT).
//!
//! The event layouts agree in both games' templates. Semantics checked against DSAnimStudio
//! f1bff06c, TaeActionSimulationEnvironment.cs (GPL-3.0, reference only; no code copied):
//! 233 assigns 32 zero-based groups at/after its start: 0 off, 1 on, 255 unchanged. Its end is
//! not a reset. 711/713 override the groups whose boolean byte is 1 while their interval is active.
//! Seek evaluation starts from the selected NpcParam default, never from the previously displayed frame.
use primase_core::{Component, Entity, World};
use serde::Serialize;
use super::Event;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MaskChange {
    pub kind: i32,
    pub start: f32,
    pub end: f32,
    pub show: u32,
    pub hide: u32,
    /// 233 is a latched assignment; 711/713 have interval scope.
    pub latched: bool,
}

/// Interpreted visibility facts alongside the raw `fromsoft.clip_events`, on the same clip entity.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClipVisibility {
    pub character: String,
    /// Source record order is priority order, including records with equal start times.
    pub changes: Vec<MaskChange>,
    /// Unknown parameter values are named, not silently assigned a guessed meaning.
    pub refusals: Vec<String>,
}

impl ClipVisibility {
    pub fn decode(character: &str, events: &[Event]) -> Self {
        let mut out=Self {character:character.into(),..Self::default()};
        for event in events.iter().filter(|e| matches!(e.kind,233|711|713)) {
            let Some(bytes)=event.parameters.get(..32) else {
                out.refusals.push(format!("TAE {} at {}: {} bytes, needs 32 mask bytes",event.kind,event.start,event.parameters.len()));
                continue;
            };
            let mut change=MaskChange {kind:event.kind,start:event.start,end:event.end,show:0,hide:0,latched:event.kind==233};
            let mut valid=true;
            for (group,&value) in bytes.iter().enumerate() {
                match (event.kind,value) {
                    (233,0)|(711,1)=>change.hide|=1<<group,
                    (233|713,1)=>change.show|=1<<group,
                    (233,255)|(711|713,0)=>{},
                    _=>{valid=false;out.refusals.push(format!("TAE {} at {} group {group}: unsupported value {value}",event.kind,event.start));}
                }
            }
            if valid {out.changes.push(change);}
        }
        out
    }

    /// Stateless seeking: changing clips, looping and scrubbing backward all restore the supplied static default.
    /// If any record is uninterpreted, keep that default rather than presenting a partially guessed timeline.
    pub fn at(&self, default:u32, time:f64)->u32 {
        if !self.refusals.is_empty() || !time.is_finite() {return default;}
        let mut mask=default;
        for change in &self.changes {
            if time>=f64::from(change.start) && (change.latched || change.end>=super::OPEN_END || time<=f64::from(change.end)) {
                mask=(mask & !change.hide)|change.show;
            }
        }
        mask
    }
}

impl Component for ClipVisibility {
    const NAME:&'static str="fromsoft.clip_visibility";
    const GEOMETRY:primase_core::Geometry=primase_core::Geometry::None;
    fn to_json(&self)->Result<Option<serde_json::Value>,primase_core::json::JsonError> {primase_core::json::to_value(self).map(Some)}
    fn apply_linear(&self,_:&primase_core::Linear)->primase_core::world::Linearity<Self> {primase_core::world::Linearity::Invariant}
    fn verify(&self,_:Entity,_:&World)->Result<(),String> {
        if self.character.is_empty() {return Err("clip visibility has no character identity".into());}
        for change in &self.changes {
            if !change.start.is_finite() || change.end.is_nan() || change.start>change.end || change.show & change.hide!=0 {
                return Err("invalid clip visibility change".into());
            }
        }
        Ok(())
    }
}

/// Playback candidates retained before resolve-state can discard an NPC-hidden primitive.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PrimitiveDisplayMasks {
    pub character:String,
    pub groups:Vec<Option<u8>>,
    pub default:u32,
}
impl Component for PrimitiveDisplayMasks {
    const NAME:&'static str="fromsoft.primitive_display_masks";
    const GEOMETRY:primase_core::Geometry=primase_core::Geometry::PerPrimitive;
    fn to_json(&self)->Result<Option<serde_json::Value>,primase_core::json::JsonError> {primase_core::json::to_value(self).map(Some)}
    fn apply_linear(&self,_:&primase_core::Linear)->primase_core::world::Linearity<Self> {primase_core::world::Linearity::Invariant}
    fn remap_primitives(&self,from:&[usize])->Option<Result<Self,String>> {
        Some(primase_core::remap_list(&self.groups,from).map(|groups|Self {groups,..self.clone()}))
    }
    fn verify(&self,e:Entity,w:&World)->Result<(),String> {
        let mesh=w.get::<primase_core::Mesh>(e).ok_or("display masks on an entity with no mesh")?;
        if self.character.is_empty() || mesh.primitives().len()!=self.groups.len() || self.groups.iter().flatten().any(|&g|g>=32) {
            return Err("display mask groups do not match the mesh's primitive list".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(kind:i32,start:f32,end:f32,group:usize,value:u8)->Event {
        let mut parameters=vec![if kind==233 {255}else{0};32];parameters[group]=value;
        Event {kind,start,end,unk04:0,parameters}
    }
    #[test]
    fn sekiro_and_elden_assignments_latch_but_seek_and_clip_changes_restore_defaults() {
        let timeline=ClipVisibility::decode("c1010",&[event(233,0.5,0.6,3,1),event(233,1.5,1.6,3,0)]);
        assert_eq!(timeline.at(0,0.),0);
        assert_eq!(timeline.at(0,1.),1<<3,"233 stays assigned beyond its end");
        assert_eq!(timeline.at(0,2.),0);
        assert_eq!(timeline.at(0,1.),1<<3,"backward seek does not inherit the last frame");
        assert_eq!(timeline.at(1<<7,1.),(1<<7)|(1<<3),"255 preserves NPC defaults");
        assert_eq!(ClipVisibility::decode("c2120",&[]).at(1<<7,1.),1<<7);
    }
    #[test]
    fn boolean_hide_and_show_are_interval_overrides_in_source_priority_order() {
        let timeline=ClipVisibility::decode("c2120",&[event(233,0.,0.,3,1),event(711,1.,2.,3,1),event(713,1.5,1.75,3,1)]);
        assert_eq!(timeline.at(0,0.5),1<<3);
        assert_eq!(timeline.at(0,1.25),0);
        assert_eq!(timeline.at(0,1.6),1<<3);
        assert_eq!(timeline.at(0,1.9),0);
        assert_eq!(timeline.at(0,2.1),1<<3);
    }
    #[test]
    fn malformed_and_unknown_mask_values_are_named_and_do_not_guess_a_partial_state() {
        let timeline=ClipVisibility::decode("c1010",&[event(233,0.,1.,2,2)]);
        assert_eq!(timeline.refusals.len(),1);assert_eq!(timeline.at(5,0.5),5);
        let mut short=event(233,0.,1.,2,1);short.parameters.truncate(31);
        assert!(ClipVisibility::decode("c1010",&[short]).refusals[0].contains("needs 32"));
    }

    #[test]
    fn imported_event_records_inherit_visibility_and_cycles_are_refused() {
        use super::super::{Animation,Header,Tae};
        let own=Animation {id:1,header:Header::None,file_name:None,events:vec![event(233,0.,1.,3,1)]};
        let alias=Animation {id:2,header:Header::ImportOther {from:1,unknown:0},file_name:None,events:vec![]};
        let mut tae=Tae {id:1,event_bank:0,skeleton_name:None,sib_name:None,animations:vec![own,alias]};
        let inherited=tae.resolved_animation(2).expect("resolve").expect("record");
        assert_eq!(ClipVisibility::decode("c1010",&inherited.events).at(0,0.5),1<<3);
        tae.animations[0].header=Header::ImportOther {from:2,unknown:0};
        assert!(tae.resolved_animation(2).is_err());
        tae.animations[0].header=Header::ImportOther {from:3,unknown:0};
        assert!(tae.resolved_animation(2).is_err());
    }
}
