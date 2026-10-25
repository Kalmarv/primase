//! Human-facing source facts: a content note, a record's category, and a credited display name.
//! These are descriptions, not game rules or inferred rendering policies.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::{Component, Entity, Geometry, JsonError, World};

/// Where a human-readable name comes from. The source's own ids remain separate from display names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum NameOrigin {
    Game,
    Community { credit: String },
}
impl NameOrigin {
    /// A community name always says whose name it is; game-authored names need no external credit.
    pub fn verify(&self) -> Result<(),String> {
        if let Self::Community { credit } = self && credit.trim().is_empty() {
            return Err("community name has no credit".into());
        }
        Ok(())
    }
}

/// `core.content_note`: what a reader or pass says a consumer should show about an entity's content.
/// `unsupported` says something is not drawn; its stable `key` and human `reason` come from the
/// lowering. `at` is a canonical world position in metres, useful where the entity has no transform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentNote {
    pub key: String,
    pub reason: String,
    pub unsupported: bool,
    pub at: Option<[f32;3]>,
}
impl Component for ContentNote {
    const NAME: &'static str = "core.content_note";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>,JsonError> { crate::json::to_value(self).map(Some) }
    /// `at` is already world space; changing the entity's local geometry does not move it.
    fn apply_linear(&self, _l:&crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn verify(&self, _e:Entity, _w:&World) -> Result<(),String> {
        if self.key.trim().is_empty() { return Err("content note has no machine key".into()); }
        if self.reason.trim().is_empty() { return Err("content note has no reason".into()); }
        if self.at.is_some_and(|at| !at.iter().all(|v|v.is_finite())) {
            return Err("content note position is not finite".into());
        }
        Ok(())
    }
}

/// `core.record`: a retained source record, with or without a position. Its lower-case category
/// and kind use [`crate::kind_is_valid`]'s vocabulary, as markers do. Events need no fake transform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub category: String,
    pub kind: String,
}
impl Component for Record {
    const NAME: &'static str = "core.record";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>,JsonError> { crate::json::to_value(self).map(Some) }
    fn apply_linear(&self, _l:&crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn verify(&self, _e:Entity, _w:&World) -> Result<(),String> {
        if !crate::kind_is_valid(&self.category) {
            return Err(format!("record category {:?} is not a word of a-z, 0-9 and _",self.category));
        }
        if !crate::kind_is_valid(&self.kind) {
            return Err(format!("record kind {:?} is not a word of a-z, 0-9 and _",self.kind));
        }
        Ok(())
    }
}

/// `core.display_name`: a source id's human name, credited independently of the entity's stable name/id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayName {
    pub name: String,
    pub origin: NameOrigin,
}
impl Component for DisplayName {
    const NAME: &'static str = "core.display_name";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>,JsonError> { crate::json::to_value(self).map(Some) }
    fn apply_linear(&self, _l:&crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn verify(&self, _e:Entity, _w:&World) -> Result<(),String> {
        if self.name.trim().is_empty() { return Err("empty display name".into()); }
        self.origin.verify()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn world() -> (World,Entity) { let mut w = World::new(); let e = w.spawn(); (w,e) }
    #[test]
    fn notes_require_key_reason_and_finite_world_position() {
        let (w,e) = world();
        let note = ContentNote { key:"source.unresolved_light".into(),reason:"Lighting is retained, not lowered".into(),
            unsupported:true,at:Some([1.,2.,3.]) };
        assert!(note.verify(e,&w).is_ok());
        let mut bad = note.clone(); bad.key.clear(); assert!(bad.verify(e,&w).is_err());
        let mut bad = note.clone(); bad.reason = " ".into(); assert!(bad.verify(e,&w).is_err());
        for value in [f32::NAN,f32::INFINITY,f32::NEG_INFINITY] {
            let mut bad = note.clone(); bad.at = Some([value,0.,0.]);
            assert!(bad.verify(e,&w).is_err()); assert!(bad.to_json().is_err());
        }
        assert_eq!(serde_json::from_value::<ContentNote>(note.to_json().unwrap().unwrap()).unwrap(),note);
        assert!(note.remap_primitives(&[0]).is_none());
    }
    #[test]
    fn records_have_valid_words_and_need_no_placement() {
        let (w,e) = world(); let r = Record { category:"event".into(),kind:"environment_light".into() };
        assert!(r.verify(e,&w).is_ok());
        for text in ["","Region","with space","a:b"] {
            let mut bad = r.clone(); bad.kind = text.into(); assert!(bad.verify(e,&w).is_err());
            let mut bad = r.clone(); bad.category = text.into(); assert!(bad.verify(e,&w).is_err());
        }
        assert_eq!(serde_json::from_value::<Record>(r.to_json().unwrap().unwrap()).unwrap(),r);
        assert!(r.remap_primitives(&[0]).is_none());
    }
    #[test]
    fn names_keep_their_origin_and_community_credit() {
        let (w,e) = world();
        for origin in [NameOrigin::Game,NameOrigin::Community { credit:"The source-name project".into() }] {
            let name = DisplayName { name:"A human name".into(),origin };
            assert!(name.verify(e,&w).is_ok());
            assert_eq!(serde_json::from_value::<DisplayName>(name.to_json().unwrap().unwrap()).unwrap(),name);
            assert!(name.remap_primitives(&[0]).is_none());
        }
        let mut bad = DisplayName { name:"A name".into(),origin:NameOrigin::Community { credit:" ".into() } };
        assert!(bad.verify(e,&w).is_err());
        bad.origin = NameOrigin::Game; bad.name.clear(); assert!(bad.verify(e,&w).is_err());
    }
}
