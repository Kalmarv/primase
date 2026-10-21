//! The world store: entities, typed components, opaque (unknown) components and world resources.
//!
//! Deliberately small and deterministic. Every column keeps its entities in ascending order, so iteration order is a
//! function of the world's contents, never of hashing or insertion timing; writers that walk the world produce the
//! same bytes for the same world.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::json::JsonError;

/// An entity: a plain id. Ids are never reused within one world, so a stale id finds nothing rather than another
/// entity's components.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entity(u32);

impl Entity {
    /// The entity's index in creation order (serialisation uses it).
    pub fn index(self) -> u32 {
        self.0
    }

    /// The entity with this index. For readers of serialised worlds: an index the world never spawned finds nothing.
    pub fn from_index(index: u32) -> Self {
        Entity(index)
    }
}

/// JSON (and any serde form) shows an entity as its index.
impl serde::Serialize for Entity {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(self.0)
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}", self.0)
    }
}

/// A typed component. `NAME` is namespaced and stable (`core.transform`, `skate.material`, `fromsoft.layered_material`):
/// serialisation, pass declarations and reports use it, so renaming one is a format change.
pub trait Component: Any + Clone + fmt::Debug + Send + Sync {
    const NAME: &'static str;
    /// Schema version of the component's content. Bump it when the meaning or layout of a field changes.
    const VERSION: u32 = 1;
    /// What the value holds of its entity's mesh primitives and vertices ([`Geometry`]). Stated by every component,
    /// with no default, so data that describes a mesh can't be left behind by an edit of it unawares: the World's edit
    /// paths take it from there.
    const GEOMETRY: Geometry;

    /// The component as JSON, for glTF extras, the debug text form and golden tests; `Ok(None)` for components
    /// that are not shown (bulk data, handles). The view must never lose a value: components that can be shown
    /// derive `serde::Serialize` and return `crate::json::to_value(self).map(Some)`, which refuses non-finite
    /// floats instead of writing them as `null`.
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        Ok(None)
    }

    /// The component's own invariants on entity `e` (indices in range, stream lengths equal, ...). The pass manager
    /// runs it on every component after the reader, after every pass and before the writer, together with the
    /// world's own check that every entity of [`Component::references`] is alive. World-wide rules (one name -> one
    /// payload) are `pass::Validator`s instead.
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        Ok(())
    }

    /// The entities this component refers to (a prototype, materials, textures), appended to `out`: the edges
    /// [`World::closure`] follows. A component holding entity references must declare every one of them.
    fn references(&self, _out: &mut Vec<Entity>) {}

    /// The bytes of bulk payload the value holds (vertex streams and indices, texture levels, triangle soups), for
    /// memory reports: element counts times element sizes, not allocated capacity. 0 for small components.
    fn payload_bytes(&self) -> u64 {
        0
    }

    /// The component after its entity's mesh primitives were rebuilt, for a component that holds something per
    /// primitive ([`Geometry::PerPrimitive`], [`Geometry::PerVertex`]): new primitive `i` comes from old primitive
    /// `from[i]` (a source may repeat, when a primitive is split, or be absent, when one is dropped). Such a component
    /// always answers (a value holding nothing per primitive answers with itself); `None`, the default, is for
    /// components of [`Geometry::None`], and [`World::remap_primitives`] refuses a dependent component that gives it.
    /// An index the component does not have is an error. [`remap_list`] does the work for a list with one entry per
    /// primitive.
    fn remap_primitives(&self, _from: &[usize]) -> Option<Result<Self, String>> {
        None
    }

    /// The component after the vertices of its entity's mesh primitive `primitive` were compacted or reordered, for a
    /// component that holds something per vertex ([`Geometry::PerVertex`]): new vertex `i` of that primitive is old
    /// vertex `from[i]` (a source may repeat or be absent). Such a component always answers (with itself when it holds
    /// nothing for that primitive); `None`, the default, is for the others, and [`World::remap_vertices`] refuses a
    /// per-vertex component that gives it. An index the component does not have is an error. Every pass that compacts
    /// or reorders a primitive's vertices calls [`World::remap_vertices`]. After a rebuild of the primitive list
    /// ([`World::rebuild_primitives`], [`Component::remap_primitives`]) a primitive's per-vertex data is its source
    /// primitive's, and the vertex map is relative to that source.
    fn remap_vertices(&self, _primitive: usize, _from: &[u32]) -> Option<Result<Self, String>> {
        None
    }

    /// The component after the vertices of its entity's mesh primitive `primitive` were rebuilt as blends of old ones
    /// ([`VertexBlend`]), for a component that holds something per vertex ([`Geometry::PerVertex`]): new vertex `i`
    /// is the combination `from[i]` of old vertices, in whatever way the component's data combines (a morph target's
    /// delta is interpolated, the way the surface it moves is). It answers as [`Component::remap_vertices`] does: with
    /// itself when it holds nothing for that primitive; `None`, the default, is for a component that does not know how
    /// to combine its data, and [`World::blend_vertices`] refuses a per-vertex component that gives it. An index the
    /// component does not have is an error. A vertex that is a copy of one old vertex ([`VertexBlend::copy`]) is the
    /// case [`Component::remap_vertices`] states, and a component answering one answers the other alike.
    fn blend_vertices(&self, _primitive: usize, _from: &[VertexBlend]) -> Option<Result<Self, String>> {
        None
    }

    /// The component after its entity's local space was mapped by `l` (a pass that bakes a placement's mirror or
    /// shear into a copy of its geometry, so transforms stay rigid). [`World::apply_linear`] calls it for every
    /// component of the entity. The default is [`Linearity::Unsupported`]: a component that holds local-space
    /// geometry and does not say how it follows is never silently left behind; one without geometry says
    /// [`Linearity::Invariant`].
    fn apply_linear(&self, _l: &crate::Linear) -> Linearity<Self> {
        Linearity::Unsupported
    }
}

/// What a component's value holds of its entity's mesh ([`Component::GEOMETRY`]): whether an edit of the primitives
/// or vertices must carry it along. A component holding geometry of its own (`core.mesh`, `core.mesh_file`, a source
/// model a reader keeps beside it) describes no other component's primitives: [`Geometry::None`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    /// Nothing per primitive or vertex of its entity's mesh: kept as it is whatever an edit does to them.
    None,
    /// Something per primitive of its entity's mesh (a material per primitive, provenance per part): it implements
    /// [`Component::remap_primitives`], goes with the mesh ([`World::remove_mesh`]), and an edit that rebuilds the
    /// primitives refuses it when it does not answer.
    PerPrimitive,
    /// Something per vertex of its entity's mesh primitives (and so per primitive): as [`Geometry::PerPrimitive`], and
    /// it implements [`Component::remap_vertices`] too.
    PerVertex,
}

impl Geometry {
    /// Whether the value describes its entity's mesh, so it goes when the mesh does.
    pub fn describes_mesh(self) -> bool {
        self != Geometry::None
    }
}

/// What a component becomes when its entity's local space is mapped by a [`crate::Linear`]
/// ([`Component::apply_linear`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Linearity<T> {
    /// It holds no local-space geometry: kept as it is.
    Invariant,
    /// It holds geometry, mapped.
    Applied(T),
    /// What it says is no longer true of the mapped geometry (a file the geometry was read from): removed.
    Drop,
    /// It cannot follow: the entity is left as it was.
    Unsupported,
}

/// `list` (one entry per primitive) for primitives rebuilt from `from` ([`Component::remap_primitives`]).
pub fn remap_list<T: Clone>(list: &[T], from: &[usize]) -> Result<Vec<T>, String> {
    from.iter()
        .map(|&i| {
            list.get(i)
                .cloned()
                .ok_or_else(|| format!("primitive {i} of {}", list.len()))
        })
        .collect()
}

/// A new vertex of a triangle primitive made as a weighted combination of up to three old vertices of it (the corners
/// of the triangle it lies in): `sum weights[k] * old vertex sources[k]`, the weights non-negative and summing to 1. A
/// copy of one old vertex has weight 1 on it; unused entries have weight 0. See [`Component::blend_vertices`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VertexBlend {
    pub sources: [u32; 3],
    pub weights: [f32; 3],
}

impl VertexBlend {
    /// The old vertex `v` itself.
    pub fn copy(v: u32) -> VertexBlend {
        VertexBlend {
            sources: [v; 3],
            weights: [1.0, 0.0, 0.0],
        }
    }

    /// The old vertices with a weight above zero, with their weights, in order.
    pub fn terms(&self) -> impl Iterator<Item = (u32, f32)> + '_ {
        self.sources
            .iter()
            .copied()
            .zip(self.weights)
            .filter(|&(_, w)| w > 0.0)
    }
}

/// A component's value rebuilt for blended vertices, not stored yet, with the column's type.
type Blended = (TypeId, Box<dyn Any + Send>);

/// A world resource: one value per world (the frame, a game's archive index, a run's options, the environment).
pub trait Resource: Any + Clone + fmt::Debug + Send + Sync {
    const NAME: &'static str;
    const VERSION: u32 = 1;

    /// As [`Component::to_json`].
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        Ok(None)
    }

    /// The resource's own invariants in `w`; as [`Component::verify`], run by the pass manager's verifier.
    fn verify(&self, _w: &World) -> Result<(), String> {
        Ok(())
    }

    /// The entities this resource refers to, appended to `out`. A resource holding entity references must declare
    /// every one of them and implement [`Resource::retain_references`].
    fn references(&self, _out: &mut Vec<Entity>) {}

    /// Forget every reference to an entity `keep` refuses (set it to "none"). The world calls it whenever entities
    /// leave it ([`World::despawn`], [`World::retain`], [`World::move_components`]) and on the copy a
    /// [`World::subset`] or [`World::split_off`] part gets, so a resource never refers to an entity its world lacks.
    fn retain_references(&mut self, _keep: &dyn Fn(Entity) -> bool) {}
}

/// One component (or resource) as JSON, with the schema version it is written with.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentJson<'w> {
    pub name: &'w str,
    pub version: u32,
    /// `None`: no JSON view (opaque bytes, typed components that are not shown).
    pub value: Option<serde_json::Value>,
}

/// A component whose JSON view would lose a value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{component}{}: {message}", entity.map(|e| format!(" on {e:?}")).unwrap_or_default())]
pub struct ComponentJsonError {
    pub component: String,
    /// `None` for a resource.
    pub entity: Option<Entity>,
    pub message: String,
}

/// The content of a component this build does not know: carried through reading and writing untouched.
#[derive(Clone, Debug, PartialEq)]
pub enum OpaqueData {
    Json(serde_json::Value),
    Bytes(Vec<u8>),
}

/// An opaque component value with the schema version it was written with.
#[derive(Clone, Debug, PartialEq)]
pub struct Opaque {
    pub version: u32,
    pub data: OpaqueData,
}

/// The storage of one component type: entity -> value, ordered by entity.
#[derive(Clone, Debug)]
pub struct Column<T> {
    values: BTreeMap<Entity, T>,
}

impl<T> Default for Column<T> {
    fn default() -> Self {
        Column {
            values: BTreeMap::new(),
        }
    }
}

impl<T> Column<T> {
    pub fn get(&self, e: Entity) -> Option<&T> {
        self.values.get(&e)
    }

