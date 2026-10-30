//! The low-resolution twins the read brought in ([`LowTexturePayloads`]) given to the textures made from their sources,
//! as each texture's one other form (`core.texture_forms`).

use crate::sekiro::source::LowTexturePayloads;
use primase_core::pass::PassLog;
use primase_core::{Entity, Name, Texture, TextureForm, TextureForms, World};

/// Give each of `made` (a texture entity and the stem of the source texture it was decoded from) that has no forms
/// yet its low-resolution twin as its one other form, at level 1: the twin decoded as stored, in the texture's role,
/// colour space and normal convention, named as the texture is with the stem `<stem>_l`. Nothing happens without the
/// read's [`LowTexturePayloads`] (a read that did not ask for them); a texture whose stem has no twin keeps only
/// itself (the read names those), and a twin the DDS reader refuses is counted and left out.
pub(crate) fn add_low_forms(
    world: &mut World,
    made: impl IntoIterator<Item = (Entity, String)>,
    log: &mut PassLog,
) {
    world.resource_scope::<LowTexturePayloads, _>(|world, low| {
        let Some(low) = low else {
            return;
        };
        for (texture, stem) in made {
            if world.has::<TextureForms>(texture) {
                continue;
            }
            let (Some(own), Some(bytes)) = (world.get::<Texture>(texture), low.0.get(&stem)) else {
                continue;
            };
            let mut twin = match primase_dds::read(bytes) {
                Ok(twin) => twin,
                Err(why) => {
                    log.omit(
                        &format!("low-resolution twins the DDS reader refuses, left out ({why})"),
                        1,
                    );
                    continue;
                }
            };
            twin.role = own.role;
            twin.color_space = own.color_space;
            twin.normal = own.normal;
            let name = world.get::<Name>(texture).map_or_else(
                || format!("{stem}_l"),
                |n| n.0.replacen(&stem, &format!("{stem}_l"), 1),
            );
            let form = world.spawn();
            world.insert(form, Name(name));
            world.insert(form, twin);
            world.insert(
                texture,
                TextureForms {
                    detail: 0,
                    forms: vec![TextureForm {
                        detail: 1,
                        texture: form,
                    }],
                },
            );
            log.count("textures given their low-resolution twin as a form", 1);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::{ColorSpace, Component, NormalConvention, TexFormat, TexRole};

    fn rgba(side: u32, value: u8) -> Texture {
        Texture {
            width: side,
            height: side,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![value; (side * side * 4) as usize]],
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        }
    }

    /// A texture made from a stem with a twin gets the twin, decoded as stored, as its one form at level 1, in the
    /// texture's role, colour space and normal convention and named after it; one without a twin, one that already has
    /// forms, and every texture of a world without the read's twins are left as they are.
    #[test]
    fn a_texture_gets_its_low_twin_as_its_form() {
        let mut w = World::new();
        let normal = w.spawn();
        let mut t = rgba(8, 128);
        t.role = TexRole::Normal;
        t.normal = Some(NormalConvention::GLTF);
        w.insert(normal, t);
        w.insert(normal, Name("aet000_001_n@normal".into()));
        let alone = w.spawn();
        w.insert(alone, rgba(4, 9));
        let mut log = PassLog::default();
        add_low_forms(&mut w, [(normal, "aet000_001_n".to_owned())], &mut log);
        assert!(!w.has::<TextureForms>(normal), "no twins read: nothing happens");

        let twin = primase_dds::write_dx10(&rgba(2, 60)).unwrap();
        w.insert_resource(LowTexturePayloads(
            [("aet000_001_n".to_owned(), twin.as_slice().into())].into(),
        ));
        add_low_forms(
            &mut w,
            [
                (normal, "aet000_001_n".to_owned()),
                (alone, "aet000_002_a".to_owned()),
            ],
            &mut log,
        );
        assert_eq!(
            log.counts["textures given their low-resolution twin as a form"],
            1
        );
        assert!(!w.has::<TextureForms>(alone));
        let forms = w.get::<TextureForms>(normal).unwrap().clone();
        assert_eq!(
            (forms.detail, forms.forms.len(), forms.forms[0].detail),
            (0, 1, 1)
        );
        let form = forms.forms[0].texture;
        let low = w.get::<Texture>(form).unwrap();
        assert_eq!((low.width, low.levels[0][0]), (2, 60));
        assert_eq!(
            (low.role, low.normal),
            (TexRole::Normal, Some(NormalConvention::GLTF))
        );
        assert_eq!(w.get::<Name>(form).unwrap().0, "aet000_001_n_l@normal");
        assert_eq!(forms.verify(normal, &w), Ok(()));
        // a second call leaves a texture that has forms as it is
        add_low_forms(&mut w, [(normal, "aet000_001_n".to_owned())], &mut log);
        assert_eq!(
            log.counts["textures given their low-resolution twin as a form"],
            1
        );
    }
}
