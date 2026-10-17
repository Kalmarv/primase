//! The world store: entities, typed components, opaque (unknown) components and world resources.
//!
//! Deliberately small and deterministic. Every column keeps its entities in ascending order, so iteration order is a
//! function of the world's contents, never of hashing or insertion timing; writers that walk the world produce the
//! same bytes for the same world.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

    /// The component as JSON, for glTF extras, the debug text form and golden tests; `Ok(None)` for components
    /// that are not shown (bulk data, handles). The view must never lose a value: components that can be shown
    /// derive `serde::Serialize` and return `crate::json::to_value(self).map(Some)`, which refuses non-finite
    /// floats instead of writing them as `null`.
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        Ok(None)
    }

    /// The component's own invariants on entity `e` (indices in range, entity references alive, stream lengths
    /// equal, ...). The pass manager runs it on every component after the reader, after every pass and before the
    /// writer. World-wide rules (one name -> one payload) are `pass::Validator`s instead.
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        Ok(())
    }

    /// The entities this component refers to (a prototype, materials, textures), appended to `out`: the edges
    /// [`World::closure`] follows. A component holding entity references must declare every one of them.
    fn references(&self, _out: &mut Vec<Entity>) {}
}

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
    /// `None` when the entity has no such component; else its JSON view.
    fn json(&self, e: Entity) -> Option<Result<Option<serde_json::Value>, JsonError>>;
    /// Every value's `Component::verify`, as (entity, message) for those that fail, in entity order.
    fn verify(&self, w: &World) -> Vec<(Entity, String)>;
    /// Every value through `Debug`, in entity order.
    fn visit_debug(&self, f: &mut dyn FnMut(Entity, &dyn fmt::Debug));
    fn entities(&self) -> Vec<Entity>;
    /// [`Component::references`] of entity `e`'s value, if it has one.
    fn references(&self, e: Entity, out: &mut Vec<Entity>);
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
    fn json(&self, e: Entity) -> Option<Result<Option<serde_json::Value>, JsonError>> {
        self.values.get(&e).map(Component::to_json)
    }
    fn verify(&self, w: &World) -> Vec<(Entity, String)> {
        self.values
            .iter()
            .filter_map(|(e, v)| v.verify(*e, w).err().map(|m| (*e, m)))
            .collect()
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
    columns: HashMap<TypeId, Box<dyn AnyColumn>>,
    /// Component name -> owning type, to refuse two Rust types claiming one name.
    names: BTreeMap<&'static str, TypeId>,
    opaque: BTreeMap<String, BTreeMap<Entity, Opaque>>,
    resources: HashMap<TypeId, Box<dyn AnyResource>>,
    opaque_resources: BTreeMap<String, Opaque>,
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
    fn resources_for(&self, alive: &BTreeSet<Entity>) -> HashMap<TypeId, Box<dyn AnyResource>> {
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
    /// its components (`select-lod`). False, and nothing changed, unless both are alive and differ.
    pub fn move_components(&mut self, from: Entity, to: Entity) -> bool {
        if from == to || !self.alive.contains(&from) || !self.alive.contains(&to) {
            return false;
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
        true
    }

    pub fn is_alive(&self, e: Entity) -> bool {
        self.alive.contains(&e)
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
        let [ca, cb] = self.columns.get_disjoint_mut([&a, &b]);
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

    /// Every typed component's [`Component::verify`]: (component name, entity, message) for each failure, by name
    /// then entity.
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
        // a name is typed or opaque, never both (insert_opaque refuses typed names)
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

    pub fn insert_resource<T: Resource>(&mut self, value: T) -> Option<T> {
        let old = self.resources.insert(TypeId::of::<T>(), Box::new(value))?;
        let any: Box<dyn Any> = old;
        any.downcast::<T>().ok().map(|b| *b)
    }

    pub fn resource<T: Resource>(&self) -> Option<&T> {
        self.resources
            .get(&TypeId::of::<T>())
            .and_then(|r| r.as_any().downcast_ref())
    }

    pub fn resource_mut<T: Resource>(&mut self) -> Option<&mut T> {
        self.resources
            .get_mut(&TypeId::of::<T>())
            .and_then(|r| r.as_any_mut().downcast_mut())
    }

    pub fn remove_resource<T: Resource>(&mut self) -> Option<T> {
        let old = self.resources.remove(&TypeId::of::<T>())?;
        let any: Box<dyn Any> = old;
        any.downcast::<T>().ok().map(|b| *b)
    }

    pub fn insert_opaque_resource(&mut self, name: &str, value: Opaque) -> Option<Opaque> {
        self.opaque_resources.insert(name.to_owned(), value)
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
    }
    #[derive(Clone, Debug, PartialEq)]
    struct B(&'static str);
    impl Component for B {
        const NAME: &'static str = "test.b";
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Same;
    impl Component for Same {
        const NAME: &'static str = "test.a";
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
        assert!(!w.move_components(from, from));
        assert!(!w.move_components(from, Entity::from_index(999)));
        assert_eq!(w.get::<A>(from), Some(&A(9)), "a refused move changes nothing");
        assert!(w.move_components(from, to));
        assert!(!w.is_alive(from));
        assert_eq!(w.get::<A>(to), Some(&A(9)));
        assert_eq!(w.get::<B>(to), Some(&B("kept")));
        assert_eq!(w.get_opaque(to, "game.form"), Some(&opaque));
        assert!(!w.move_components(from, to));
        assert_ne!(w.spawn(), from, "the moved-from id is not reused");
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
