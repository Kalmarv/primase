//! Whether a material's surface takes its vertex colour, as the source states it (`core.vertex_color`, on material
//! entities).

use serde::Serialize;
use serde_json::Value;

use crate::layered::LayeredMaterial;
use crate::{Component, Entity, JsonError, MaterialVariants, Mesh, Semantic, World};

/// `core.vertex_color`: what a material does with `COLOR_0` as a colour. Core's convention is glTF's: a material
/// multiplies its base colour by `COLOR_0`, and a material without this component does. A reader whose source
/// shaders do not (one engine's opaque shaders) marks their materials [`VertexColor::Ignored`]
/// from a named shader table in its lowering, counting what the table does not map; it is never guessed from names.
/// The stream itself stays on the mesh: one prototype can be drawn with several materials
/// ([`MaterialVariants`]), and a consumer drops `COLOR_0` per drawn primitive. Only `COLOR_0` as a colour: a layered
/// material's weights read their own attribute ([`uses_color0`]). Further kinds are added with a source that states
/// them, as a new VERSION.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VertexColor {
    /// The surface does not take `COLOR_0`: a consumer draws it as if the primitive had none.
    Ignored,
}

impl Component for VertexColor {
    const NAME: &'static str = "core.vertex_color";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// Whether drawing with `material` reads `COLOR_0`: as a colour (it is not [`VertexColor::Ignored`]) or as a
/// layered material's weights.
pub fn uses_color0(world: &World, material: Entity) -> bool {
    world.get::<VertexColor>(material).is_none()
        || world
            .get::<LayeredMaterial>(material)
            .is_some_and(|l| l.reads(Semantic::Color(0)))
}

/// Whether some drawing of primitive `index` of `prototype`'s `mesh` reads `COLOR_0`: with its own material or a
/// variant's ([`MaterialVariants`]). A primitive without a material is drawn with the consumer's default material,
/// which takes its vertex colour (glTF).
pub fn color0_drawn(world: &World, prototype: Entity, mesh: &Mesh, index: usize) -> bool {
    let Some(p) = mesh.primitives().get(index) else {
        return false;
    };
    let variants = world
        .get::<MaterialVariants>(prototype)
        .and_then(|v| v.0.get(index));
    p.material.is_none_or(|m| uses_color0(world, m))
        || variants.is_some_and(|v| v.values().any(|&m| uses_color0(world, m)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::layered::{LayerWeights, WeightMapping};
    use crate::{Primitive, Stream, Topology};

    fn mesh(material: Option<Entity>) -> Mesh {
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; 3]))]),
            indices: None,
            material,
        }])
    }

    #[test]
    fn shown_by_its_name() {
        assert_eq!(
            VertexColor::Ignored.to_json().unwrap(),
            Some(serde_json::json!("ignored"))
        );
    }

    /// Absent, a material takes `COLOR_0`; Ignored, not, unless it is layered with weights in `COLOR_0`. A
    /// primitive's `COLOR_0` is drawn when any of its materials, variants included, reads it.
    #[test]
    fn color0_is_drawn_when_any_material_drawing_it_reads_it() {
        let mut w = World::new();
        let plain = w.spawn();
        let ignores = w.spawn();
        w.insert(ignores, VertexColor::Ignored);
        let layered = w.spawn();
        w.insert(layered, VertexColor::Ignored);
        w.insert(
            layered,
            LayeredMaterial {
                layers: vec![],
                weights: LayerWeights::new(Semantic::Color(0), WeightMapping::Identity),
            },
        );
        assert!(uses_color0(&w, plain));
        assert!(!uses_color0(&w, ignores));
        assert!(uses_color0(&w, layered));

        let proto = w.spawn();
        let m = mesh(Some(ignores));
        assert!(!color0_drawn(&w, proto, &m, 0));
        w.insert(proto, MaterialVariants(vec![BTreeMap::from([(1, plain)])]));
        assert!(color0_drawn(&w, proto, &m, 0), "a variant takes it");
        assert!(
            color0_drawn(&w, proto, &mesh(None), 0),
            "the default material takes it"
        );
        assert!(!color0_drawn(&w, proto, &m, 1), "no such primitive");
    }
}
