//! A prototype's or form's mesh from the `.modelbin` files its `forzatech.model_files` name: the meshes of its level
//! from each file in turn, one primitive each, in the files' order, positions and normals converted to the world's
//! frame ([`crate::FRAME`]) and each triangle's winding reversed with it.
//!
//! A primitive's material is a material entity per (file, `MatI` id), made once: the id the season takes from the
//! mesh's base material group (spring, summer, autumn, winter; -1 is the group's summer id), with its chain and
//! textures ([`super::materials`]). A model with a variant
//! table gives the prototype `core.material_variants`: for each variant whose group gives a primitive another material,
//! that material. Texture coordinates are kept as stored (v down, as Direct3D samples them).
//!
//! Not read here: vertex colours and tangents (what their values mean to the game's shaders is not established), skin
//! and morph buffers.
use super::materials::{Library, MaterialReport};
use super::open::Track;
use crate::components::{MaterialRef, MeshRecord, MeshRecords, ModelFile, ModelFiles};
use crate::{Error, FRAME, Result};
use forzatech_formats::modelbin::Model;
use forzatech_formats::track::Location;
use forzatech_formats::world::Season;
use primase_core::{Entity, MaterialVariants, Mesh, Name, Primitive, Semantic, Stream, Topology, World};
use std::collections::{BTreeMap, HashMap};

/// Material entities already made in one world, by file and `MatI` id, and the texture entities they name, by path
/// hash; read through the track's [`Library`].
#[derive(Debug)]
pub struct Materials<'l> {
    by_id: HashMap<(Location, u32), Entity>,
    library: &'l mut Library,
    textures: HashMap<u32, Entity>,
    pub report: MaterialReport,
}

impl<'l> Materials<'l> {
    pub fn new(library: &'l mut Library) -> Self {
        Materials {
            by_id: HashMap::new(),
            library,
            textures: HashMap::new(),
            report: MaterialReport::default(),
        }
    }
}

/// What loading meshes read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelReport {
    pub files: usize,
    pub meshes: usize,
    pub triangles: u64,
    /// Meshes of a file not of the form's level.
    pub other_level: usize,
}

impl ModelReport {
    pub fn lines(&self) -> Vec<(String, String)> {
        vec![(
            "forzatech models".into(),
            format!(
                "{} files, {} meshes, {} triangles ({} meshes of other levels left in their files)",
                self.files, self.meshes, self.triangles, self.other_level
            ),
        )]
    }
}

/// Give `e` (a prototype or a form with `forzatech.model_files`) its mesh, its meshes' records and, when its model
/// has a variant table, its material variants. Nothing is given when its files hold no mesh of its level.
pub fn load_mesh(
    track: &Track,
    season: Season,
    world: &mut World,
    e: Entity,
    materials: &mut Materials<'_>,
    report: &mut ModelReport,
) -> Result<()> {
    let Some(files) = world.get::<ModelFiles>(e).cloned() else {
        return Ok(());
    };
    let mut primitives = Vec::new();
    let mut records = Vec::new();
    let mut variants: Vec<BTreeMap<u32, Entity>> = Vec::new();
    for file in &files.files {
        let at = Location {
            chunk: file.chunk,
            entry: file.entry,
        };
        let bytes = track.read(at)?;
        let model = Model::parse(&bytes).map_err(|err| Error::Model(format!("{}: {err}", file.path)))?;
        report.files += 1;
        for (k, mesh) in model.meshes.iter().enumerate() {
            if let Some(level) = files.level
                && mesh.lod_flags & (1u16 << (u32::from(level) + 1)) == 0
            {
                report.other_level += 1;
                continue;
            }
            let data = model
                .mesh_data(k)
                .map_err(|err| Error::Model(format!("{} mesh {k}: {err}", file.path)))?;
            let mut prim =
                primitive(&data).map_err(|err| Error::Model(format!("{} mesh {k}: {err}", file.path)))?;
            let base = mesh
                .material_groups
                .first()
                .ok_or_else(|| Error::Model(format!("{} mesh {k}: no material group", file.path)))?;
            prim.material = material(track, world, materials, at, &model, file, *base, season)?;
            let mut overrides = BTreeMap::new();
            for (v, row) in model.header.variants.iter().enumerate() {
                let group = row
                    .get(k)
                    .and_then(|g| mesh.material_groups.get(usize::from(*g)))
                    .ok_or_else(|| {
                        Error::Model(format!("{} variant {v}: no group for mesh {k}", file.path))
                    })?;
                let m = material(track, world, materials, at, &model, file, *group, season)?;
                if m != prim.material
                    && let Some(m) = m
                {
                    overrides.insert(
                        u32::try_from(v).map_err(|_| Error::Model("variant count".into()))?,
                        m,
                    );
                }
            }
            report.meshes += 1;
            report.triangles += prim.indices.as_ref().map_or(0, |i| i.len() as u64 / 3);
            primitives.push(prim);
            variants.push(overrides);
            records.push(MeshRecord {
                file: file.path.clone(),
                name: mesh.name.clone(),
                lod_flags: mesh.lod_flags,
                bucket_flags: mesh.bucket_flags,
                bucket_order: mesh.bucket_order,
            });
        }
    }
    if primitives.is_empty() {
        return Ok(());
    }
    world.insert(e, Mesh::new(primitives));
    world.insert(e, MeshRecords(records));
    if variants.iter().any(|v| !v.is_empty()) {
        world.insert(e, MaterialVariants(variants));
    }
    Ok(())
}

