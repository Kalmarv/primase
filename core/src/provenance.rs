//! Provenance (`core.provenance`): where an entity's content came from, down to the primitive, and which passes
//! derived it, so a glitch in an output traces back to the archive, the file and the part of the file it was read
//! from (DESIGN.md §4: passes that merge entities merge their provenance).
//!
//! - An [`Origin`] is a place in the source data: a file, the archive it is in, and the item inside the file (a
//!   FLVER mesh, a drawable's model and geometry, a texture's name), in the game's own naming, never a local path.
//! - [`Provenance::origins`] lists what the entity was made from: one origin for content read as it is, several for a
//!   merge (a material shared by many files, an image several textures encode to).
//! - [`Provenance::parts`] gives, per primitive of the entity's `core.mesh`, the index of its own origin. A pass
//!   that drops, splits or reorders primitives keeps it in step through `World::remap_primitives`, like every
//!   per-primitive component (`Component::remap_primitives`); the verifier
//!   refuses a list whose length differs from the mesh's primitive count, naming the pass that broke it.
//! - [`Provenance::passes`] lists the passes that derived the entity (split, merged or baked it), as pipeline text,
//!   in the order they ran. Passes that only edit in place are in the run report, not repeated here.
//!
//! With levels of detail (`core.lods`) each form carries its own provenance: `select-lod` moves the chosen form's
//! components onto the prototype, so a form without one would leave the finest form's parts against a coarser mesh
//! (which the verifier then refuses).
//!
//! `core.source` stays the reader's short key; this component is the structured trail.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::Value;

use crate::components::Mesh;
use crate::json::JsonError;
use crate::world::{Component, Entity, World};

/// A place in the source data. `file` is within `archive` (on disk when `archive` is `None`); `item` the part of the
/// file the content is (`None`: the whole file).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin {
    pub archive: Option<Arc<str>>,
    pub file: Arc<str>,
    pub item: Option<Arc<str>>,
}

impl Origin {
    pub fn new(archive: Option<&str>, file: &str, item: Option<&str>) -> Self {
        Origin {
            archive: archive.map(Arc::from),
            file: Arc::from(file),
            item: item.map(Arc::from),
        }
    }

    /// The same file, another item in it (shares the strings).
    pub fn with_item(&self, item: &str) -> Self {
        Origin {
            item: Some(Arc::from(item)),
            ..self.clone()
        }
    }
}

impl Serialize for Origin {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let n = 1 + usize::from(self.archive.is_some()) + usize::from(self.item.is_some());
        let mut m = s.serialize_map(Some(n))?;
        if let Some(a) = &self.archive {
            m.serialize_entry("archive", &**a)?;
        }
        m.serialize_entry("file", &*self.file)?;
        if let Some(i) = &self.item {
            m.serialize_entry("item", &**i)?;
        }
        m.end()
    }
}

/// `core.provenance`: see the module docs.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Provenance {
    /// What the entity was made from, without repeats, in first-seen order; never empty.
    pub origins: Vec<Origin>,
    /// Per primitive of the entity's `core.mesh`, in draw order, the index in `origins` of its origin. Empty when
    /// the entity has no mesh or the source does not say which part a primitive came from.
    pub parts: Vec<u32>,
    /// The passes that derived the entity, as pipeline text, in the order they ran.
    pub passes: Vec<Arc<str>>,
}

impl Serialize for Provenance {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let n = 1 + usize::from(!self.parts.is_empty()) + usize::from(!self.passes.is_empty());
        let mut m = s.serialize_map(Some(n))?;
        m.serialize_entry("origins", &self.origins)?;
        if !self.parts.is_empty() {
            m.serialize_entry("parts", &self.parts)?;
        }
        if !self.passes.is_empty() {
            let passes: Vec<&str> = self.passes.iter().map(|p| &**p).collect();
            m.serialize_entry("passes", &passes)?;
        }
        m.end()
    }
}

impl Component for Provenance {
    const NAME: &'static str = "core.provenance";
    const GEOMETRY: crate::Geometry = crate::Geometry::PerPrimitive;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// Origins present and unrepeated; parts index them and, when the entity has a mesh, number its primitives.
    /// The parts follow the rebuilt primitives; without parts the provenance holds nothing per primitive and stays.
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        if self.parts.is_empty() {
            return Some(Ok(self.clone()));
        }
        Some(crate::remap_list(&self.parts, from).map(|parts| Provenance {
            parts,
            ..self.clone()
        }))
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if self.origins.is_empty() {
            return Err("no origins".into());
        }
        let mut seen = BTreeSet::new();
        for (i, o) in self.origins.iter().enumerate() {
            if !seen.insert(o) {
                return Err(format!("origin {i} repeats an earlier one"));
            }
        }
        if let Some(p) = self.parts.iter().find(|&&p| p as usize >= self.origins.len()) {
            return Err(format!("part origin {p} of {} origins", self.origins.len()));
        }
        if !self.parts.is_empty()
            && let Some(m) = w.get::<Mesh>(e)
            && m.primitives().len() != self.parts.len()
        {
            return Err(format!(
                "{} parts for {} primitives",
                self.parts.len(),
                m.primitives().len()
            ));
        }
        Ok(())
    }
}

impl Provenance {
    /// Content read as it is from one place.
    pub fn new(origin: Origin) -> Self {
        Provenance {
            origins: vec![origin],
            ..Provenance::default()
        }
    }

