//! A material's program read through to its compiled bytecode: a program database ([`FrostbitePrograms`]) names, per
//! material key, program indices; a program record names permutation ids; a permutation names CompiledBytecode
//! resources by GUID ([`crate::permutation`]). The game resolves ids and names in every bundle loaded: a bundle with a
//! database holds the permutations it adds and names the stock ones, which lie in another bundle (the game's
//! configuration bundle in `globals.toc`, a level's root). A [`Scope`] is the bundles an id or a name is looked up in,
//! the earlier first; one that none holds is an error naming it, not a skipped program.

use std::collections::HashMap;

use primase_core::shader::Shader;
use primase_core::{Entity, World};

use super::entry::spawn_entity;
use super::folder::Source;
use super::listing::{Bundle, Listed};
use crate::components::{FrostbiteBytecode, FrostbitePermutation, FrostbiteProgram, FrostbitePrograms};
use crate::permutation::{self, Permutation};
use crate::{Error, Result, bytecode, lookup};

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
                    permutation::RASTER | permutation::COMPUTE => {
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

    /// The bundle and entry of the permutation with `id`, a raster one first.
    fn permutation(&self, id: u64) -> Option<(&'a Bundle, Listed)> {
        let &(b, i) = [permutation::RASTER, permutation::COMPUTE]
            .iter()
            .find_map(|t| self.permutations.get(&(*t, id)))?;
        Some((&self.bundles[b], self.bundles[b].resource(i)))
    }

    /// The bundle and entry of the CompiledBytecode resource named `name`.
    fn bytecode(&self, name: &str) -> Option<(&'a Bundle, Listed)> {
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
        Ok(PermutationRead {
            entry: entry.clone(),
            permutation,
            programs,
        })
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
            programs.push((index, record.key, permutations));
        }
        Ok(programs
            .into_iter()
            .map(|(index, record_key, permutations)| {
                let permutations: Vec<Entity> = permutations.into_iter().map(|p| p.spawn(w)).collect();
                let e = spawn_entity(w, database, format!("{}#{index}", database.name));
                w.insert(
                    e,
                    FrostbiteProgram {
                        material,
                        index,
                        record_key,
                        permutations,
                    },
                );
                e
            })
            .collect())
    }
}