    pub fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        self.values.get_mut(&e)
    }

    pub fn contains(&self, e: Entity) -> bool {
        self.values.contains_key(&e)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Entities and values in ascending entity order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (Entity, &T)> + ExactSizeIterator {
        self.values.iter().map(|(e, v)| (*e, v))
    }

    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = (Entity, &mut T)> + ExactSizeIterator {
        self.values.iter_mut().map(|(e, v)| (*e, v))
    }

    pub fn entities(&self) -> impl DoubleEndedIterator<Item = Entity> + ExactSizeIterator + '_ {
        self.values.keys().copied()
    }

    fn insert(&mut self, e: Entity, value: T) -> Option<T> {
        self.values.insert(e, value)
    }

    fn remove(&mut self, e: Entity) -> Option<T> {
        self.values.remove(&e)
    }
}

/// Type-erased operations the world needs on every column.
trait AnyColumn: Any + Send + Sync {
    fn name(&self) -> &'static str;
    fn len(&self) -> usize;
    fn contains(&self, e: Entity) -> bool;
    fn remove_entity(&mut self, e: Entity);
    fn version(&self) -> u32;
    fn geometry(&self) -> Geometry;
    /// `None` when the entity has no such component; else its JSON view.
    fn json(&self, e: Entity) -> Option<Result<Option<serde_json::Value>, JsonError>>;
    /// Every value's `Component::verify`, and that each entity its `Component::references` names is alive, as
    /// (entity, message) for those that fail, in entity order.
    fn verify(&self, w: &World) -> Vec<(Entity, String)>;
    /// Every value through `Debug`, in entity order.
    fn visit_debug(&self, f: &mut dyn FnMut(Entity, &dyn fmt::Debug));
    fn entities(&self) -> Vec<Entity>;
    /// [`Component::references`] of entity `e`'s value, if it has one.
    fn references(&self, e: Entity, out: &mut Vec<Entity>);
    /// The sum of every value's [`Component::payload_bytes`].
    fn payload_bytes(&self) -> u64;
    fn clone_box(&self) -> Box<dyn AnyColumn>;
    /// A copy holding only the values of `keep`.
    fn subset_box(&self, keep: &BTreeSet<Entity>) -> Box<dyn AnyColumn>;
    /// Drop the values of entities outside `keep`.
    fn retain(&mut self, keep: &BTreeSet<Entity>);
    /// Move the values of `keep` out into a column of their own.
    fn split_off_box(&mut self, keep: &BTreeSet<Entity>) -> Box<dyn AnyColumn>;
    /// Move every value of `other`, a column of the same type, in.
    fn absorb(&mut self, other: Box<dyn AnyColumn>);
    /// Move `from`'s value, if it has one, onto `to` (replacing `to`'s).
    fn move_entity(&mut self, from: Entity, to: Entity);
    /// `e`'s value remapped to rebuilt primitives ([`Component::remap_primitives`]), not stored yet; `None` when `e`
    /// has none or it does not follow primitives.
    fn remapped(&self, e: Entity, from: &[usize]) -> Option<Result<Box<dyn Any + Send>, String>>;
    /// `e`'s value with primitive `primitive`'s vertices remapped ([`Component::remap_vertices`]), not stored yet;
    /// `None` when `e` has none or it holds nothing per vertex of that primitive.
    fn vertices_remapped(
        &self,
        e: Entity,
        primitive: usize,
        from: &[u32],
    ) -> Option<Result<Box<dyn Any + Send>, String>>;
    /// `e`'s value with primitive `primitive`'s vertices rebuilt as blends ([`Component::blend_vertices`]), not stored
    /// yet; `None` when `e` has none or it cannot combine its vertices.
    fn vertices_blended(
        &self,
        e: Entity,
        primitive: usize,
        from: &[VertexBlend],
    ) -> Option<Result<Box<dyn Any + Send>, String>>;
    /// Store a value [`AnyColumn::remapped`], [`AnyColumn::vertices_remapped`], [`AnyColumn::vertices_blended`] or
    /// [`AnyColumn::linear`] made.
    fn put_remapped(&mut self, e: Entity, value: Box<dyn Any + Send>);
    /// A copy of `from`'s value, if it has one, onto `to`.
    fn copy_entity(&mut self, from: Entity, to: Entity);
    /// `e`'s value under [`Component::apply_linear`], not stored yet; `None` when `e` has none.
    fn linear(&self, e: Entity, l: &crate::Linear) -> Option<Linearity<Box<dyn Any + Send>>>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Component> AnyColumn for Column<T> {
    fn name(&self) -> &'static str {
        T::NAME
    }
    fn len(&self) -> usize {
        self.values.len()
    }
    fn contains(&self, e: Entity) -> bool {
        self.values.contains_key(&e)
    }
    fn remove_entity(&mut self, e: Entity) {
        self.values.remove(&e);
    }
    fn version(&self) -> u32 {
        T::VERSION
    }
    fn geometry(&self) -> Geometry {
        T::GEOMETRY
    }
    fn json(&self, e: Entity) -> Option<Result<Option<serde_json::Value>, JsonError>> {
        self.values.get(&e).map(Component::to_json)
    }
    fn verify(&self, w: &World) -> Vec<(Entity, String)> {
        let mut out = Vec::new();
        let mut refs = Vec::new();
        for (e, v) in &self.values {
            if let Err(m) = v.verify(*e, w) {
                out.push((*e, m));
            }
            refs.clear();
            v.references(&mut refs);
            if let Some(r) = refs.iter().find(|r| !w.alive.contains(r)) {
                out.push((*e, format!("refers to {r:?}, which is not alive")));
            }
        }
        out
    }
    fn visit_debug(&self, f: &mut dyn FnMut(Entity, &dyn fmt::Debug)) {
        for (e, v) in &self.values {
            f(*e, v);
        }
    }
    fn entities(&self) -> Vec<Entity> {
        self.values.keys().copied().collect()
    }
    fn references(&self, e: Entity, out: &mut Vec<Entity>) {
        if let Some(v) = self.values.get(&e) {
            v.references(out);
        }
    }
    fn payload_bytes(&self) -> u64 {
        self.values.values().map(Component::payload_bytes).sum()
    }
    fn clone_box(&self) -> Box<dyn AnyColumn> {
        Box::new(self.clone())
    }
    fn subset_box(&self, keep: &BTreeSet<Entity>) -> Box<dyn AnyColumn> {
        Box::new(Column {
            values: subset_of(&self.values, keep),
        })
    }
    fn retain(&mut self, keep: &BTreeSet<Entity>) {
        self.values.retain(|e, _| keep.contains(e));
    }
    fn split_off_box(&mut self, keep: &BTreeSet<Entity>) -> Box<dyn AnyColumn> {
        Box::new(Column {
            values: keep
                .iter()
                .filter_map(|e| self.values.remove(e).map(|v| (*e, v)))
                .collect(),
        })
    }
    fn absorb(&mut self, mut other: Box<dyn AnyColumn>) {
        let other = other
            .as_any_mut()
            .downcast_mut::<Column<T>>()
            .expect("absorbing a column of the same type");
        self.values.append(&mut other.values);
    }
    fn move_entity(&mut self, from: Entity, to: Entity) {
        if let Some(v) = self.values.remove(&from) {
            self.values.insert(to, v);
        }
    }
    fn remapped(&self, e: Entity, from: &[usize]) -> Option<Result<Box<dyn Any + Send>, String>> {
        let r = self.values.get(&e)?.remap_primitives(from)?;
        Some(r.map(|v| Box::new(v) as Box<dyn Any + Send>))
    }
    fn vertices_remapped(
        &self,
        e: Entity,
        primitive: usize,
        from: &[u32],
    ) -> Option<Result<Box<dyn Any + Send>, String>> {
        let r = self.values.get(&e)?.remap_vertices(primitive, from)?;
        Some(r.map(|v| Box::new(v) as Box<dyn Any + Send>))
    }
    fn vertices_blended(
        &self,
        e: Entity,
        primitive: usize,
        from: &[VertexBlend],
    ) -> Option<Result<Box<dyn Any + Send>, String>> {
        let r = self.values.get(&e)?.blend_vertices(primitive, from)?;
        Some(r.map(|v| Box::new(v) as Box<dyn Any + Send>))
    }
    fn put_remapped(&mut self, e: Entity, value: Box<dyn Any + Send>) {
        let v = *(value as Box<dyn Any>)
            .downcast::<T>()
            .expect("a value remapped by this column");
        self.values.insert(e, v);
    }
    fn copy_entity(&mut self, from: Entity, to: Entity) {
        if let Some(v) = self.values.get(&from).cloned() {
            self.values.insert(to, v);
        }
    }
    fn linear(&self, e: Entity, l: &crate::Linear) -> Option<Linearity<Box<dyn Any + Send>>> {
        Some(match self.values.get(&e)?.apply_linear(l) {
            Linearity::Invariant => Linearity::Invariant,
            Linearity::Applied(v) => Linearity::Applied(Box::new(v) as Box<dyn Any + Send>),
            Linearity::Drop => Linearity::Drop,
            Linearity::Unsupported => Linearity::Unsupported,
        })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The entries of `values` whose entity is in `keep`, walking the smaller of the two.
fn subset_of<T: Clone>(values: &BTreeMap<Entity, T>, keep: &BTreeSet<Entity>) -> BTreeMap<Entity, T> {
    if keep.len() < values.len() {
        keep.iter()
            .filter_map(|e| values.get(e).map(|v| (*e, v.clone())))
            .collect()
    } else {
        values
            .iter()
            .filter(|(e, _)| keep.contains(e))
            .map(|(e, v)| (*e, v.clone()))
            .collect()
    }
}

trait AnyResource: Any + Send + Sync {
    fn name(&self) -> &'static str;
    fn version(&self) -> u32;
    fn json(&self) -> Result<Option<serde_json::Value>, JsonError>;
    fn debug(&self) -> &dyn fmt::Debug;
    fn verify(&self, w: &World) -> Result<(), String>;
    fn references(&self, out: &mut Vec<Entity>);
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool);
    fn clone_box(&self) -> Box<dyn AnyResource>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Resource> AnyResource for T {
    fn name(&self) -> &'static str {
        T::NAME
    }
    fn version(&self) -> u32 {
        T::VERSION
    }
    fn json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        self.to_json()
    }
    fn debug(&self) -> &dyn fmt::Debug {
        self
    }
    fn verify(&self, w: &World) -> Result<(), String> {
        Resource::verify(self, w)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        Resource::references(self, out);
    }
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool) {
        Resource::retain_references(self, keep);
        let mut left = Vec::new();
        Resource::references(self, &mut left);
        debug_assert!(
            left.into_iter().all(keep),
            "{} keeps a reference it was told to forget",
            T::NAME
        );
    }
    fn clone_box(&self) -> Box<dyn AnyResource> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Entities with their components, opaque components and resources.