    /// The index of `origin`, added when new (a linear search: for a mesh's own few origins; [`Provenance::merge`]
    /// takes in many at once).
    pub fn intern(&mut self, origin: Origin) -> u32 {
        match self.origins.iter().position(|o| *o == origin) {
            Some(i) => i as u32,
            None => {
                self.origins.push(origin);
                (self.origins.len() - 1) as u32
            }
        }
    }

    /// Another primitive, from `origin`.
    pub fn push_part(&mut self, origin: Origin) {
        let i = self.intern(origin);
        self.parts.push(i);
    }

    /// Take in `other`'s origins and passes (a merge). Parts are not touched: they follow this entity's mesh.
    pub fn merge(&mut self, other: &Provenance) {
        let mut seen: BTreeSet<Origin> = self.origins.iter().cloned().collect();
        for o in &other.origins {
            if seen.insert(o.clone()) {
                self.origins.push(o.clone());
            }
        }
        for p in &other.passes {
            if !self.passes.contains(p) {
                self.passes.push(p.clone());
            }
        }
    }

    /// Record that `pass` (its pipeline text) derived the entity; not repeated when it is already the last.
    pub fn derived_by(&mut self, pass: &str) {
        if self.passes.last().map(|p| &**p) != Some(pass) {
            self.passes.push(Arc::from(pass));
        }
    }
}

/// The provenance of an entity `pass` made from `from`: their origins and passes merged, `pass` appended. `None`
/// when none of them has any (nothing to trace, nothing invented).
pub fn derive(w: &World, from: &[Entity], pass: &str) -> Option<Provenance> {
    let mut out: Option<Provenance> = None;
    for &e in from {
        if let Some(p) = w.get::<Provenance>(e) {
            match &mut out {
                Some(o) => o.merge(p),
                None => {
                    out = Some(Provenance {
                        parts: Vec::new(),
                        ..p.clone()
                    })
                }
            }
        }
    }
    let mut out = out?;
    out.derived_by(pass);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Primitive, Semantic, Stream, Topology};
    use std::collections::BTreeMap;

    fn prim() -> Primitive {
        Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(vec![[0.; 3]; 3]))]),
            indices: None,
            material: None,
        }
    }

    fn flver(mesh: u32) -> Origin {
        Origin::new(
            Some("map/m11/m11_000123.mapbnd"),
            "m11_000123.flver",
            Some(&format!("mesh {mesh}")),
        )
    }

    #[test]
    fn parts_follow_the_mesh_and_the_verifier_holds_them_to_it() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Mesh::new(vec![prim(), prim(), prim()]));
        let mut p = Provenance::default();
        for m in [0, 1, 1] {
            p.push_part(flver(m));
        }
        assert_eq!((p.origins.len(), &p.parts[..]), (2, &[0, 1, 1][..]));
        w.insert(e, p.clone());
        assert_eq!(w.verify_components(), []);

        // a pass drops primitive 0 and splits primitive 2 in two, forgetting provenance: the verifier names it
        *w.get_mut::<Mesh>(e).unwrap() = Mesh::new(vec![prim(), prim(), prim(), prim()]);
        let failures = w.verify_components();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].2, "3 parts for 4 primitives");
        // remapping with the edit keeps it in step; an index it lacks is refused and changes nothing
        assert_eq!(
            w.remap_primitives(e, &[4]).unwrap_err(),
            "core.provenance: primitive 4 of 3"
        );
        w.remap_primitives(e, &[1, 2, 2, 2]).unwrap();
        assert_eq!(w.get::<Provenance>(e).unwrap().parts, [1, 1, 1, 1]);
        assert_eq!(w.verify_components(), []);
        let mut p = w.get::<Provenance>(e).unwrap().clone();
        p.derived_by("split{max=2}");
        // provenance without parts does not follow primitives
        assert_eq!(Provenance::new(flver(0)).remap_primitives(&[0, 0]), None);

        // indices out of range, repeated or missing origins are refused
        for bad in [
            Provenance {
                parts: vec![0, 0, 0, 2],
                ..p.clone()
            },
            Provenance {
                origins: vec![flver(0), flver(0)],
                parts: vec![],
                passes: vec![],
            },
            Provenance::default(),
        ] {
            w.insert(e, bad);
            assert_eq!(w.verify_components().len(), 1);
        }
    }

    #[test]
    fn derive_merges_origins_and_appends_the_pass() {
        let mut w = World::new();
        let a = w.spawn();
        let b = w.spawn();
        let c = w.spawn();
        let mut pa = Provenance::new(flver(0));
        pa.derived_by("load-textures");
        w.insert(a, pa);
        w.insert(b, Provenance::new(flver(1)));
        let d = derive(&w, &[a, b, c, a], "bake{size=512}").unwrap();
        assert_eq!(d.origins, [flver(0), flver(1)]);
        assert_eq!(d.parts, Vec::<u32>::new());
        assert_eq!(
            d.passes.iter().map(|p| &**p).collect::<Vec<_>>(),
            ["load-textures", "bake{size=512}"]
        );
        assert_eq!(derive(&w, &[c], "bake"), None);
        assert_eq!(
            d.to_json().unwrap().unwrap().to_string(),
            r#"{"origins":[{"archive":"map/m11/m11_000123.mapbnd","file":"m11_000123.flver","item":"mesh 0"},{"archive":"map/m11/m11_000123.mapbnd","file":"m11_000123.flver","item":"mesh 1"}],"passes":["load-textures","bake{size=512}"]}"#
        );
        let bare = Provenance::new(Origin::new(None, "a.ytd", None));
        assert_eq!(
            bare.to_json().unwrap().unwrap().to_string(),
            r#"{"origins":[{"file":"a.ytd"}]}"#
        );
    }
}
