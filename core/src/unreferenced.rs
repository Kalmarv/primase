//! `drop-unreferenced-textures`: the texture entities nothing in the world refers to, despawned, so a program that
//! writes only what its materials reach stops holding the rest.

use crate::pass::{Pass, PassLog, PassSpec};
use crate::{Texture, TextureFile, Transform, World};

/// `drop-unreferenced-textures`: every texture entity (one holding `core.texture` or `core.texture_file`) that no
/// component in the world refers to and nothing places (no `core.transform`) is despawned with everything it holds,
/// counted, with the bytes of its loaded levels. A texture no material, binding or form names is reached by no writer
/// that writes what materials bind; one a program writes on its own (a texture export) is not this pass's to drop, so
/// the pass is the program's statement, never a writer's default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropUnreferencedTextures;

/// `drop-unreferenced-textures`' declaration ([`crate::pass::PassRegistry::add`]): no options.
pub const DROP_UNREFERENCED_TEXTURES: PassSpec = PassSpec {
    name: "drop-unreferenced-textures",
    options: &[],
    make: |_| Ok(Box::new(DropUnreferencedTextures)),
};

impl Pass for DropUnreferencedTextures {
    fn name(&self) -> &'static str {
        "drop-unreferenced-textures"
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let dropped: Vec<_> = w
            .unreferenced()
            .into_iter()
            .filter(|&e| (w.has::<Texture>(e) || w.has::<TextureFile>(e)) && !w.has::<Transform>(e))
            .collect();
        let bytes: usize = dropped
            .iter()
            .filter_map(|&e| w.get::<Texture>(e))
            .flat_map(|t| t.levels.iter().map(Vec::len))
            .sum();
        log.count("unreferenced textures dropped", dropped.len() as u64);
        log.count("bytes of the unreferenced textures' loaded levels", bytes as u64);
        for e in dropped {
            crate::ledger::despawn(w, e, log, "textures nothing refers to");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::{PassManager, PassRegistry, Pipeline, RunReport};
    use crate::pbr::{PbrMaterial, TextureRef};
    use crate::{ColorSpace, Name, TexFormat, TexRole};

    fn texture(w: &mut World, name: &str) -> crate::Entity {
        let e = w.spawn();
        w.insert(e, Name(name.into()));
        w.insert(
            e,
            Texture {
                width: 1,
                height: 1,
                format: TexFormat::Rgba8Unorm,
                levels: vec![vec![1, 2, 3, 4]],
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
        );
        e
    }

    /// The texture a material binds stays, as does a placed one; the one nothing names is despawned, counted with its
    /// bytes, and declared dropped.
    #[test]
    fn a_texture_nothing_refers_to_is_dropped_and_counted() {
        let mut w = World::new();
        let bound = texture(&mut w, "bound");
        let placed = texture(&mut w, "placed");
        w.insert(placed, Transform::IDENTITY);
        let loose = texture(&mut w, "loose");
        let material = w.spawn();
        w.insert(
            material,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(bound)),
                ..Default::default()
            },
        );
        let mut report = RunReport::default();
        PassManager::new(PassRegistry::new())
            .with_pass_checks(true)
            .run(
                &mut w,
                &Pipeline {
                    passes: vec![Box::new(DropUnreferencedTextures)],
                },
                &mut report,
            )
            .unwrap();
        assert!(w.is_alive(bound) && w.is_alive(placed) && !w.is_alive(loose));
        let counts = &report.steps[0].log.counts;
        assert_eq!(counts["unreferenced textures dropped"], 1);
        assert_eq!(counts["bytes of the unreferenced textures' loaded levels"], 4);
    }
}