#[derive(Default)]
pub struct World {
    next: u32,
    /// Alive entities. Despawned ids stay out forever.
    alive: std::collections::BTreeSet<Entity>,
    columns: BTreeMap<TypeId, Box<dyn AnyColumn>>,
    /// Component name -> owning type, to refuse two Rust types claiming one name.
    names: BTreeMap<&'static str, TypeId>,
    opaque: BTreeMap<String, BTreeMap<Entity, Opaque>>,
    resources: BTreeMap<TypeId, Box<dyn AnyResource>>,
    opaque_resources: BTreeMap<String, Opaque>,
    /// While a pass runs with conservation diagnostics on ([`crate::ledger`]): per entity whose primitives it rebuilt
    /// ([`World::rebuild_primitives`]), the rebuilds composed; `None` for an entity whose mesh was also replaced another
    /// way in between (counted net).
    rebuilds: Option<RebuildLog>,
    /// While a pass runs ([`World::start_effects`]): what it did that the pass manager can't see by comparing the world
    /// before and after (resources written in place or replaced, entities filled by copying another).
    effects: Option<Effects>,
}

/// What a pass did to a world that a before/after comparison of names doesn't show ([`World::start_effects`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct Effects {
    /// Resources it replaced or borrowed mutably (an existing one written, or written in place), by name.
    pub(crate) resources_written: BTreeSet<String>,
    /// Resources it took away ([`World::remove_resource`]), by name: one the world holds again afterwards was
    /// replaced, a write as much as [`World::insert_resource`] over it.
    pub(crate) resources_removed: BTreeSet<String>,
    /// Copies it made: (source, target) for each [`World::duplicate`] and [`World::copy_components`]. An exemption,
    /// not a check: a name a copy puts on its target is the source's, so it counts as no addition.
    pub(crate) copies: Vec<(Entity, Entity)>,
}

/// The primitive rebuilds a pass made, per entity; `None` when its mesh was also replaced another way between two of
/// them.
pub(crate) type RebuildLog = BTreeMap<Entity, Option<Rebuilt>>;

/// One mesh's rebuilds in a pass, composed.
#[derive(Clone, Debug)]
pub(crate) struct Rebuilt {
    /// The mesh the first rebuild started from ([`crate::Mesh`]'s identity) and its primitive count.
    pub(crate) source: u64,
    pub(crate) first: usize,
    /// For each primitive of the mesh the last rebuild made, the source primitive it comes from.
    pub(crate) from: Vec<usize>,
    /// The mesh the last rebuild made.
    pub(crate) made: u64,
}

