//! The asset browser previews content rather than a distance band's empty stand-in. This is a caller-selected
//! preview policy, not an interpretation of the game's current camera distance.
use crate::components::AnvilRenderStatus;
use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::{Component, Lods, Mesh, World};

#[derive(Clone, Debug)]
pub struct SelectAnvilPreviewLod;
pub const SELECT_ANVIL_PREVIEW_LOD: PassSpec = PassSpec {
    name: "select-anvil-preview-lod",
    options: &[],
    make: |_| Ok(Box::new(SelectAnvilPreviewLod)),
};

fn triangles(mesh: &Mesh) -> usize {
    mesh.primitives()
        .iter()
        .map(|p| p.indices.as_ref().map_or(p.vertex_count(), Vec::len) / 3)
        .sum()
}

fn canopy(world: &World, mesh: &Mesh) -> bool {
    !mesh.primitives().is_empty()
        && mesh.primitives().iter().all(|p| {
            p.material
                .and_then(|m| world.get::<AnvilRenderStatus>(m))
                .is_some_and(|s| s.template.as_deref() == Some("MAD_FakeForest"))
        })
}

impl Pass for SelectAnvilPreviewLod {
    fn name(&self) -> &'static str {
        "select-anvil-preview-lod"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[Lods::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Lods::NAME]
    }
    fn approximates(&self) -> &'static str {
        "browser policy: a tiny FakeForest stand-in is previewed through its authored canopy or impostor form"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut changes = Vec::new();
        for (entity, lods) in world.iter::<Lods>() {
            let Some(own) = world.get::<Mesh>(entity) else {
                continue;
            };
            // The material identity is essential: a two-triangle sign or a twelve-triangle box remains real content.
            if !canopy(world, own) || triangles(own) > 12 {
                continue;
            }
            let selected = lods.forms.iter().enumerate().find(|(_, form)| {
                world.get::<Mesh>(form.form).is_some_and(|mesh| {
                    world
                        .get::<AnvilRenderStatus>(form.form)
                        .is_some_and(|s| s.source.starts_with("Impostor "))
                        || triangles(mesh) > triangles(own).max(12)
                })
            });
            if let Some((index, _)) = selected {
                changes.push((entity, index + 1));
            }
        }
        for (entity, index) in changes {
            if let Some(lods) = world.get_mut::<Lods>(entity) {
                lods.default = Some(index);
            }
            log.approximate(
                "preview authored canopy/impostor instead of its near-distance FakeForest stand-in",
                1,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{LodForm, Primitive, Semantic, Stream, Topology};
    use std::collections::BTreeMap;

    fn mesh(material: primase_core::Entity, triangles: usize) -> Mesh {
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; triangles * 3]))]),
            indices: None,
            material: Some(material),
        }])
    }

    #[test]
    fn only_an_identified_canopy_standin_switches_to_its_authored_form() {
        let mut world = World::new();
        let canopy = world.spawn();
        world.insert(
            canopy,
            AnvilRenderStatus {
                source: "Material".into(),
                template: Some("MAD_FakeForest".into()),
                unsupported: false,
                reason: "authored vertex colour".into(),
            },
        );
        let ordinary = world.spawn();
        let form = world.spawn();
        world.insert(form, mesh(canopy, 30));
        let tiny_canopy = world.spawn();
        let tiny_sign = world.spawn();
        for (entity, material) in [(tiny_canopy, canopy), (tiny_sign, ordinary)] {
            world.insert(entity, mesh(material, 2));
            world.insert(
                entity,
                Lods {
                    detail: 0,
                    forms: vec![LodForm { detail: 1, form }],
                    default: None,
                },
            );
        }
        SelectAnvilPreviewLod
            .run(&mut world, &mut PassLog::default())
            .unwrap();
        assert_eq!(world.get::<Lods>(tiny_canopy).unwrap().default, Some(1));
        assert_eq!(world.get::<Lods>(tiny_sign).unwrap().default, None);
    }
}
