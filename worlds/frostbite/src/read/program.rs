//! A material's program read through to its compiled bytecode: a program database ([`FrostbitePrograms`]) names, per
//! material key, program indices; a program record names permutation ids; a permutation names CompiledBytecode
//! resources by GUID ([`crate::permutation`]). The game resolves ids and names in every bundle loaded: a bundle with a
//! database holds the permutations it adds and names the stock ones, which lie in another bundle (the game's
//! configuration bundle in `globals.toc`, a level's root). A [`Scope`] is the bundles an id or a name is looked up in,
//! the earlier first; one that none holds is an error naming it, not a skipped program. [`Materials`] is what a
//! scope's depots, program databases and texture lookups hold by material key, and [`Source::resolve_materials`]
//! resolves a MeshSet's section keys through it.

use std::collections::HashMap;

use primase_core::shader::Shader;
use primase_core::{Entity, Name, World};

use super::entry::spawn_entity;
use super::folder::Source;
use super::listing::{Bundle, Listed};
use crate::components::{
    FrostbiteBindings, FrostbiteBytecode, FrostbiteInputLayout, FrostbiteMaterial, FrostbiteMeshSet,
    FrostbitePermutation, FrostbiteProgram, FrostbitePrograms, MaterialDepot,
};
use crate::inputlayout::{self, InputLayout};
use crate::permutation::{self, Permutation};
use crate::{Error, Result, binding, bytecode, depot, lookup};

/// Bundles that ids and names are looked up in, with the index of what they hold built once: the permutations by type
/// and id, the CompiledBytecode resources by name. Where two bundles hold the same id or name, the earlier wins.
#[derive(Debug)]
pub struct Scope<'a> {
    bundles: &'a [Bundle],
    /// (type, id) -> (bundle, resource index in its manifest).
    permutations: HashMap<(u32, u64), (usize, usize)>,
    bytecode: HashMap<&'a str, (usize, usize)>,
}

impl<'a> Scope<'a> {
    pub fn new(bundles: &'a [Bundle]) -> Self {
        let (mut permutations, mut bytecode) = (HashMap::new(), HashMap::new());
        for (b, bundle) in bundles.iter().enumerate() {
            for (i, a) in bundle.manifest.res.iter().enumerate() {
                match a.rtype {
                    permutation::RASTER
                    | permutation::COMPUTE
                    | permutation::SHARED_RASTER
                    | permutation::SHARED_COMPUTE
                    | binding::BINDING_SET
                    | binding::PASS_BINDING
                    | crate::inputlayout::RTYPE
                    | lookup::STREAMABLE_TEXTURE_SET => {
                        permutations
                            .entry((a.rtype, permutation::resource_id(&a.rmeta)))
                            .or_insert((b, i));
                    }
                    crate::bytecode::RTYPE => {
                        bytecode.entry(a.name.as_str()).or_insert((b, i));
                    }
                    _ => {}
                }
            }
        }
        Scope {
            bundles,
            permutations,
            bytecode,
        }
    }

    /// The bundles, in lookup order.
    pub fn bundles(&self) -> &'a [Bundle] {
        self.bundles
    }

    /// The entry of the resource of type `rtype` with `id` in the first bundle of the scope that holds one (a
    /// permutation or a streamable texture set).
    pub fn resource_by_id(&self, rtype: u32, id: u64) -> Option<Listed> {
        let &(b, i) = self.permutations.get(&(rtype, id))?;
        Some(self.bundles[b].resource(i))
    }

    /// The bundle and entry of the resource of type `rtype` with `id`.
    pub(super) fn resource_with_home(&self, rtype: u32, id: u64) -> Option<(&'a Bundle, Listed)> {
        let &(b, i) = self.permutations.get(&(rtype, id))?;
        Some((&self.bundles[b], self.bundles[b].resource(i)))
    }

    /// The bundle and entry of the permutation with `id`, a raster one first.
    pub fn permutation(&self, id: u64) -> Option<(&'a Bundle, Listed)> {
        let &(b, i) = [permutation::RASTER, permutation::COMPUTE]
            .iter()
            .find_map(|t| self.permutations.get(&(*t, id)))?;
        Some((&self.bundles[b], self.bundles[b].resource(i)))
    }

    /// The bundle and entry of the CompiledBytecode resource named `name`.
    pub fn bytecode(&self, name: &str) -> Option<(&'a Bundle, Listed)> {
        let &(b, i) = self.bytecode.get(name)?;
        Some((&self.bundles[b], self.bundles[b].resource(i)))
    }

    /// The bundle `entry` was listed from, among these.
    fn bundle_of(&self, entry: &Listed) -> Option<&'a Bundle> {
        self.bundles
            .iter()
            .find(|b| b.toc_path == entry.toc && b.name == entry.bundle)
    }
}