impl Clone for World {
    fn clone(&self) -> Self {
        World {
            next: self.next,
            alive: self.alive.clone(),
            columns: self.columns.iter().map(|(k, v)| (*k, v.clone_box())).collect(),
            names: self.names.clone(),
            opaque: self.opaque.clone(),
            resources: self.resources.iter().map(|(k, v)| (*k, v.clone_box())).collect(),
            opaque_resources: self.opaque_resources.clone(),
            rebuilds: None,
            effects: None,
        }
    }
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for c in self.columns.values() {
            counts.insert(c.name(), c.len());
        }
        for (name, col) in &self.opaque {
            counts.insert(name, col.len());
        }
        f.debug_struct("World")
            .field("entities", &self.alive.len())
            .field("components", &counts)
            .finish()
    }
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every resource, copied, with its references to entities outside `alive` forgotten.
    fn resources_for(&self, alive: &BTreeSet<Entity>) -> BTreeMap<TypeId, Box<dyn AnyResource>> {
        self.resources
            .iter()
            .map(|(k, v)| {
                let mut r = v.clone_box();
                r.retain_references(&|e| alive.contains(&e));
                (*k, r)
            })
            .collect()
    }

    /// Every resource forgets its references to the entities `gone` accepts.
    fn forget_in_resources(&mut self, gone: impl Fn(Entity) -> bool) {
        for r in self.resources.values_mut() {
            r.retain_references(&|e| !gone(e));
        }
    }

    pub fn spawn(&mut self) -> Entity {
        let e = Entity(self.next);
        self.next = self.next.checked_add(1).expect("more than u32::MAX entities");
        self.alive.insert(e);
        e
    }

    /// Spawn with an explicit id, for readers of serialised worlds that keep the file's ids. Ids must come in
    /// ascending order (gaps allowed); panics on an id at or below one already handed out, so ids stay unique.
    pub fn spawn_at(&mut self, e: Entity) -> Entity {
        assert!(
            e.0 >= self.next,
            "entity {e:?} is not above every id already used"
        );
        self.alive.insert(e);
        self.next = e.0.checked_add(1).expect("more than u32::MAX entities");
        e
    }

    /// Remove an entity and all its components. Returns false if it was not alive.
    pub fn despawn(&mut self, e: Entity) -> bool {
        if !self.alive.remove(&e) {
            return false;
        }
        for c in self.columns.values_mut() {
            c.remove_entity(e);
        }
        for col in self.opaque.values_mut() {
            col.remove(&e);
        }
        self.forget_in_resources(|x| x == e);
        true
    }

    /// Every component of `from`, typed and opaque, moved onto `to`, replacing `to`'s of the same name (`to` keeps
    /// the others), then `from` despawned: for a pass that swaps one form of an entity for another without knowing
    /// its components (`select-lod`). `Ok(false)`, and nothing changed, unless both are alive and differ.
    ///
    /// Refused, changing nothing, when it would leave data describing one mesh beside another
    /// ([`Component::GEOMETRY`]): `from` brings a mesh (`core.mesh` or `core.mesh_file`) while `to` keeps such data
    /// `from` has none of (take `to`'s away first: [`World::remove_mesh`]), or `from` brings such data without a mesh
    /// onto a `to` that has one.
    pub fn move_components(&mut self, from: Entity, to: Entity) -> Result<bool, String> {
        if from == to || !self.alive.contains(&from) || !self.alive.contains(&to) {
            return Ok(false);
        }
        let has_mesh = |e| self.has::<crate::Mesh>(e) || self.has::<crate::MeshFile>(e);
        let (from_mesh, to_mesh) = (has_mesh(from), has_mesh(to));
        let mut stale: Vec<&'static str> = self
            .columns
            .values()
            .filter(|c| c.geometry().describes_mesh())
            .filter(|c| {
                let (at_from, at_to) = (c.contains(from), c.contains(to));
                (from_mesh && at_to && !at_from) || (!from_mesh && to_mesh && at_from)
            })
            .map(|c| c.name())
            .collect();
        if !stale.is_empty() {
            stale.sort_unstable();
            return Err(format!(
                "moving {from:?} onto {to:?} would leave data describing one mesh beside another: {stale:?}"
            ));
        }
        for c in self.columns.values_mut() {
            c.move_entity(from, to);
        }
        for col in self.opaque.values_mut() {
            if let Some(v) = col.remove(&from) {
                col.insert(to, v);
            }
        }
        self.alive.remove(&from);
        self.forget_in_resources(|x| x == from);
        self.note_copy(from, to);
        Ok(true)
    }

    /// A new entity with a copy of every typed and opaque component of `e`; `None` when `e` is not alive. Shallow:
    /// references in the copies still name the entities `e`'s did (a pass that needs referenced entities copied too,
    /// LOD forms of a prototype, copies them itself).
    pub fn duplicate(&mut self, e: Entity) -> Option<Entity> {
        if !self.is_alive(e) {
            return None;
        }
        let d = self.spawn();
        for c in self.columns.values_mut() {
            c.copy_entity(e, d);
        }
        for values in self.opaque.values_mut() {
            if let Some(v) = values.get(&e).cloned() {
                values.insert(d, v);
            }
        }
        self.note_copy(e, d);
        Some(d)
    }

    /// Map `e`'s local space by `l`: every component of `e` through [`Component::apply_linear`]. All or nothing:
    /// when a component is [`Linearity::Unsupported`] (opaque components always are) nothing changes and the error
    /// names every such component, sorted. Otherwise the names of the components it dropped ([`Linearity::Drop`]),
    /// sorted.
    pub fn apply_linear(&mut self, e: Entity, l: &crate::Linear) -> Result<Vec<String>, Vec<String>> {
        let mut unsupported: Vec<String> = self
            .opaque
            .iter()
            .filter(|(_, values)| values.contains_key(&e))
            .map(|(name, _)| name.clone())
            .collect();
        let mut done = Vec::new();
        for (id, c) in &self.columns {
            match c.linear(e, l) {
                None | Some(Linearity::Invariant) => {}
                Some(Linearity::Unsupported) => unsupported.push(c.name().to_string()),
                Some(r) => done.push((*id, r)),
            }
        }
        if !unsupported.is_empty() {
            unsupported.sort();
            return Err(unsupported);
        }
        let mut dropped = Vec::new();
        for (id, r) in done {
            if let Some(c) = self.columns.get_mut(&id) {
                match r {
                    Linearity::Applied(v) => c.put_remapped(e, v),
                    Linearity::Drop => {
                        c.remove_entity(e);
                        dropped.push(c.name().to_string());
                    }
                    Linearity::Invariant | Linearity::Unsupported => {}
                }
            }
        }
        dropped.sort();
        Ok(dropped)
    }

    /// Keep `e`'s per-primitive components in step with its rebuilt mesh: every typed component that follows
    /// primitives ([`Component::remap_primitives`]) remapped by `from` (new primitive `i` from old `from[i]`). A pass
    /// that splits, reorders or drops a mesh's primitives calls it once, so it need not know the dialects' components.
    /// All or nothing: an error (an index a component does not have, or a component refusing the mapping) names the
    /// component and changes nothing.
    ///
    /// An opaque component (bytes a reader carried without a type) can't follow: whether it describes primitives
    /// can't be known, so a mapping that changes the list (anything but `0..n` in order) is refused while `e` has
    /// one, before anything changes, naming them. The program removes them (a selection, reported) or gives them a
    /// typed form first.
    pub fn remap_primitives(&mut self, e: Entity, from: &[usize]) -> Result<(), String> {
        let identity = from.iter().enumerate().all(|(i, &f)| i == f)
            && self
                .get::<crate::Mesh>(e)
                .is_none_or(|m| m.primitives().len() == from.len());
        if !identity {
            let mut opaque: Vec<&String> = self
                .opaque
                .iter()
                .filter(|(_, values)| values.contains_key(&e))
                .map(|(name, _)| name)
                .collect();
            if !opaque.is_empty() {
                opaque.sort();
                return Err(format!(
                    "{e:?}: opaque components can't follow a change to its primitives: {opaque:?} (remove them, or \
                     give them a typed form, before the pass)"
                ));
            }
        }
        let mut done = Vec::new();
        // by name, so the error reported is always the same one
        let mut columns: Vec<_> = self.columns.iter().collect();
        columns.sort_by_key(|(_, c)| c.name());
        for (name, c) in columns {
            match c.remapped(e, from) {
                None if c.contains(e) && c.geometry().describes_mesh() => {
                    return Err(format!(
                        "{}: holds data per primitive ({:?}) but does not follow a change to them",
                        c.name(),
                        c.geometry()
                    ));
                }
                None => {}
                Some(Ok(v)) => done.push((*name, v)),
                Some(Err(m)) => return Err(format!("{}: {m}", c.name())),
            }
        }
        for (name, v) in done {
            if let Some(c) = self.columns.get_mut(&name) {
                c.put_remapped(e, v);
            }
        }
        Ok(())
    }

    /// Remap every per-vertex component of `e` ([`Component::remap_vertices`]) for its mesh primitive `primitive`,
    /// whose vertices a pass compacted or reordered: new vertex `i` is old vertex `from[i]`. The pass changes the
    /// primitive itself and calls this for every other component, so it need not know the dialects' components; it
    /// calls it only when the vertices changed. All or nothing: an error (an index a component does not have, or a
    /// component refusing the mapping) names the component and changes nothing. Refused, changing nothing, while `e`
    /// has an opaque component, as [`World::remap_primitives`] refuses then (whether it describes vertices can't be
    /// known).
    pub fn remap_vertices(&mut self, e: Entity, primitive: usize, from: &[u32]) -> Result<(), String> {
        let done = self.vertex_remapped_components(e, primitive, from)?;
        for (name, v) in done {
            if let Some(c) = self.columns.get_mut(&name) {
                c.put_remapped(e, v);
            }
        }
        Ok(())
    }

    /// Why [`World::remap_vertices`] would refuse, without changing anything: a pass checks every entity before it
    /// changes any.
    pub fn check_remap_vertices(&self, e: Entity, primitive: usize, from: &[u32]) -> Result<(), String> {
        self.vertex_remapped_components(e, primitive, from).map(|_| ())
    }

    fn vertex_remapped_components(
        &self,
        e: Entity,
        primitive: usize,
        from: &[u32],
    ) -> Result<Vec<Blended>, String> {
        let mut opaque: Vec<&String> = self
            .opaque
            .iter()
            .filter(|(_, values)| values.contains_key(&e))
            .map(|(name, _)| name)
            .collect();
        if !opaque.is_empty() {
            opaque.sort();
            return Err(format!(
                "{e:?}: opaque components can't follow a change to a primitive's vertices: {opaque:?} (remove them,                  or give them a typed form, before the pass)"
            ));
        }
        let mut done = Vec::new();
        // by name, so the error reported is always the same one
        let mut columns: Vec<_> = self.columns.iter().collect();
        columns.sort_by_key(|(_, c)| c.name());
        for (name, c) in columns {
            match c.vertices_remapped(e, primitive, from) {
                None if c.contains(e) && c.geometry() == Geometry::PerVertex => {
                    return Err(format!(
                        "{}: holds data per vertex but does not follow a change to them",
                        c.name()
                    ));
                }
                None => {}
                Some(Ok(v)) => done.push((*name, v)),
                Some(Err(m)) => return Err(format!("{}: {m}", c.name())),
            }
        }
        Ok(done)
    }

    /// Rebuild every per-vertex component of `e` ([`Component::blend_vertices`]) for its mesh primitive `primitive`,
    /// whose vertices a pass made as blends of the old ones: new vertex `i` is `from[i]`. The pass changes the primitive
    /// itself and calls this for every other component, so it need not know the dialects' components. All or nothing,
    /// like [`World::remap_vertices`]: an error (an index a component does not have, a component that cannot combine
    /// its vertices, an opaque component on `e`) names the cause and changes nothing.
    pub fn blend_vertices(
        &mut self,
        e: Entity,
        primitive: usize,
        from: &[VertexBlend],
    ) -> Result<(), String> {
        let done = self.blended_components(e, primitive, from)?;
        for (name, v) in done {
            if let Some(c) = self.columns.get_mut(&name) {
                c.put_remapped(e, v);
            }
        }
        Ok(())
    }

    /// Why [`World::blend_vertices`] would refuse, without changing anything: a pass checks every entity before it
    /// changes any.
    pub fn check_blend_vertices(
        &self,
        e: Entity,
        primitive: usize,
        from: &[VertexBlend],
    ) -> Result<(), String> {
        self.blended_components(e, primitive, from).map(|_| ())
    }

    fn blended_components(
        &self,
        e: Entity,
        primitive: usize,
        from: &[VertexBlend],
    ) -> Result<Vec<Blended>, String> {
        let mut opaque: Vec<&String> = self
            .opaque
            .iter()
            .filter(|(_, values)| values.contains_key(&e))
            .map(|(name, _)| name)
            .collect();
        if !opaque.is_empty() {
            opaque.sort();
            return Err(format!(
                "{e:?}: opaque components can't follow a change to a primitive's vertices: {opaque:?} (remove them, \
                 or give them a typed form, before the pass)"
            ));
        }
        let mut done = Vec::new();
        // by name, so the error reported is always the same one
        let mut columns: Vec<_> = self.columns.iter().collect();
        columns.sort_by_key(|(_, c)| c.name());
        for (name, c) in columns {
            match c.vertices_blended(e, primitive, from) {
                None if c.contains(e) && c.geometry() == Geometry::PerVertex => {
                    return Err(format!(
                        "{}: holds data per vertex but cannot combine vertices (it has no blend_vertices)",
                        c.name()
                    ));
                }
                None => {}
                Some(Ok(v)) => done.push((*name, v)),
                Some(Err(m)) => return Err(format!("{}: {m}", c.name())),
            }
        }
        Ok(done)
    }

    /// Take `e`'s mesh away with every component describing it (each typed component whose [`Component::GEOMETRY`]
    /// says it holds something per primitive or vertex) and its file link (`core.mesh_file`, which a load would restore it
    /// from): for an entity left drawing nothing, whose per-primitive data would describe no primitive, or one whose
    /// geometry another replaces (`select-lod`). A mesh still only in its file (`core.mesh_file` without `core.mesh`, as
    /// a world is before its load) goes the same way, with the per-primitive data describing it. Returns the names
    /// taken, the mesh's first; none when `e` has neither. Refused, changing nothing, while `e` has an opaque component
    /// ([`World::check_remove_mesh`]).
    pub fn remove_mesh(&mut self, e: Entity) -> Result<Vec<&'static str>, String> {
        let mesh = self.has::<crate::Mesh>(e);
        if !mesh && !self.has::<crate::MeshFile>(e) {
            return Ok(Vec::new());
        }
        self.check_remove_mesh(e)?;
        let mut names: Vec<&'static str> = mesh.then_some(crate::Mesh::NAME).into_iter().collect();
        let mut per_primitive: Vec<(&'static str, TypeId)> = self
            .columns
            .iter()
            .filter(|(_, c)| c.contains(e) && c.geometry().describes_mesh())
            .map(|(k, c)| (c.name(), *k))
            .collect();
        per_primitive.sort();
        for (name, k) in per_primitive {
            if let Some(c) = self.columns.get_mut(&k) {
                c.remove_entity(e);
            }
            names.push(name);
        }
        self.remove::<crate::Mesh>(e);
        if self.remove::<crate::MeshFile>(e).is_some() {
            names.push(crate::MeshFile::NAME);
        }
        Ok(names)
    }

    /// Why [`World::remove_mesh`] would refuse `e`, so a pass can check every entity before it changes any: an opaque
    /// component on it, as [`World::remap_primitives`] refuses a change to the primitives then (whether it describes
    /// primitives can't be known).
    pub fn check_remove_mesh(&self, e: Entity) -> Result<(), String> {
        let mut opaque: Vec<&String> = self
            .opaque
            .iter()
            .filter(|(_, values)| values.contains_key(&e))
            .map(|(name, _)| name)
            .collect();
        if opaque.is_empty() {
            return Ok(());
        }
        opaque.sort();
        Err(format!(
            "{e:?}: opaque components can't follow its mesh's removal: {opaque:?} (remove them, or give them a typed \
             form, before the pass)"
        ))
    }

    /// The names of `e`'s typed components that hold something per vertex of its mesh ([`Geometry::PerVertex`]), in
    /// name order: what a pass that joins primitives into one (whose vertices a source primitive's per-vertex data
    /// does not cover) must refuse, since [`Component::remap_vertices`] maps only one source primitive's vertices.
    pub fn per_vertex_components(&self, e: Entity) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .columns
            .values()
            .filter(|c| c.contains(e) && c.geometry() == Geometry::PerVertex)
            .map(|c| c.name())
            .collect();
        names.sort_unstable();
        names
    }

    /// Every component of `from`, typed and opaque, copied onto `to`, replacing `to`'s of the same name (`to` keeps
    /// the others): for a pass that makes another entity like one it knows only partly (a prototype per tint row,
    /// keeping whatever dialect data the reader attached). References are copied as they are. False, and nothing
    /// changed, unless both are alive and differ.
    pub fn copy_components(&mut self, from: Entity, to: Entity) -> bool {
        if from == to || !self.alive.contains(&from) || !self.alive.contains(&to) {
            return false;
        }
        for c in self.columns.values_mut() {
            c.copy_entity(from, to);
        }
        for col in self.opaque.values_mut() {
            if let Some(v) = col.get(&from).cloned() {
                col.insert(to, v);
            }
        }
        self.note_copy(from, to);
        true
    }

    /// Rebuild `e`'s mesh primitives: `f` returns the new primitives and, for each, the old primitive it comes from (a
    /// source may repeat or be absent, as for [`World::remap_primitives`]). The new mesh and every per-primitive
    /// component change together, or, when `f` fails or a component cannot follow, nothing does. The one way to change
    /// a mesh's primitive list.
    pub fn rebuild_primitives(
        &mut self,
        e: Entity,
        f: impl FnOnce(&crate::Mesh) -> Result<(Vec<crate::Primitive>, Vec<usize>), String>,
    ) -> Result<(), String> {
        let mesh = self
            .get::<crate::Mesh>(e)
            .ok_or_else(|| format!("{e:?} has no core.mesh"))?;
        let (primitives, from) = f(mesh)?;
        if primitives.len() != from.len() {
            return Err(format!(
                "{} primitives with {} sources",
                primitives.len(),
                from.len()
            ));
        }
        let (old, source) = (mesh.primitives().len(), mesh.id());
        if let Some(&i) = from.iter().find(|&&i| i >= old) {
            return Err(format!("source primitive {i} of {old}"));
        }
        self.remap_primitives(e, &from)?;
        let made = crate::Mesh::new(primitives);
        self.record_rebuild(e, source, old, from, made.id());
        self.insert(e, made);
        Ok(())
    }

    /// Edit `e`'s mesh with `f`, which may also rebuild its primitive list: `f` returns `None` when the list keeps its
    /// primitives in order (edited in place at most: the mesh stays the same mesh, whatever `f` built to hold them),
    /// else, for each primitive now, the one it comes from, as
    /// [`World::rebuild_primitives`] (per-primitive components follow, and the conservation diagnostics count the
    /// rebuild exactly). False when `e` has no mesh.
    ///
    /// All or nothing: when `f` fails, its mapping is malformed, or a per-primitive component can't follow it, the
    /// mesh and its components are as they were (the mesh is kept while `f` runs, to put back), and the error says
    /// why.
    pub fn edit_mesh(
        &mut self,
        e: Entity,
        f: impl FnOnce(&mut crate::Mesh) -> Result<Option<Vec<usize>>, String>,
    ) -> Result<bool, String> {
        // the edit works on a copy: `e` keeps the original until the edit is accepted
        let Some(mut mesh) = self.get::<crate::Mesh>(e).cloned() else {
            return Ok(false);
        };
        let (old, source) = (mesh.primitives().len(), mesh.id());
        let from = f(&mut mesh)?;
        let Some(from) = from else {
            let n = mesh.primitives().len();
            if n != old {
                return Err(format!("{e:?}: {n} primitives from {old} without their sources"));
            }
            // its primitives kept in order: the same mesh, even when `f` built a new one to hold them
            mesh.keep_id(source);
            self.insert(e, mesh);
            return Ok(true);
        };
        if mesh.primitives().len() != from.len() || from.iter().any(|&i| i >= old) {
            let n = mesh.primitives().len();
            return Err(format!(
                "{e:?}: {n} primitives with {} sources of {old}",
                from.len()
            ));
        }
        // all or nothing: a component that can't follow leaves the components and the original mesh as they were
        self.remap_primitives(e, &from)?;
        let made = crate::Mesh::new(mesh.into_primitives());
        self.record_rebuild(e, source, old, from, made.id());
        self.insert(e, made);
        Ok(true)
    }

    /// Record a rebuild of `e`'s mesh `source` (`old` primitives) into `made` for the conservation diagnostics, when
    /// they run.
    fn record_rebuild(&mut self, e: Entity, source: u64, old: usize, from: Vec<usize>, made: u64) {
        if let Some(log) = &mut self.rebuilds {
            match log.entry(e) {
                std::collections::btree_map::Entry::Vacant(v) => {
                    v.insert(Some(Rebuilt {
                        source,
                        first: old,
                        from,
                        made,
                    }));
                }
                // compose with the earlier rebuild, unless this one starts from another mesh (replaced in between)
                std::collections::btree_map::Entry::Occupied(mut o) => {
                    let composed = o.get_mut().take().filter(|r| r.made == source).map(|r| Rebuilt {
                        from: from.iter().map(|&i| r.from[i]).collect(),
                        made,
                        ..r
                    });
                    *o.get_mut() = composed;
                }
            }
        }
    }

    /// Start recording primitive rebuilds (the pass manager, around a pass with conservation diagnostics on).
    /// Record, until [`World::take_effects`], the resources written and the copies made.
    pub(crate) fn start_effects(&mut self) {
        self.effects = Some(Effects::default());
    }

    /// Stop recording and return what was recorded since [`World::start_effects`].
    pub(crate) fn take_effects(&mut self) -> Effects {
        self.effects.take().unwrap_or_default()
    }

    fn note_resource_write(&mut self, name: &str) {
        if let Some(fx) = &mut self.effects {
            fx.resources_written.insert(name.to_owned());
        }
    }

    fn note_copy(&mut self, from: Entity, to: Entity) {
        if let Some(fx) = &mut self.effects {
            fx.copies.push((from, to));
        }
    }

    pub(crate) fn start_rebuild_log(&mut self) {
        self.rebuilds = Some(BTreeMap::new());
    }

    /// Stop recording and return what was recorded since [`World::start_rebuild_log`].
    pub(crate) fn take_rebuild_log(&mut self) -> RebuildLog {
        self.rebuilds.take().unwrap_or_default()
    }

    pub fn is_alive(&self, e: Entity) -> bool {
        self.alive.contains(&e)
    }

    /// The index the next spawned entity gets: an entity with an index at or above it was made after this call
    /// (ids are handed out in order and never reused; a part loaded with [`World::subset`] keeps its planning world's,
    /// so what a pass makes in it is told from what was loaded).
    pub fn next_index(&self) -> u32 {
        self.next
    }

    /// The entities `e`'s components refer to (not its resources'), in no particular order, repeats possible.
    pub fn references(&self, e: Entity) -> Vec<Entity> {
        let mut out = Vec::new();
        for c in self.columns.values() {
            c.references(e, &mut out);
        }
        out
    }

    /// The entities nothing in the world refers to (through any entity's components): its independent content,
    /// placements, markers, standalone soups, rather than the prototypes, materials and textures they reach.
    pub fn unreferenced(&self) -> BTreeSet<Entity> {
        let mut referenced = BTreeSet::new();
        for &e in &self.alive {
            referenced.extend(self.references(e));
        }
        self.alive
            .iter()
            .copied()
            .filter(|e| !referenced.contains(e))
            .collect()
    }

    /// `roots` and every alive entity they reach through [`Component::references`], transitively. Opaque
    /// components declare no references: an entity's opaque components travel with it in [`World::subset`], but
    /// nothing they may name is followed.
    pub fn closure(&self, roots: impl IntoIterator<Item = Entity>) -> BTreeSet<Entity> {
        let mut seen = BTreeSet::new();
        let mut todo: Vec<Entity> = roots.into_iter().collect();
        let mut out = Vec::new();
        while let Some(e) = todo.pop() {
            if !self.alive.contains(&e) || !seen.insert(e) {
                continue;
            }
            for c in self.columns.values() {
                c.references(e, &mut out);
            }
            todo.append(&mut out);
        }
        seen
    }

    /// Every typed reference ([`Component::references`]) to an entity of `targets`: (component name, the entity holding
    /// it, the entity referred to), sorted. Opaque components declare no references, as in [`World::closure`].
    pub(crate) fn references_to(&self, targets: &BTreeSet<Entity>) -> Vec<(&'static str, Entity, Entity)> {
        let mut edges = Vec::new();
        let mut refs = Vec::new();
        for c in self.columns.values() {
            for source in c.entities() {
                refs.clear();
                c.references(source, &mut refs);
                edges.extend(
                    refs.iter()
                        .filter(|r| targets.contains(r))
                        .map(|&target| (c.name(), source, target)),
                );
            }
        }
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    /// A world of only `keep`'s entities (those alive) with all their components, typed and opaque, under the
    /// same ids, and every resource (forgetting its references to entities left out). Entities spawned in it get ids
    /// above every id of `self`.
    pub fn subset(&self, keep: &BTreeSet<Entity>) -> World {
        let alive: BTreeSet<Entity> = self.alive.intersection(keep).copied().collect();
        World {
            next: self.next,
            resources: self.resources_for(&alive),
            alive,
            columns: self
                .columns
                .iter()
                .map(|(k, v)| (*k, v.subset_box(keep)))
                .collect(),
            names: self.names.clone(),
            opaque: self
                .opaque
                .iter()
                .map(|(k, v)| (k.clone(), subset_of(v, keep)))
                .collect(),
            opaque_resources: self.opaque_resources.clone(),
            rebuilds: None,
            effects: None,
        }
    }

    /// Move `keep`'s entities (those alive), with all their components, out into a world of their own: what
    /// [`World::subset`] copies, without copying. The part has every resource (copied, as a subset's) and spawns ids
    /// above every id of `self`, as a subset does; [`World::absorb`] moves it back. This world's resources keep their
    /// references to the moved entities, which come back with the part.
    pub fn split_off(&mut self, keep: &BTreeSet<Entity>) -> World {
        let alive: BTreeSet<Entity> = self.alive.intersection(keep).copied().collect();
        for e in &alive {
            self.alive.remove(e);
        }
        World {
            next: self.next,
            columns: self
                .columns
                .iter_mut()
                .map(|(k, v)| (*k, v.split_off_box(&alive)))
                .collect(),
            names: self.names.clone(),
            opaque: self
                .opaque
                .iter_mut()
                .map(|(k, v)| {
                    let part = alive
                        .iter()
                        .filter_map(|e| v.remove(e).map(|o| (*e, o)))
                        .collect();
                    (k.clone(), part)
                })
                .collect(),
            resources: self.resources_for(&alive),
            alive,
            opaque_resources: self.opaque_resources.clone(),
            rebuilds: None,
            effects: None,
        }
    }

    /// Move every entity of `part` (a [`World::split_off`] of this world) back in, with its components; `part`'s
    /// resources are dropped. Ids the part spawned and despawned again were never alive here and are not reserved, so
    /// a split, written and absorbed part leaves this world as it was. Panics if an entity of `part` is alive here.
    pub fn absorb(&mut self, part: World) {
        for e in &part.alive {
            assert!(!self.alive.contains(e), "absorbing {e:?}, which is alive here");
        }
        // ids the part spawned and despawned again are not taken here: the next id stays this world's own,
        // unless the part brings back an entity at or above it
        if let Some(last) = part.alive.last() {
            self.next = self.next.max(last.0 + 1);
        }
        self.alive.extend(part.alive);
        for (k, col) in part.columns {
            match self.columns.get_mut(&k) {
                Some(mine) => mine.absorb(col),
                None => {
                    self.names.insert(col.name(), k);
                    self.columns.insert(k, col);
                }
            }
        }
        for (k, mut col) in part.opaque {
            self.opaque.entry(k).or_default().append(&mut col);
        }
    }

    /// Despawn every entity outside `keep`: in place, what [`World::subset`] copies.
    pub fn retain(&mut self, keep: &BTreeSet<Entity>) {
        self.alive.retain(|e| keep.contains(e));
        self.forget_in_resources(|e| !keep.contains(&e));
        for c in self.columns.values_mut() {
            c.retain(keep);
        }
        for col in self.opaque.values_mut() {
            col.retain(|e, _| keep.contains(e));
        }
    }

    /// Alive entities in ascending order.
    pub fn entities(&self) -> impl DoubleEndedIterator<Item = Entity> + '_ {
        self.alive.iter().copied()
    }

    pub fn entity_count(&self) -> usize {
        self.alive.len()
    }

    fn register<T: Component>(&mut self) -> &mut Column<T> {
        let id = TypeId::of::<T>();
        if let Some(&owner) = self.names.get(T::NAME) {
            assert!(owner == id, "two component types share the name {:?}", T::NAME);
        } else {
            assert!(
                !self.is_resource_name(T::NAME),
                "{} is a resource's name: a name is a component's or a resource's, not both",
                T::NAME
            );
            assert!(
                !self.opaque.contains_key(T::NAME),
                "{} is an opaque component's name: a name is typed or opaque, never both",
                T::NAME
            );
            self.names.insert(T::NAME, id);
        }
        self.columns
            .entry(id)
            .or_insert_with(|| Box::new(Column::<T>::default()))
            .as_any_mut()
            .downcast_mut::<Column<T>>()
            .expect("column type matches its TypeId")
    }

    /// Attach (or replace) a component. Panics if the entity is not alive: attaching to a dead entity is a bug.
    /// Replacing a value an entity keeps is an in-place edit to the pass manager, as through [`World::get_mut`]:
    /// declared in [`crate::pass::Pass::modifies`], not checked.
    pub fn insert<T: Component>(&mut self, e: Entity, value: T) -> Option<T> {
        assert!(self.alive.contains(&e), "insert {} on dead entity {e:?}", T::NAME);
        self.register::<T>().insert(e, value)
    }

    pub fn remove<T: Component>(&mut self, e: Entity) -> Option<T> {
        self.column_mut_existing::<T>()?.remove(e)
    }

    pub fn get<T: Component>(&self, e: Entity) -> Option<&T> {
        self.column::<T>()?.get(e)
    }

    pub fn get_mut<T: Component>(&mut self, e: Entity) -> Option<&mut T> {
        self.column_mut_existing::<T>()?.get_mut(e)
    }

    pub fn has<T: Component>(&self, e: Entity) -> bool {
        self.column::<T>().is_some_and(|c| c.contains(e))
    }

    /// The column of `T`, if any entity ever had one.
    pub fn column<T: Component>(&self) -> Option<&Column<T>> {
        self.columns
            .get(&TypeId::of::<T>())
            .map(|c| c.as_any().downcast_ref().expect("column type"))
    }

    fn column_mut_existing<T: Component>(&mut self) -> Option<&mut Column<T>> {
        self.columns
            .get_mut(&TypeId::of::<T>())
            .map(|c| c.as_any_mut().downcast_mut().expect("column type"))
    }

    /// Mutable access to the values of `T` (entities cannot be added or removed through it).
    pub fn column_mut<T: Component>(&mut self) -> Option<ColumnMut<'_, T>> {
        self.column_mut_existing::<T>().map(ColumnMut)
    }

    /// Two different columns at once, e.g. writing `B` while reading `A` of the same entities.
    /// Panics if `A` and `B` are the same type.
    pub fn columns2_mut<A: Component, B: Component>(
        &mut self,
    ) -> (Option<ColumnMut<'_, A>>, Option<ColumnMut<'_, B>>) {
        let (a, b) = (TypeId::of::<A>(), TypeId::of::<B>());
        assert!(a != b, "columns2_mut needs two different component types");
        let (mut ca, mut cb) = (None, None);
        for (k, c) in &mut self.columns {
            if *k == a {
                ca = Some(c);
            } else if *k == b {
                cb = Some(c);
            }
        }
        (
            ca.map(|c| ColumnMut(c.as_any_mut().downcast_mut().expect("column type"))),
            cb.map(|c| ColumnMut(c.as_any_mut().downcast_mut().expect("column type"))),
        )
    }

    /// Entities with `T`, in ascending order.
    pub fn iter<T: Component>(&self) -> impl DoubleEndedIterator<Item = (Entity, &T)> {
        self.column::<T>().into_iter().flat_map(|c| c.iter())
    }

    /// Entities with both `A` and `B`, in ascending order.
    pub fn iter2<A: Component, B: Component>(&self) -> impl Iterator<Item = (Entity, &A, &B)> {
        let b = self.column::<B>();
        self.iter::<A>()
            .filter_map(move |(e, va)| Some((e, va, b?.get(e)?)))
    }

    /// Names of every component kind present (typed and opaque), sorted.
    pub fn component_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .columns
            .values()
            .filter(|c| c.len() > 0)
            .map(|c| c.name())
            .collect();
        names.extend(
            self.opaque
                .iter()
                .filter(|(_, c)| !c.is_empty())
                .map(|(n, _)| n.as_str()),
        );
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Whether any entity has a component of this name (typed or opaque).
    pub fn has_component_named(&self, name: &str) -> bool {
        self.columns.values().any(|c| c.name() == name && c.len() > 0)
            || self.opaque.get(name).is_some_and(|c| !c.is_empty())
    }

    /// Entities that have a component of this name (typed or opaque), ascending.
    pub fn entities_named(&self, name: &str) -> Vec<Entity> {
        let mut out: Vec<Entity> = self
            .columns
            .values()
            .filter(|c| c.name() == name)
            .flat_map(|c| c.entities())
            .collect();
        if let Some(col) = self.opaque.get(name) {
            out.extend(col.keys().copied());
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Every typed component's [`Component::verify`], and that each entity it refers to ([`Component::references`])
    /// is alive: (component name, entity, message) for each failure, by name then entity.
    pub fn verify_components(&self) -> Vec<(&'static str, Entity, String)> {
        let mut out: Vec<(&'static str, Entity, String)> = self
            .columns
            .values()
            .flat_map(|c| c.verify(self).into_iter().map(|(e, m)| (c.name(), e, m)))
            .collect();
        out.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        out
    }

    /// Every typed resource's [`Resource::verify`], and that each entity it refers to is alive: (resource name,
    /// message) for each failure, by name.
    pub fn verify_resources(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        let mut refs = Vec::new();
        for r in self.resources.values() {
            if let Err(m) = r.verify(self) {
                out.push((r.name(), m));
            }
            refs.clear();
            r.references(&mut refs);
            if let Some(e) = refs.iter().find(|e| !self.alive.contains(e)) {
                out.push((r.name(), format!("refers to {e:?}, which is not alive")));
            }
        }
        out.sort();
        out
    }

    /// Bulk payload bytes by component name, for the names that hold any: typed components'
    /// [`Component::payload_bytes`] and opaque byte payloads.
    pub fn payload_bytes(&self) -> BTreeMap<&str, u64> {
        let mut out: BTreeMap<&str, u64> = BTreeMap::new();
        for c in self.columns.values() {
            let n = c.payload_bytes();
            if n > 0 {
                *out.entry(c.name()).or_default() += n;
            }
        }
        for (name, col) in &self.opaque {
            let n: u64 = col
                .values()
                .map(|o| match &o.data {
                    OpaqueData::Bytes(b) => b.len() as u64,
                    OpaqueData::Json(_) => 0,
                })
                .sum();
            if n > 0 {
                *out.entry(name.as_str()).or_default() += n;
            }
        }
        out
    }

    /// Whether this entity has a component of this name (typed or opaque).
    pub fn entity_has_named(&self, e: Entity, name: &str) -> bool {
        self.columns.values().any(|c| c.name() == name && c.contains(e))
            || self.opaque.get(name).is_some_and(|c| c.contains_key(&e))
    }

    /// Every component of an entity as JSON, sorted by name: typed ones through [`Component::to_json`], opaque JSON
    /// ones as they are. Components without a JSON view (opaque bytes, typed ones that are not shown) are listed with
    /// `None`, so callers can count them.
    ///
    /// Fails, naming the component and the entity, when a view would lose a value (a non-finite float).
    pub fn components_json(&self, e: Entity) -> Result<Vec<ComponentJson<'_>>, ComponentJsonError> {
        let mut out = Vec::new();
        for c in self.columns.values() {
            if let Some(v) = c.json(e) {
                out.push(ComponentJson {
                    name: c.name(),
                    version: c.version(),
                    value: v.map_err(|err| ComponentJsonError {
                        component: c.name().to_owned(),
                        entity: Some(e),
                        message: err.0,
                    })?,
                });
            }
        }
        for (name, col) in &self.opaque {
            if let Some(o) = col.get(&e) {
                out.push(ComponentJson {
                    name: name.as_str(),
                    version: o.version,
                    value: opaque_json(o),
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(b.name));
        Ok(out)
    }

    /// Every resource (typed and opaque) as JSON, sorted by name; as [`World::components_json`].
    pub fn resources_json(&self) -> Result<Vec<ComponentJson<'_>>, ComponentJsonError> {
        let mut out = Vec::new();
        for r in self.resources.values() {
            out.push(ComponentJson {
                name: r.name(),
                version: r.version(),
                value: r.json().map_err(|err| ComponentJsonError {
                    component: r.name().to_owned(),
                    entity: None,
                    message: err.0,
                })?,
            });
        }
        for (name, o) in &self.opaque_resources {
            out.push(ComponentJson {
                name: name.as_str(),
                version: o.version,
                value: opaque_json(o),
            });
        }
        out.sort_by(|a, b| a.name.cmp(b.name));
        Ok(out)
    }

    /// Every component (typed and opaque) with its schema version, for debugging tools: `f(name, version, entity,
    /// value)`, by name then entity. Values are shown through `Debug`, which is not a format: its text may change
    /// between builds and toolchains, so nothing that is gated, cached or compared across builds may depend on it.
    pub fn visit_components_debug(&self, f: &mut dyn FnMut(&str, u32, Entity, &dyn fmt::Debug)) {
        enum Kind<'a> {
            Typed(&'a dyn AnyColumn),
            Opaque(&'a BTreeMap<Entity, Opaque>),
        }
        let mut kinds: Vec<(&str, Kind<'_>)> = self
            .columns
            .values()
            .filter(|c| c.len() > 0)
            .map(|c| (c.name(), Kind::Typed(c.as_ref())))
            .collect();
        kinds.extend(
            self.opaque
                .iter()
                .filter(|(_, c)| !c.is_empty())
                .map(|(n, c)| (n.as_str(), Kind::Opaque(c))),
        );
        // a name is typed or opaque, never both (each insert refuses the other kind's names)
        kinds.sort_unstable_by_key(|(n, _)| *n);
        for (name, kind) in kinds {
            match kind {
                Kind::Typed(c) => {
                    let version = c.version();
                    c.visit_debug(&mut |e, v| f(name, version, e, v));
                }
                Kind::Opaque(c) => {
                    for (e, o) in c {
                        f(name, o.version, *e, &o.data);
                    }
                }
            }
        }
    }

    /// Every resource (typed and opaque) with its schema version, by name, through `Debug`; as
    /// [`World::visit_components_debug`], for debugging tools only.
    pub fn visit_resources_debug(&self, f: &mut dyn FnMut(&str, u32, &dyn fmt::Debug)) {
        let mut all: Vec<(&str, u32, &dyn fmt::Debug)> = self
            .resources
            .values()
            .map(|r| (r.name(), r.version(), r.debug()))
            .collect();
        all.extend(
            self.opaque_resources
                .iter()
                .map(|(n, o)| (n.as_str(), o.version, &o.data as &dyn fmt::Debug)),
        );
        all.sort_unstable_by_key(|(n, _, _)| *n);
        for (name, version, value) in all {
            f(name, version, value);
        }
    }

    // ---- opaque components -------------------------------------------------------------------------------------

    /// Attach a component this build does not model. Panics if a typed component owns the name: a reader that
    /// knows the type must decode it.
    pub fn insert_opaque(&mut self, e: Entity, name: &str, value: Opaque) -> Option<Opaque> {
        assert!(self.alive.contains(&e), "insert {name} on dead entity {e:?}");
        assert!(
            !self.names.contains_key(name),
            "{name} is a typed component; decode it instead"
        );
        assert!(
            !self.is_resource_name(name),
            "{name} is a resource's name: a name is a component's or a resource's, not both"
        );
        self.opaque.entry(name.to_owned()).or_default().insert(e, value)
    }

    pub fn get_opaque(&self, e: Entity, name: &str) -> Option<&Opaque> {
        self.opaque.get(name)?.get(&e)
    }

    pub fn remove_opaque(&mut self, e: Entity, name: &str) -> Option<Opaque> {
        self.opaque.get_mut(name)?.remove(&e)
    }

    /// Every opaque component kind with its values, by name then entity.
    pub fn opaque_components(&self) -> impl Iterator<Item = (&str, &BTreeMap<Entity, Opaque>)> {
        self.opaque.iter().map(|(n, c)| (n.as_str(), c))
    }

    // ---- resources ---------------------------------------------------------------------------------------------

    /// Insert (or replace) a resource. Panics if a component has its name: a name is a component's or a resource's,
    /// not both, so pass declarations, which list both, stay unambiguous.
    pub fn insert_resource<T: Resource>(&mut self, value: T) -> Option<T> {
        assert!(
            !self.is_component_name(T::NAME),
            "{} is a component's name: a name is a component's or a resource's, not both",
            T::NAME
        );
        let old = self.resources.insert(TypeId::of::<T>(), Box::new(value))?;
        self.note_resource_write(T::NAME);
        let any: Box<dyn Any> = old;
        any.downcast::<T>().ok().map(|b| *b)
    }

    pub fn resource<T: Resource>(&self) -> Option<&T> {
        self.resources
            .get(&TypeId::of::<T>())
            .and_then(|r| r.as_any().downcast_ref())
    }

    /// Borrowed to write: a pass that does declares the name in [`crate::pass::Pass::modifies`].
    pub fn resource_mut<T: Resource>(&mut self) -> Option<&mut T> {
        if self.resources.contains_key(&TypeId::of::<T>()) {
            self.note_resource_write(T::NAME);
        }
        self.resources
            .get_mut(&TypeId::of::<T>())
            .and_then(|r| r.as_any_mut().downcast_mut())
    }

    /// Lend resource `T` read-only to `f` while `f` changes the rest of the world, then put it back as it was: a
    /// borrow, not a write, so the pass manager records none. `f` gets `None` when the world holds no `T`. Panics if
    /// `f` puts a `T` of its own into the world while one is lent: the one lent is the one that goes back.
    pub fn resource_scope<T: Resource, R>(&mut self, f: impl FnOnce(&mut World, Option<&T>) -> R) -> R {
        let held = self.resources.remove(&TypeId::of::<T>());
        let out = f(self, held.as_ref().and_then(|b| b.as_any().downcast_ref::<T>()));
        if let Some(b) = held {
            let replaced = self.resources.insert(TypeId::of::<T>(), b);
            assert!(
                replaced.is_none(),
                "{} was inserted while lent by resource_scope",
                T::NAME
            );
        }
        out
    }

    pub fn remove_resource<T: Resource>(&mut self) -> Option<T> {
        let old = self.resources.remove(&TypeId::of::<T>())?;
        if let Some(fx) = &mut self.effects {
            fx.resources_removed.insert(T::NAME.to_owned());
        }
        let any: Box<dyn Any> = old;
        any.downcast::<T>().ok().map(|b| *b)
    }

    /// As [`World::insert_resource`], for a resource this build does not model.
    pub fn insert_opaque_resource(&mut self, name: &str, value: Opaque) -> Option<Opaque> {
        assert!(
            !self.is_component_name(name),
            "{name} is a component's name: a name is a component's or a resource's, not both"
        );
        let old = self.opaque_resources.insert(name.to_owned(), value);
        if old.is_some() {
            self.note_resource_write(name);
        }
        old
    }

    /// Names of every resource (typed and opaque), sorted.
    pub fn resource_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .resources
            .values()
            .map(|r| r.name())
            .chain(self.opaque_resources.keys().map(String::as_str))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    fn is_resource_name(&self, name: &str) -> bool {
        self.resources.values().any(|r| r.name() == name) || self.opaque_resources.contains_key(name)
    }

    /// A component type or an opaque component has ever been stored under `name`.
    fn is_component_name(&self, name: &str) -> bool {
        self.names.contains_key(name) || self.opaque.contains_key(name)
    }

    pub fn opaque_resources(&self) -> impl Iterator<Item = (&str, &Opaque)> {
        self.opaque_resources.iter().map(|(n, v)| (n.as_str(), v))
    }
}

fn opaque_json(o: &Opaque) -> Option<serde_json::Value> {
    match &o.data {
        OpaqueData::Json(v) => Some(v.clone()),
        OpaqueData::Bytes(_) => None,
    }
}

/// Mutable access to one column's values without changing which entities it holds.
#[derive(Debug)]
pub struct ColumnMut<'w, T>(&'w mut Column<T>);

impl<T> ColumnMut<'_, T> {
    pub fn get(&self, e: Entity) -> Option<&T> {
        self.0.get(e)
    }
    pub fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        self.0.get_mut(e)
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (Entity, &T)> + ExactSizeIterator {
        self.0.iter()
    }
    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = (Entity, &mut T)> + ExactSizeIterator {
        self.0.iter_mut()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct A(u32);
    impl Component for A {
        const NAME: &'static str = "test.a";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
    }
    #[derive(Clone, Debug, PartialEq)]
    struct B(&'static str);
    impl Component for B {
        const NAME: &'static str = "test.b";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Same;
    impl Component for Same {
        const NAME: &'static str = "test.a";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
    }
    #[derive(Clone, Debug, PartialEq)]
    struct R(i32);
    impl Resource for R {
        const NAME: &'static str = "test.r";
    }

    #[test]
    fn iteration_is_in_entity_order_regardless_of_insert_order() {
        let mut w = World::new();
        let es: Vec<Entity> = (0..5).map(|_| w.spawn()).collect();
        for &e in es.iter().rev() {
            w.insert(e, A(e.index() * 10));
        }
        let got: Vec<u32> = w.iter::<A>().map(|(_, a)| a.0).collect();
        assert_eq!(got, [0, 10, 20, 30, 40]);
    }

    /// One value per vertex of each primitive: follows primitives and vertices.
    #[derive(Clone, Debug, PartialEq)]
    struct PerVertex(Vec<Vec<u8>>);
    impl Component for PerVertex {
        const NAME: &'static str = "test.per_vertex";
        const GEOMETRY: crate::Geometry = crate::Geometry::PerVertex;
        fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
            Some(remap_list(&self.0, from).map(PerVertex))
        }
        fn remap_vertices(&self, primitive: usize, from: &[u32]) -> Option<Result<Self, String>> {
            let mut out = self.0.clone();
            let Some(list) = self.0.get(primitive) else {
                return Some(Err(format!("primitive {primitive} of {}", self.0.len())));
            };
            out[primitive] = match from
                .iter()
                .map(|&i| {
                    list.get(i as usize)
                        .copied()
                        .ok_or(format!("vertex {i} of {}", list.len()))
                })
                .collect()
            {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            Some(Ok(PerVertex(out)))
        }
    }

    /// States it holds something per primitive (or per vertex) but does not answer an edit: what a component that
    /// forgot to follow its mesh looks like to the World.
    #[derive(Clone, Debug, PartialEq)]
    struct Unanswering(crate::Geometry);
    impl Component for Unanswering {
        const NAME: &'static str = "test.unanswering";
        const GEOMETRY: crate::Geometry = crate::Geometry::PerVertex;
        fn remap_primitives(&self, _from: &[usize]) -> Option<Result<Self, String>> {
            (self.0 != crate::Geometry::PerPrimitive).then(|| Ok(self.clone()))
        }
        fn remap_vertices(&self, _primitive: usize, _from: &[u32]) -> Option<Result<Self, String>> {
            None
        }
    }

    fn points(n: usize) -> crate::Primitive {
        crate::Primitive {
            topology: crate::Topology::Points,
            streams: [(crate::Semantic::Position, crate::Stream::F32x3(vec![[0.0; 3]; n]))].into(),
            indices: None,
            material: None,
        }
    }

    /// A component stating it describes the mesh must answer the edits it describes: one that doesn't is refused,
    /// naming it, and nothing changes. One stating `None` is kept as it is.
    #[test]
    fn edits_refuse_a_dependent_component_that_does_not_answer() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, crate::Mesh::new(vec![points(2), points(2)]));
        w.insert(e, A(7));
        // per primitive: a rebuild of the list is refused
        w.insert(e, Unanswering(crate::Geometry::PerPrimitive));
        let err = w
            .rebuild_primitives(e, |m| Ok((vec![m.primitives()[1].clone()], vec![1])))
            .unwrap_err();
        assert!(
            err.contains("test.unanswering") && err.contains("per primitive"),
            "{err}"
        );
        assert_eq!(
            w.get::<crate::Mesh>(e).unwrap().primitives().len(),
            2,
            "nothing changed"
        );
        // per vertex: it answers the rebuild, but a compaction is refused
        w.insert(e, Unanswering(crate::Geometry::PerVertex));
        let err = w.remap_vertices(e, 0, &[1]).unwrap_err();
        assert!(
            err.contains("test.unanswering") && err.contains("per vertex"),
            "{err}"
        );
        // a component of Geometry::None follows nothing and is never refused
        w.remove::<Unanswering>(e);
        w.rebuild_primitives(e, |m| Ok((vec![m.primitives()[1].clone()], vec![1])))
            .unwrap();
        w.remap_vertices(e, 0, &[1]).unwrap();
        assert_eq!(w.get::<A>(e), Some(&A(7)));
    }

    /// Moving one form onto another can't leave data describing one mesh beside another: `from` bringing a mesh
    /// while `to` keeps per-primitive data `from` lacks, or `from` bringing such data without a mesh onto `to`'s mesh,
    /// is refused, changing nothing. With `to`'s mesh taken away first it moves.
    #[test]
    fn a_move_leaving_stale_mesh_data_is_refused() {
        use crate::{MaterialVariants, Mesh};
        let mut w = World::new();
        let (to, from) = (w.spawn(), w.spawn());
        w.insert(to, Mesh::new(vec![points(1)]));
        w.insert(to, MaterialVariants(vec![Default::default()]));
        w.insert(from, Mesh::new(vec![points(3), points(3)]));
        let err = w.move_components(from, to).unwrap_err();
        assert!(err.contains("core.material_variants"), "{err}");
        assert!(w.is_alive(from) && w.get::<Mesh>(to).unwrap().primitives().len() == 1);
        // data without a mesh onto a mesh
        let bare = w.spawn();
        w.insert(bare, MaterialVariants(vec![]));
        let err = w.move_components(bare, to).unwrap_err();
        assert!(err.contains("core.material_variants"), "{err}");
        assert!(w.is_alive(bare));
        // the old geometry taken away first: the move goes through
        w.remove_mesh(to).unwrap();
        assert!(w.move_components(from, to).unwrap());
        assert_eq!(w.get::<Mesh>(to).unwrap().primitives().len(), 2);
    }

    /// A primitive's vertices compacted: a per-vertex component follows them (after a rebuild that kept only
    /// primitive 1, from its source primitive's values); one that holds nothing per vertex is kept; an index the
    /// component lacks fails naming it, and an opaque component refuses, each changing nothing.
    #[test]
    fn per_vertex_components_follow_a_compaction() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, PerVertex(vec![vec![10, 11], vec![20, 21, 22, 23]]));
        w.insert(e, A(7));
        // keep primitive 1 only, then its vertices 3 and 1
        w.remap_primitives(e, &[1]).unwrap();
        w.remap_vertices(e, 0, &[3, 1]).unwrap();
        assert_eq!(w.get::<PerVertex>(e), Some(&PerVertex(vec![vec![23, 21]])));
        assert_eq!(w.get::<A>(e), Some(&A(7)));
        let err = w.remap_vertices(e, 0, &[5]).unwrap_err();
        assert!(
            err.contains("test.per_vertex") && err.contains("vertex 5 of 2"),
            "{err}"
        );
        assert_eq!(w.get::<PerVertex>(e), Some(&PerVertex(vec![vec![23, 21]])));
        w.insert_opaque(
            e,
            "game.thing",
            Opaque {
                version: 1,
                data: OpaqueData::Bytes(vec![1]),
            },
        );
        assert!(w.remap_vertices(e, 0, &[1]).unwrap_err().contains("game.thing"));
        assert_eq!(w.get::<PerVertex>(e), Some(&PerVertex(vec![vec![23, 21]])));
    }

    /// `check_remap_vertices` states what `remap_vertices` would do and changes nothing.
    #[test]
    fn the_check_of_a_vertex_remap_changes_nothing() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, PerVertex(vec![vec![10, 11]]));
        w.check_remap_vertices(e, 0, &[1, 0, 1]).unwrap();
        assert_eq!(w.get::<PerVertex>(e), Some(&PerVertex(vec![vec![10, 11]])));
        let err = w.check_remap_vertices(e, 0, &[2]).unwrap_err();
        assert!(err.contains("vertex 2 of 2"), "{err}");
        w.insert(e, Unanswering(crate::Geometry::PerVertex));
        let err = w.check_remap_vertices(e, 0, &[0]).unwrap_err();
        assert!(err.contains("test.unanswering"), "{err}");
    }

    /// A mesh taken away goes with what follows its primitives; with an opaque component (which can't be known to
    /// describe primitives or not) nothing changes.
    #[test]
    fn remove_mesh_takes_its_per_primitive_components_and_refuses_with_opaque_data() {
        use crate::{MaterialVariants, Mesh, MeshFile};
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Mesh::new(Vec::new()));
        w.insert(e, MaterialVariants(Vec::new()));
        w.insert(
            e,
            MeshFile {
                path: "a.glb".into(),
                mesh: 0,
                materials: Vec::new(),
            },
        );
        w.insert(e, A(1));
        w.insert_opaque(
            e,
            "game.thing",
            Opaque {
                version: 1,
                data: OpaqueData::Bytes(vec![1]),
            },
        );
        assert!(w.remove_mesh(e).unwrap_err().contains("game.thing"));
        assert!(w.has::<Mesh>(e) && w.has::<MaterialVariants>(e));
        w.remove_opaque(e, "game.thing");
        assert_eq!(
            w.remove_mesh(e).unwrap(),
            ["core.mesh", "core.material_variants", "core.mesh_file"]
        );
        // nothing left to restore the mesh from
        assert!(!w.has::<Mesh>(e) && !w.has::<MaterialVariants>(e) && !w.has::<MeshFile>(e));
        // what does not follow primitives stays
        assert!(w.has::<A>(e));
        assert!(w.remove_mesh(e).unwrap().is_empty());
        // a mesh still only in its file goes the same way, with the per-primitive data that describes it
        w.insert(e, MaterialVariants(Vec::new()));
        w.insert(
            e,
            MeshFile {
                path: "a.glb".into(),
                mesh: 0,
                materials: Vec::new(),
            },
        );
        assert_eq!(
            w.remove_mesh(e).unwrap(),
            ["core.material_variants", "core.mesh_file"]
        );
        assert!(!w.has::<MaterialVariants>(e) && !w.has::<MeshFile>(e) && w.has::<A>(e));
    }

    #[test]
    fn iter2_joins_and_despawn_removes_everything() {
        let mut w = World::new();
        let e0 = w.spawn();
        let e1 = w.spawn();
        w.insert(e0, A(1));
        w.insert(e1, A(2));
        w.insert(e1, B("x"));
        w.insert_opaque(
            e1,
            "game.thing",
            Opaque {
                version: 3,
                data: OpaqueData::Bytes(vec![1, 2]),
            },
        );
        assert_eq!(w.iter2::<A, B>().map(|(e, ..)| e).collect::<Vec<_>>(), [e1]);
        assert!(w.entity_has_named(e1, "game.thing"));
        assert_eq!(w.component_names(), ["game.thing", "test.a", "test.b"]);
        assert!(w.despawn(e1));
        assert!(!w.has::<A>(e1) && w.get_opaque(e1, "game.thing").is_none());
        assert_eq!(w.component_names(), ["test.a"]);
        let e2 = w.spawn();
        assert_ne!(e2, e1, "ids are never reused");
    }

    #[test]
    fn columns2_mut_reads_one_writes_another() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, A(7));
        w.insert(e, B("a"));
        let (a, b) = w.columns2_mut::<A, B>();
        let (a, mut b) = (a.unwrap(), b.unwrap());
        if a.get(e).unwrap().0 == 7 {
            *b.get_mut(e).unwrap() = B("seven");
        }
        assert_eq!(w.get::<B>(e), Some(&B("seven")));
    }

    #[test]
    fn move_components_replaces_by_name_and_keeps_the_rest() {
        let mut w = World::new();
        let from = w.spawn();
        let to = w.spawn();
        w.insert(from, A(9));
        w.insert(to, A(1));
        w.insert(to, B("kept"));
        let opaque = Opaque {
            version: 4,
            data: OpaqueData::Bytes(vec![3, 2, 1]),
        };
        w.insert_opaque(from, "game.form", opaque.clone());
        assert!(!w.move_components(from, from).unwrap());
        assert!(!w.move_components(from, Entity::from_index(999)).unwrap());
        assert_eq!(w.get::<A>(from), Some(&A(9)), "a refused move changes nothing");
        assert!(w.move_components(from, to).unwrap());
        assert!(!w.is_alive(from));
        assert_eq!(w.get::<A>(to), Some(&A(9)));
        assert_eq!(w.get::<B>(to), Some(&B("kept")));
        assert_eq!(w.get_opaque(to, "game.form"), Some(&opaque));
        assert!(!w.move_components(from, to).unwrap());
        assert_ne!(w.spawn(), from, "the moved-from id is not reused");
    }

    #[derive(Clone, Debug, PartialEq)]
    struct NamedLikeA;
    impl Resource for NamedLikeA {
        const NAME: &'static str = "test.a";
    }

    #[test]
    #[should_panic(expected = "a component's or a resource's, not both")]
    fn a_resource_cannot_take_a_component_name() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, A(1));
        w.insert_resource(NamedLikeA);
    }

    #[test]
    #[should_panic(expected = "a component's or a resource's, not both")]
    fn a_component_cannot_take_a_resource_name() {
        let mut w = World::new();
        w.insert_resource(NamedLikeA);
        let e = w.spawn();
        w.insert(e, A(1));
    }

    #[test]
    fn resource_names_list_typed_and_opaque_sorted() {
        let mut w = World::new();
        w.insert_resource(R(1));
        w.insert_opaque_resource(
            "game.settings",
            Opaque {
                version: 1,
                data: OpaqueData::Bytes(vec![]),
            },
        );
        assert_eq!(w.resource_names(), ["game.settings", "test.r"]);
    }

    #[test]
    fn copy_components_replaces_by_name_keeps_the_rest_and_the_source() {
        let mut w = World::new();
        let from = w.spawn();
        let to = w.spawn();
        w.insert(from, A(9));
        w.insert(to, A(1));
        w.insert(to, B("kept"));
        let opaque = Opaque {
            version: 4,
            data: OpaqueData::Bytes(vec![3, 2, 1]),
        };
        w.insert_opaque(from, "game.form", opaque.clone());
        assert!(!w.copy_components(from, from));
        assert!(!w.copy_components(from, Entity::from_index(999)));
        assert!(w.copy_components(from, to));
        assert!(w.is_alive(from));
        assert_eq!(w.get::<A>(from), Some(&A(9)));
        assert_eq!(w.get_opaque(from, "game.form"), Some(&opaque));
        assert_eq!(w.get::<A>(to), Some(&A(9)));
        assert_eq!(w.get::<B>(to), Some(&B("kept")));
        assert_eq!(w.get_opaque(to, "game.form"), Some(&opaque));
    }

    #[test]
    #[should_panic(expected = "share the name")]
    fn two_types_cannot_share_a_name() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, A(1));
        w.insert(e, Same);
    }

    #[derive(Clone, Debug, PartialEq, serde::Serialize)]
    struct Shown {
        x: u32,
        of: Entity,
    }
    impl Component for Shown {
        const NAME: &'static str = "test.shown";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
        const VERSION: u32 = 2;
        fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
            crate::json::to_value(self).map(Some)
        }
    }

    #[test]
    fn components_json_lists_every_component_sorted() {
        let mut w = World::new();
        let e0 = w.spawn();
        let e = w.spawn();
        w.insert(e, Shown { x: 3, of: e0 });
        w.insert(e, A(1));
        w.insert_opaque(
            e,
            "game.bytes",
            Opaque {
                version: 1,
                data: OpaqueData::Bytes(vec![9]),
            },
        );
        w.insert_opaque(
            e,
            "game.json",
            Opaque {
                version: 1,
                data: OpaqueData::Json(serde_json::json!({"k": 1})),
            },
        );
        let got: Vec<(&str, u32, Option<serde_json::Value>)> = w
            .components_json(e)
            .unwrap()
            .into_iter()
            .map(|c| (c.name, c.version, c.value))
            .collect();
        assert_eq!(
            got,
            [
                ("game.bytes", 1, None),
                ("game.json", 1, Some(serde_json::json!({"k": 1}))),
                ("test.a", 1, None),
                ("test.shown", 2, Some(serde_json::json!({"x": 3, "of": 0}))),
            ]
        );
        assert!(w.components_json(e0).unwrap().is_empty());
    }

    #[derive(Clone, Debug, PartialEq, serde::Serialize)]
    struct F(f32);
    impl Component for F {
        const NAME: &'static str = "test.f";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
        fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
            crate::json::to_value(self).map(Some)
        }
    }
    impl Resource for F {
        const NAME: &'static str = "test.res";
        const VERSION: u32 = 4;
        fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
            crate::json::to_value(self).map(Some)
        }
    }

    #[test]
    fn a_json_view_that_would_lose_a_value_is_an_error() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, F(f32::NAN));
        assert_eq!(
            w.components_json(e).unwrap_err().to_string(),
            "test.f on e0: NaN at the value: JSON has no non-finite numbers"
        );
        w.insert_resource(F(0.5));
        let r = w.resources_json().unwrap();
        assert_eq!(
            (r[0].name, r[0].version, r[0].value.clone()),
            ("test.res", 4, Some(serde_json::json!(0.5)))
        );
        w.insert_resource(F(f32::INFINITY));
        assert_eq!(
            w.resources_json().unwrap_err().to_string(),
            "test.res: inf at the value: JSON has no non-finite numbers"
        );
    }

    #[derive(Clone, Debug, PartialEq)]
    struct Points(Entity);
    impl Component for Points {
        const NAME: &'static str = "test.points";
        const GEOMETRY: crate::Geometry = crate::Geometry::None;
        fn references(&self, out: &mut Vec<Entity>) {
            out.push(self.0);
        }
    }

    #[test]
    fn a_reference_to_a_despawned_entity_fails_verification() {
        let mut w = World::new();
        let target = w.spawn();
        let holder = w.spawn();
        w.insert(holder, Points(target));
        assert!(w.verify_components().is_empty());
        w.despawn(target);
        assert_eq!(
            w.verify_components(),
            [(
                "test.points",
                holder,
                format!("refers to {target:?}, which is not alive")
            )]
        );
    }

    #[test]
    #[should_panic(expected = "opaque component")]
    fn a_typed_component_cannot_take_an_opaque_name() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert_opaque(
            e,
            "test.a",
            Opaque {
                version: 1,
                data: OpaqueData::Bytes(vec![]),
            },
        );
        w.insert(e, A(1));
    }

    #[test]
    fn resources_and_clone() {
        let mut w = World::new();
        assert!(w.insert_resource(R(1)).is_none());
        assert_eq!(w.insert_resource(R(2)), Some(R(1)));
        let e = w.spawn();
        w.insert(e, A(5));
        let w2 = w.clone();
        w.get_mut::<A>(e).unwrap().0 = 6;
        assert_eq!(w2.get::<A>(e), Some(&A(5)));
        assert_eq!(w2.resource::<R>(), Some(&R(2)));
    }
}