/// The material entity a group's id for `season` names, made on first use; `None` for a slot with no instance.
#[allow(
    clippy::too_many_arguments,
    reason = "the model being read and where its material entities go"
)]
fn material(
    track: &Track,
    world: &mut World,
    materials: &mut Materials<'_>,
    at: Location,
    model: &Model<'_>,
    file: &ModelFile,
    group: [i16; 4],
    season: Season,
) -> Result<Option<Entity>> {
    let summer = group[usize::from(Season::Summer.index())];
    let id = match group[usize::from(season.index())] {
        -1 => summer,
        id => id,
    };
    let mati = model
        .material(id)
        .ok_or_else(|| Error::Model(format!("{}: no MatI {id}", file.path)))?;
    let Some(instance) = &mati.instance else {
        return Ok(None);
    };
    let key = (at, mati.id);
    if let Some(e) = materials.by_id.get(&key) {
        return Ok(Some(*e));
    }
    let e = world.spawn();
    world.insert(e, Name(mati.name.clone()));
    world.insert(
        e,
        MaterialRef {
            chunk: at.chunk,
            entry: at.entry,
            file: file.path.clone(),
            id: mati.id,
        },
    );
    let chain = materials
        .library
        .chain(instance, season, &mut materials.report)
        .map_err(|err| Error::Model(format!("{} MatI {}: {err}", file.path, mati.id)))?;
    let textures = materials.library.textures(
        track,
        &chain,
        &mut materials.textures,
        world,
        &mut materials.report,
    )?;
    world.insert(e, chain);
    world.insert(e, textures);
    materials.by_id.insert(key, e);
    Ok(Some(e))
}

/// One mesh's triangles: positions and normals in the world's frame, texture coordinates as stored, each triangle
/// reversed as the frame mirrors.
fn primitive(data: &forzatech_formats::modelbin::MeshData<'_>) -> std::result::Result<Primitive, String> {
    let positions = data
        .positions()
        .ok_or("no POSITION0")?
        .map_err(|e| e.to_string())?;
    let mut streams = BTreeMap::new();
    streams.insert(
        Semantic::Position,
        Stream::F32x3(positions.iter().map(|p| FRAME.point(*p)).collect()),
    );
    if let Some(n) = data.normals() {
        streams.insert(
            Semantic::Normal,
            Stream::F32x3(n.iter().map(|d| FRAME.direction(*d)).collect()),
        );
    }
    for set in 0..5u8 {
        if let Some(uv) = data.uvs(u32::from(set)) {
            streams.insert(Semantic::TexCoord(set), Stream::F32x2(uv));
        }
    }
    let mut indices = data.local_indices();
    if !indices.len().is_multiple_of(3) {
        return Err(format!("{} indices, not whole triangles", indices.len()));
    }
    if FRAME.flips_winding() {
        for t in indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    Ok(Primitive {
        topology: Topology::Triangles,
        streams,
        indices: Some(indices),
        material: None,
    })
}