/// A permutation and the programs it names, read and not yet in a world.
pub(super) struct PermutationRead {
    entry: Listed,
    permutation: Permutation,
    bindings: FrostbiteBindings,
    layout: Option<InputLayout>,
    /// Each program's CompiledBytecode entry and its shader.
    programs: Vec<(Listed, Shader, FrostbiteBytecode)>,
}

impl PermutationRead {
    /// The permutation's entity, added after the shader entities of its programs, which it names.
    fn spawn(self, w: &mut World) -> Entity {
        let shaders: Vec<Entity> = (self.programs.into_iter())
            .map(|(listed, shader, c)| {
                let e = spawn_entity(w, &listed, listed.name.clone());
                w.insert(e, shader);
                w.insert(e, c);
                e
            })
            .collect();
        let e = spawn_entity(w, &self.entry, self.entry.name.clone());
        w.insert(
            e,
            FrostbitePermutation {
                permutation: self.permutation,
                shaders,
            },
        );
        w.insert(e, self.bindings);
        if let Some(l) = self.layout {
            w.insert(e, FrostbiteInputLayout(l));
        }
        e
    }
}

impl Source<'_> {
    /// The permutation of `entry` (a resource of type [`permutation::RASTER`] or [`permutation::COMPUTE`] of
    /// `bundle`) with the programs it names, looked up in `scope`.
    pub(super) fn read_permutation(
        &self,
        scope: &Scope<'_>,
        bundle: &Bundle,
        entry: &Listed,
    ) -> Result<PermutationRead> {
        let meta = bundle.manifest.res[entry.index].rmeta;
        let payload = self.payload(entry.file, entry.size as usize)?;
        let permutation = permutation::read(entry.resource_type.unwrap_or(0), &payload, &meta)
            .map_err(|e| Error::Unsupported(format!("{} ({}): {e}", entry.name, entry.bundle)))?;
        let mut programs = Vec::with_capacity(permutation.bytecode.len());
        for guid in &permutation.bytecode {
            let name = permutation::bytecode_name(guid);
            let (_, listed) = scope.bytecode(&name).ok_or_else(|| {
                Error::Missing(format!("{}: {name} is in no bundle of the scope", entry.name))
            })?;
            let mut one = bytecode::read(&self.payload(listed.file, listed.size as usize)?)
                .map_err(|e| Error::Unsupported(format!("{name}: {e}")))?;
            if one.len() != 1 {
                return Err(Error::Unsupported(format!(
                    "{name}: {} containers in a permutation's program",
                    one.len()
                )));
            }
            let (shader, c) = one.remove(0);
            programs.push((listed, shader, c));
        }
        let bindings = self.read_bindings(scope, &permutation, entry)?;
        let layout = permutation
            .input_layout
            .map(|id| self.read_input_layout(scope, id, entry))
            .transpose()?;
        Ok(PermutationRead {
            entry: entry.clone(),
            permutation,
            bindings,
            layout,
            programs,
        })
    }

    /// The input layout `id` of a raster permutation, looked up in `scope`.
    fn read_input_layout(&self, scope: &Scope<'_>, id: u64, entry: &Listed) -> Result<InputLayout> {
        let (home, e) = scope.resource_with_home(inputlayout::RTYPE, id).ok_or_else(|| {
            Error::Missing(format!(
                "{}: input layout {id} is in no bundle of the scope",
                entry.name
            ))
        })?;
        let payload = self.payload(e.file, e.size as usize)?;
        inputlayout::read(&payload, &home.manifest.res[e.index].rmeta)
            .map_err(|err| Error::Unsupported(format!("{}: {err}", entry.name)))
    }

    /// The binding sets of `permutation`: its shared data, then the pass binding and sets the data names, looked up in
    /// `scope`. One the scope does not hold is an error naming it.
    fn read_bindings(
        &self,
        scope: &Scope<'_>,
        permutation: &Permutation,
        entry: &Listed,
    ) -> Result<FrostbiteBindings> {
        let missing = |what: &str, id: u64| {
            Error::Missing(format!(
                "{}: {what} {id} is in no bundle of the scope",
                entry.name
            ))
        };
        let unsupported = |e: Error| Error::Unsupported(format!("{}: {e}", entry.name));
        let (home, shared_entry) = [permutation::SHARED_RASTER, permutation::SHARED_COMPUTE]
            .iter()
            .find_map(|&t| scope.resource_with_home(t, permutation.shared_data))
            .ok_or_else(|| missing("shared data", permutation.shared_data))?;
        let payload = self.payload(shared_entry.file, shared_entry.size as usize)?;
        let shared = permutation::read_shared(
            shared_entry.resource_type.unwrap_or(0),
            &payload,
            &home.manifest.res[shared_entry.index].rmeta,
        )
        .map_err(unsupported)?;
        let read_set = |rtype: u32, id: u64| -> Result<binding::Set> {
            let (home, e) = scope
                .resource_with_home(rtype, id)
                .ok_or_else(|| missing("binding set", id))?;
            let payload = self.payload(e.file, e.size as usize)?;
            binding::read(rtype, &payload, &home.manifest.res[e.index].rmeta).map_err(unsupported)
        };
        let pass = shared
            .pass_binding
            .map(|id| read_set(binding::PASS_BINDING, id))
            .transpose()?;
        let mut sets = [None, None, None];
        for (slot, id) in sets.iter_mut().zip(shared.sets) {
            *slot = id.map(|id| read_set(binding::BINDING_SET, id)).transpose()?;
        }
        Ok(FrostbiteBindings { shared, pass, sets })
    }

    /// A permutation entry as its entities: its own, then those of the programs it names (looked up in the bundle
    /// alone: a permutation read by itself names programs the bundle holds).
    pub(super) fn spawn_permutation(
        &self,
        bundle: &Bundle,
        entry: &Listed,
        w: &mut World,
    ) -> Result<Vec<Entity>> {
        let alone = [bundle.clone()];
        let e = self
            .read_permutation(&Scope::new(&alone), bundle, entry)?
            .spawn(w);
        let mut out = vec![e];
        if let Some(p) = w.get::<FrostbitePermutation>(e) {
            out.extend(&p.shaders);
        }
        Ok(out)
    }

    /// A program database entry (an ExpressionShaderDatabase resource of `bundle`): its records and rows.
    pub(super) fn read_database(&self, bundle: &Bundle, entry: &Listed) -> Result<FrostbitePrograms> {
        let meta = bundle.manifest.res[entry.index].rmeta;
        let payload = self.payload(entry.file, entry.size as usize)?;
        let unsupported = |e: Error| Error::Unsupported(format!("{} ({}): {e}", entry.name, entry.bundle));
        Ok(FrostbitePrograms {
            records: lookup::read_records(&payload, &meta).map_err(unsupported)?,
            lookup: lookup::read_programs(&payload, &meta).map_err(unsupported)?,
        })
    }

    /// The programs a material key reaches in the program database `database` (an ExpressionShaderDatabase entry of a
    /// bundle of `scope`): a `frostbite.program` entity for each program index of the key's row, naming its
    /// permutations (`frostbite.permutation`), which name their programs' `core.shader` entities. Everything is read
    /// before anything is added to `w`; the row, a program index, a permutation or a program that the database or
    /// the scope does not hold is an error naming it.
    pub fn read_material_program(
        &self,
        scope: &Scope<'_>,
        database: &Listed,
        material: u64,
        w: &mut World,
    ) -> Result<Vec<Entity>> {
        if database.resource_type != Some(lookup::PROGRAM_LOOKUP) {
            return Err(Error::Unsupported(format!(
                "{}: not a program database",
                database.name
            )));
        }
        let home = scope.bundle_of(database).ok_or_else(|| {
            Error::Missing(format!(
                "{}: its bundle {} is not in the scope",
                database.name, database.bundle
            ))
        })?;
        let db = self.read_database(home, database)?;
        self.spawn_programs(scope, database, &db, material, w)
    }

    /// The programs of `material`'s row of `db` (read from `database`), as [`Source::read_material_program`].
    fn spawn_programs(
        &self,
        scope: &Scope<'_>,
        database: &Listed,
        db: &FrostbitePrograms,
        material: u64,
        w: &mut World,
    ) -> Result<Vec<Entity>> {
        let row = db.lookup.rows.get(&material).ok_or_else(|| {
            Error::Missing(format!(
                "{}: no program row for material key {material:#018x}",
                database.name
            ))
        })?;
        let mut programs = Vec::with_capacity(row.len());
        for &index in row {
            let record = db.records.get(index as usize).ok_or_else(|| {
                Error::Missing(format!(
                    "{}: program {index} of {} records",
                    database.name,
                    db.records.len()
                ))
            })?;
            let mut permutations = Vec::with_capacity(record.permutations.len());
            for &id in &record.permutations {
                let (bundle, entry) = scope.permutation(id).ok_or_else(|| {
                    Error::Missing(format!(
                        "{}: program {index}: permutation {id} is in no bundle of the scope",
                        database.name
                    ))
                })?;
                permutations.push(self.read_permutation(scope, bundle, &entry)?);
            }
            programs.push((index, record.key, record.raw, permutations));
        }
        Ok(programs
            .into_iter()
            .map(|(index, record_key, record, permutations)| {
                let permutations: Vec<Entity> = permutations.into_iter().map(|p| p.spawn(w)).collect();
                let e = spawn_entity(w, database, format!("{}#{index}", database.name));
                w.insert(
                    e,
                    FrostbiteProgram {
                        material,
                        index,
                        record_key,
                        record,
                        permutations,
                    },
                );
                e
            })
            .collect())
    }

    /// What the scope's depots, program databases and texture lookups hold by material key. Reads each depot, database
    /// and lookup of the scope's bundles once; where two hold a key the earlier bundle's wins.
    pub fn materials(&self, scope: &Scope<'_>) -> Result<Materials> {
        let mut m = Materials::default();
        for bundle in scope.bundles() {
            for (i, a) in bundle.manifest.res.iter().enumerate() {
                let entry = match a.rtype {
                    depot::RTYPE | lookup::PROGRAM_LOOKUP | lookup::TEXTURE_LOOKUP => bundle.resource(i),
                    _ => continue,
                };
                match a.rtype {
                    depot::RTYPE => {
                        let payload = self.payload(entry.file, entry.size as usize)?;
                        let d = depot::read(&payload, &a.rmeta).map_err(|e| {
                            Error::Unsupported(format!("{} ({}): {e}", entry.name, entry.bundle))
                        })?;
                        let home = m.depots.len();
                        for (key, block) in &d.entries {
                            m.depot_keys.entry(*key).or_insert((home, *block));
                        }
                        m.depots.push((entry, d));
                    }
                    lookup::PROGRAM_LOOKUP => {
                        let db = self.read_database(bundle, &entry)?;
                        let home = m.databases.len();
                        for key in db.lookup.rows.keys() {
                            m.rows.entry(*key).or_insert(home);
                        }
                        m.databases.push((entry, db));
                    }
                    _ => {
                        let payload = self.payload(entry.file, entry.size as usize)?;
                        let rows = lookup::read_texture_sets(&payload).map_err(|e| {
                            Error::Unsupported(format!("{} ({}): {e}", entry.name, entry.bundle))
                        })?;
                        for (key, set) in rows {
                            m.texture_sets.entry(key).or_insert(set);
                        }
                    }
                }
            }
        }
        Ok(m)
    }

    /// The material `key` as a new `frostbite.material` entity of `w`: the depot block it names, the programs of its
    /// row (as [`Source::read_material_program`]) and its texture set. A part no bundle of the scope holds is named in
    /// the material's `unresolved`; a permutation or program that a row names and the scope does not hold is an error.
    pub fn resolve_material(
        &self,
        scope: &Scope<'_>,
        materials: &Materials,
        key: u64,
        w: &mut World,
    ) -> Result<Entity> {
        let mut unresolved = Vec::new();
        let depot = match materials.depot_keys.get(&key) {
            Some(&(home, block)) => {
                let (entry, d) = &materials.depots[home];
                let b = d.blocks.get(block).ok_or_else(|| {
                    Error::Missing(format!(
                        "{}: key {key:#018x} names block {block} of {}",
                        entry.name,
                        d.blocks.len()
                    ))
                })?;
                let parameters = depot::params(&b.data)
                    .map_err(|e| Error::Unsupported(format!("{} key {key:#018x}: {e}", entry.name)))?;
                Some(MaterialDepot {
                    resource: entry.name.clone(),
                    bundle: entry.bundle.clone(),
                    block: b.clone(),
                    parameters,
                })
            }
            None => {
                unresolved.push("depot: no ShaderBlockDepot of the scope holds the key".to_owned());
                None
            }
        };
        let programs = match materials.rows.get(&key) {
            Some(&home) => {
                let (database, db) = &materials.databases[home];
                self.spawn_programs(scope, database, db, key, w)?
            }
            None => {
                unresolved
                    .push("programs: no program database of the scope has a row for the key".to_owned());
                Vec::new()
            }
        };
        let texture_set = match materials.texture_sets.get(&key) {
            Some(&id) => Some((
                id,
                scope
                    .resource_by_id(lookup::STREAMABLE_TEXTURE_SET, id)
                    .map(|e| e.name),
            )),
            None => {
                unresolved
                    .push("texture set: no texture lookup of the scope has a row for the key".to_owned());
                None
            }
        };
        if let Some((id, None)) = &texture_set {
            unresolved.push(format!("texture set {id}: in no bundle of the scope"));
        }
        let e = w.spawn();
        w.insert(e, Name(format!("material {key:#018x}")));
        w.insert(
            e,
            FrostbiteMaterial {
                key,
                depot,
                programs,
                texture_set,
                textures: Vec::new(),
                unresolved,
            },
        );
        Ok(e)
    }

    /// The materials of a MeshSet entity's section keys ([`Source::read_entry`] made it): one `frostbite.material`
    /// entity for each distinct key, recorded in the MeshSet's `materials`. Entities made for earlier keys stay when
    /// a later key's resolution is an error.
    pub fn resolve_materials(
        &self,
        scope: &Scope<'_>,
        materials: &Materials,
        meshset: Entity,
        w: &mut World,
    ) -> Result<()> {
        let mut set = w
            .get::<FrostbiteMeshSet>(meshset)
            .ok_or_else(|| Error::Unsupported("not a MeshSet entity".to_owned()))?
            .clone();
        let keys: std::collections::BTreeSet<u64> = (set.meshset.lods.iter())
            .flat_map(|l| l.sections.iter().map(|s| s.key))
            .collect();
        for key in keys {
            if let std::collections::btree_map::Entry::Vacant(slot) = set.materials.entry(key) {
                slot.insert(self.resolve_material(scope, materials, key, w)?);
            }
        }
        w.insert(meshset, set);
        Ok(())
    }
}

/// What a scope's depots, program databases and texture lookups hold by material key ([`Source::materials`]).
#[derive(Debug, Default)]
pub struct Materials {
    /// Each depot read, with its entry.
    depots: Vec<(Listed, depot::Depot)>,
    /// Material key -> (depot, block).
    depot_keys: HashMap<u64, (usize, usize)>,
    /// Each program database read, with its entry.
    databases: Vec<(Listed, FrostbitePrograms)>,
    /// Material key -> the database that has a row for it.
    rows: HashMap<u64, usize>,
    /// Material key -> the id of its streamable texture set.
    texture_sets: HashMap<u64, u64>,
}

impl Materials {
    /// How many material keys have a depot block, a program row and a texture lookup row.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.depot_keys.len(), self.rows.len(), self.texture_sets.len())
    }
}
