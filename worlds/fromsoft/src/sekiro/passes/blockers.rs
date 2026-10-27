//! Interior-blocker recognition is a per-mesh material policy. Preserve the source
//! records outside `core.mesh`, so no renderer accidentally draws the cards.
use crate::materials::Material;
use primase_core::{Transform, world::Resource};
use std::collections::BTreeMap;

/// An interior blocker card: a non-skipped `GXFlver_Col*` shader with main albedo `m[0-9][0-9]_black_a`.
pub fn is_card(material: &Material) -> bool {
    material.skip.is_none()
        && matches_binding(
            &material.shader,
            crate::preview::preview(material).albedo.as_deref(),
        )
}
fn matches_binding(shader: &str, albedo: Option<&str>) -> bool {
    shader.starts_with("GXFlver_Col")
        && albedo.is_some_and(|name| {
            let b = name.as_bytes();
            b.len() == 11
                && b[0] == b'm'
                && b[1].is_ascii_digit()
                && b[2].is_ascii_digit()
                && &b[3..] == b"_black_a"
        })
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Card {
    pub source_mesh: usize,
    pub material: Material,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub tangents: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}
impl Card {
    pub fn from_part(part: crate::geometry::Part, material: Material) -> Self {
        Self {
            source_mesh: part.source_mesh,
            material,
            positions: part.mesh.positions,
            normals: part.mesh.normals,
            uv: part.mesh.uv,
            tangents: part.tangents,
            indices: part.mesh.indices,
        }
    }
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct Placement {
    pub source: String,
    pub prototype: String,
    /// Local-to-world columns in the core coordinate frame.
    pub transform: [[f32; 3]; 4],
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct BlockerCards {
    pub prototypes: BTreeMap<String, Vec<Card>>,
    pub placements: Vec<Placement>,
}
impl Resource for BlockerCards {
    const NAME: &'static str = "fromsoft.blocker_cards";
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
impl BlockerCards {
    pub fn record(&mut self, prototype: &str, source: String, transform: &Transform, cards: &[Card]) {
        if cards.is_empty() {
            return;
        }
        self.prototypes
            .entry(prototype.into())
            .or_insert_with(|| cards.to_vec());
        self.placements.push(Placement {
            source,
            prototype: prototype.into(),
            transform: transform.cols,
        });
    }
    pub fn triangles(&self) -> usize {
        self.placements
            .iter()
            .map(|p| {
                self.prototypes[&p.prototype]
                    .iter()
                    .map(|c| c.indices.len() / 3)
                    .sum::<usize>()
            })
            .sum()
    }
    pub fn meshes(&self) -> usize {
        self.prototypes.values().map(Vec::len).sum()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn material_rule_requires_both_shader_and_exact_texture_pattern() {
        assert!(matches_binding("GXFlver_Col", Some("m11_black_a")));
        assert!(matches_binding("GXFlver_ColDif", Some("m99_black_a")));
        for name in [
            "m11_ground_400_a",
            "m11_metal_00_a",
            "m11_obj_box_01_a",
            "m1_black_a",
            "m111_black_a",
            "mxx_black_a",
            "m11_black_a_extra",
        ] {
            assert!(!matches_binding("GXFlver_ColDif", Some(name)), "{name}");
        }
        assert!(!matches_binding("MetallicValue", Some("m11_black_a")));
        assert!(!matches_binding("GXFlver_Col", None));
    }
}
