//! How a writer for a format of its own extends the glTF this crate writes.
//!
//! The glTF writer ([`crate::write`]) writes standard glTF 2.0 and the Khronos extensions it names, and nothing of a
//! format of primase's: what glTF has no form for is left out, counted by component name
//! ([`crate::WriteReport::components_without_form`]). A writer for a format built on glTF that carries more (a
//! viewer's tiles) passes an [`Extension`] in [`crate::WriteOptions::extensions`], and the writer offers it two
//! things:
//!
//! - **Materials.** Each material the writer writes, first whole ([`Extension::material`]: the extension writes the
//!   material object itself), else as an extension object among the ones the writer puts on the material it writes
//!   from the `pbr.material` ([`Extension::material_extension`]). Either goes through the writer's own operations
//!   ([`MaterialWriter`]): another material entity written as the writer writes it, a texture slot as its
//!   `textureInfo`, so what the extension references (layer materials, textures, samplers, images) takes its index
//!   and its place in the binary buffer as the writer's own do. Its names go into `extensionsUsed` in the order
//!   [`Extension::names`] lists them, after the standard ones.
//! - **Extras** of each object the writer writes ([`Extension::extras`]). This call is not a way to carry a format's
//!   data: core is the interchange, and a format's own data comes from the passes that make its components, written
//!   by that format's own writer. It exists so the web format's current tiles keep their bytes, component maps in
//!   extras included, while that format moves off them; nothing else uses it, and nothing new should.
//!
//! The extension's methods take `&self`; one that counts what it did keeps its counts behind a lock and is read by its
//! caller after the write.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use primase_core::{Entity, Mesh, World};
use serde_json::{Map, Value};

use crate::Result;

pub use crate::write::MaterialWriter;

/// A writer's addition to the glTF this crate writes (see the module documentation).
pub trait Extension: Send + Sync {
    /// The glTF extensions it may put on materials, in the order `extensionsUsed` lists those it put.
    fn names(&self) -> &'static [&'static str];

    /// `material` written whole by the extension, or `None` for the writer to write it from its `pbr.material`.
    fn material(&self, _w: &mut MaterialWriter<'_, '_>, _material: Entity) -> Result<Option<WholeMaterial>> {
        Ok(None)
    }

    /// An extension object for `material`, which the writer is writing from its `pbr.material`: placed among the
    /// material's extensions after the writer's own material extensions and before `KHR_materials_unlit`. `sets`
    /// takes the texture coordinate sets its slots sample.
    fn material_extension(
        &self,
        _w: &mut MaterialWriter<'_, '_>,
        _material: Entity,
        _sets: &mut BTreeSet<u8>,
    ) -> Result<Option<MaterialExtension>> {
        Ok(None)
    }

    /// Extras for `object`, which the writer writes, merged into the extras it writes itself (a mesh's `targetNames`).
    fn extras(&self, _world: &World, _object: Object<'_>) -> Result<Option<Map<String, Value>>> {
        Ok(None)
    }
}

/// A material an extension wrote whole.
#[derive(Clone, Debug)]
pub struct WholeMaterial {
    /// The material object, its extras included (written as it is).
    pub material: Map<String, Value>,
    /// The glTF extension it uses (one of [`Extension::names`]).
    pub extension: &'static str,
    /// The texture coordinate sets it samples: every primitive drawn with it must have them.
    pub sets: BTreeSet<u8>,
    /// A vertex attribute it reads (`COLOR_1`): a primitive drawn with it that lacks it is counted.
    pub attribute: Option<String>,
    /// The material entity's components it expresses (not counted as without a glTF form).
    pub components: &'static [&'static str],
}

/// An extension object of a material the writer writes.
#[derive(Clone, Debug)]
pub struct MaterialExtension {
    /// Its name (one of [`Extension::names`]).
    pub name: &'static str,
    pub value: Value,
    /// The material entity's components it expresses (not counted as without a glTF form).
    pub components: &'static [&'static str],
}

/// An object the writer writes, offered to [`Extension::extras`].
#[derive(Clone, Copy, Debug)]
pub enum Object<'a> {
    /// The scene.
    Scene,
    /// A light's node.
    Light(Entity),
    /// A placement's node.
    Placement { placement: Entity, prototype: Entity },
    /// A prototype's mesh, or a piece cut from it (its geometry and, per primitive, the prototype's primitive it is
    /// cut from).
    Mesh {
        prototype: Entity,
        piece: Option<(&'a Mesh, &'a [usize])>,
    },
    /// A material the writer writes from its `pbr.material`, or one an extension writes whole and asks for
    /// ([`MaterialWriter::extras`]).
    Material(Entity),
    /// A texture.
    Texture(Entity),
    /// The image of a texture, offered each time a texture of it is written.
    Image { texture: Entity },
}

/// The components the glTF object made from an entity expresses, by the kind of object: what the writer counts as
/// without a glTF form is every other one.
pub fn expressed(object: &Object<'_>) -> &'static [&'static str] {
    match object {
        Object::Scene => &[],
        Object::Light(_) => crate::write::EXPRESSED_LIGHT,
        Object::Placement { .. } => crate::write::EXPRESSED_PLACEMENT,
        Object::Mesh { .. } => crate::write::EXPRESSED_PROTOTYPE,
        Object::Material(_) => crate::write::EXPRESSED_MATERIAL,
        Object::Texture(_) | Object::Image { .. } => crate::write::EXPRESSED_TEXTURE,
    }
}

/// The extensions a write is given, in the order the writer offers them each object.
#[derive(Clone, Default)]
pub struct Extensions(pub Vec<Arc<dyn Extension>>);

impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.iter().map(|x| x.names())).finish()
    }
}
