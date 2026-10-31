//! Materials and textures ([`Materials`]): each material (`CPlugMaterial`) an entity with [`TrackmaniaMaterial`] and
//! `core.name`, each image its texture slots show an entity with `core.texture` (decoded from its DDS), `core.name`
//! and `core.source`, each read once however many materials share it.
use std::collections::BTreeMap;

use std::sync::Arc;

use gbx_formats::{Node, NodeIndex};
use primase_core::{Entity, Name, Source, TexRole, World};

use crate::Result;
use crate::components::{TrackmaniaMaterial, TrackmaniaTextureSlot};
use crate::read::files::GameFiles;
use crate::read::library::{At, Doc, Library};
use crate::read::solid::MaterialRef;

/// The materials and images read so far.
#[derive(Debug, Default)]
pub struct Materials {
    materials: BTreeMap<MaterialRef, Option<Entity>>,
    images: BTreeMap<String, Option<Entity>>,
    /// Each image not read, with why.
    pub images_unread: BTreeMap<String, String>,
    /// Each material not read, with why.
    pub materials_unread: BTreeMap<String, String>,
}

/// A material's name: its file name without `.Material.Gbx`, or `<file>#<node>` for one stored inside another file.
fn material_name(r: &MaterialRef) -> String {
    match r {
        MaterialRef::File(p) => {
            let file = p.rsplit(['\\', '/']).next().unwrap_or(p);
            file.split('.').next().unwrap_or(file).to_owned()
        }
        MaterialRef::Node(p, i) => format!("{p}#{i}"),
    }
}

impl Materials {
    /// The material entity of `r`, read on first use.
    pub fn material(
        &mut self,
        lib: &mut Library<'_>,
        files: &GameFiles,
        w: &mut World,
        r: &MaterialRef,
    ) -> Result<Option<Entity>> {
        if let Some(e) = self.materials.get(r) {
            return Ok(*e);
        }
        let at = match r {
            MaterialRef::File(p) => lib.main(p)?,
            MaterialRef::Node(p, i) => lib.load(p)?.map(|doc| At {
                doc,
                index: NodeIndex(*i),
            }),
        };
        let e = match at {
            None => {
                self.materials_unread
                    .insert(material_name(r), "its file can't be read".into());
                None
            }
            Some(at) => match at.node() {
                Node::Material(m) => {
                    let m = m.clone();
                    let path = match r {
                        MaterialRef::File(p) => p.clone(),
                        MaterialRef::Node(p, i) => format!("{p}#{i}"),
                    };
                    let shader = Library::file_of(&at.doc, m.shader);
                    let mut slots = Vec::new();
                    if let Some(custom) = lib.follow(&at.doc, m.custom_material.flatten())?
                        && let Node::MaterialCustom(c) = custom.node()
                    {
                        for bitmap in c.textures.clone().unwrap_or_default() {
                            let slot = match &bitmap.name {
                                gbx_formats::Id::Name(n) => n.clone(),
                                other => format!("{other:?}"),
                            };
                            slots.push(self.slot(lib, files, w, &custom.doc, slot, bitmap.texture)?);
                        }
                    }
                    let e = w.spawn();
                    w.insert(
                        e,
                        TrackmaniaMaterial {
                            path: path.clone(),
                            shader,
                            slots,
                            surface_id: m.surface_id,
                        },
                    );
                    w.insert(e, Name(material_name(r)));
                    w.insert(e, Source(path));
                    Some(e)
                }
                other => {
                    self.materials_unread
                        .insert(material_name(r), format!("class {}", other.class_name()));
                    None
                }
            },
        };
        self.materials.insert(r.clone(), e);
        Ok(e)
    }

    /// One texture slot: the bitmap `texture` (a reference made in `doc`) names, and its image.
    fn slot(
        &mut self,
        lib: &mut Library<'_>,
        files: &GameFiles,
        w: &mut World,
        doc: &Arc<Doc>,
        slot: String,
        texture: Option<NodeIndex>,
    ) -> Result<TrackmaniaTextureSlot> {
        let mut out = TrackmaniaTextureSlot {
            slot,
            bitmap: Library::file_of(doc, texture),
            image: None,
            texture: None,
            scale: [1.0, 1.0],
            translation: [0.0, 0.0],
            rotation: 0.0,
        };
        let Some(bitmap) = lib.follow(doc, texture)? else {
            return Ok(out);
        };
        let Node::TextureBitmap(b) = bitmap.node() else {
            return Ok(out);
        };
        out.scale = b.texcoord_scale.unwrap_or([1.0, 1.0]);
        out.translation = b.texcoord_translation.unwrap_or([0.0, 0.0]);
        out.rotation = b.texcoord_rotation.unwrap_or(0.0);
        if let Some(image) = b
            .image
            .as_ref()
            .and_then(|i| Library::file_of(&bitmap.doc, i.image))
        {
            out.texture = self.image(files, w, &image)?;
            out.image = Some(image);
        }
        Ok(out)
    }

    /// The texture entity of the image at `path`, decoded on first use.
    fn image(&mut self, files: &GameFiles, w: &mut World, path: &str) -> Result<Option<Entity>> {
        let key = path.to_lowercase();
        if let Some(e) = self.images.get(&key) {
            return Ok(*e);
        }
        let e = match files.read(path)? {
            None => {
                self.images_unread.insert(path.to_owned(), "no such file".into());
                None
            }
            Some(bytes) if bytes.starts_with(b"DDS ") => match primase_dds::read(&bytes) {
                Ok(mut texture) => {
                    texture.role = TexRole::Other;
                    let e = w.spawn();
                    w.insert(e, texture);
                    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
                    w.insert(e, Name(file.split('.').next().unwrap_or(file).to_owned()));
                    w.insert(e, Source(path.to_owned()));
                    Some(e)
                }
                Err(err) => {
                    self.images_unread.insert(path.to_owned(), err.to_string());
                    None
                }
            },
            Some(_) => {
                self.images_unread
                    .insert(path.to_owned(), "not a DDS image".into());
                None
            }
        };
        self.images.insert(key, e);
        Ok(e)
    }
}
