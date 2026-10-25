//! What a character's files hold beyond the model and its clips, kept as stored for a display or a pass to interpret: the
//! model's dummy polygons, the event table of each animation and the NPC param's descriptive fields.
use primase_core::json::{JsonError, to_value};
use primase_core::{Component, Resource};
use serde::Serialize;
use serde_json::Value;

use crate::flver::Dummy;
use crate::param::NpcStats;
use crate::tae::{Event, Header};

/// A model's dummy polygons (`fromsoft.dummies`, on the prototype), in file order, as the FLVER stores them: the file's
/// frame, positions in the space of the parent bone where there is one. [`Sockets`](primase_core::Sockets) on the
/// model's skeleton place them for a display.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Dummies(pub Vec<Dummy>);
impl Component for Dummies {
    const NAME: &'static str = "fromsoft.dummies";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
}

/// One cloth companion file of a model ([`crate::clm2::Clm2`]), by its member name (`wp_a_0427_c.clm2`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClothFile {
    pub file: String,
    /// For each mesh of the model, its entries as `[unk00, unk02]` (what the words mean is not established).
    pub meshes: Vec<Vec<[i16; 2]>>,
}

/// The cloth companion files that go with a model (`fromsoft.cloth`, on the prototype), as stored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Cloth(pub Vec<ClothFile>);
impl Component for Cloth {
    const NAME: &'static str = "fromsoft.cloth";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
}

/// The events one animation's `.tae` record holds (`fromsoft.clip_events`, on the entity that holds the animation and,
/// once lowered, its `core.clip`): where the record was read from, the animation's number in it, its mini header and
/// the events with their times in seconds and their parameter bytes as stored.
/// Version 2 retains the authored record separately and adds reader-typed descriptors.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClipEvents {
    /// The `.tae` member the record was read from (`c1010.tae`).
    pub file: String,
    /// The animation's number as the file stores it (`3000`).
    pub animation: i64,
    pub header: Header,
    /// The motion file name the record's mini header names, where it names one.
    pub file_name: Option<String>,
    /// This clip's own event record, before following its header's import reference.
    pub authored_events: Vec<Event>,
    /// Events after resolving the authored import chain; source order is retained.
    pub events: Vec<Event>,
    /// Reader-decoded fields, one per resolved event in the same order. An empty vector
    /// on older manually constructed data means not read, never permission to parse in a pass.
    pub typed: Vec<DecodedEvent>,
}

/// Template interpretation attached by the reader, separate from the stored parameter bytes.
/// Duplicate field labels remain in source order. A refusal never discards the raw record.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DecodedEvent {
    pub name: Option<String>,
    pub naming: Option<primase_core::NameOrigin>,
    pub values: Vec<primase_core::EventValue>,
    pub refusals: Vec<String>,
}
impl Component for ClipEvents {
    const NAME: &'static str = "fromsoft.clip_events";
    const VERSION:u32=2;
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
    fn verify(&self, _entity:primase_core::Entity, _world:&primase_core::World)->Result<(),String> {
        if !self.typed.is_empty() && self.typed.len()!=self.events.len() {
            return Err(format!("{}: {} raw events but {} typed records",self.file,self.events.len(),self.typed.len()));
        }
        for event in &self.typed {
            if event.name.is_some()!=event.naming.is_some() {
                return Err("event name and naming origin must travel together".into());
            }
        }
        Ok(())
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::world::Linearity<Self> {
        primase_core::world::Linearity::Invariant
    }
}

/// The descriptive fields of the NPC param's rows (`fromsoft.npc_stats`): every row's [`NpcStats`], read with the rows'
/// display masks ([`NpcRows`](super::source::NpcRows)).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NpcStatRows(pub Vec<NpcStats>);
impl Resource for NpcStatRows {
    const NAME: &'static str = "fromsoft.npc_stats";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        to_value(self).map(Some)
    }
}

/// Authored NPC-row names joined through NpcParam.nameId to the game's NPC-name FMG,
/// published by npc_names_into_world. The pure naming pass consumes this small table;
/// it never opens message binders or guesses a row from a character model number.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NpcNames {
    pub game: crate::Game,
    pub language: String,
    pub names: std::collections::BTreeMap<i32,String>,
    /// Missing optional binders and duplicate message ids, with both source paths.
    pub report: Vec<String>,
}
impl Resource for NpcNames {
    const NAME: &'static str = "fromsoft.npc_names";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {to_value(self).map(Some)}
    fn verify(&self,_world:&primase_core::World)->Result<(),String> {
        if self.language.trim().is_empty() {return Err("NPC game-name language is empty".into())}
        if self.names.values().any(|name|name.trim().is_empty()) {
            return Err("NPC game-name table has an empty name".into());
        }
        Ok(())
    }
}
