//! A texture's other stored forms ([`TextureForms`], `core.texture_forms`) and the pass that keeps one
//! (`select-texture-form`). The forms stay in the world as the source stored them; a writer sees the one texture a
//! pipeline's pass leaves, so no writer chooses on its own.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use crate::{Component, Entity, JsonError, Name, Source, Texture, TextureFile, World};

/// A texture's other stored forms (`core.texture_forms`), on the texture entity materials bind. Its own texture is one
/// form, at level `detail` (0 the finest, larger coarser); `forms` are its other forms, as the source stores them.
///
/// A form is an entity of its own holding a `core.texture` or `core.texture_file`: a texture the source made apart
/// from the bound one, not levels cut from it (Elden Ring stores each asset's textures twice, in
/// `aetNNN_MMM.tpf.dcx` and a low-resolution `aetNNN_MMM_l.tpf.dcx` encoded on its own, at other sizes and sometimes in
/// another format). Nothing binds a form directly, and a form has no forms of its own. The game holds both forms at
/// once and picks one per placement: in a captured Elden Ring frame, one placement of an asset samples the full
/// texture while another placement of the same mesh samples the low one. Where a source states which form a placement
/// draws (by distance, with hysteresis), the placement carries [`crate::TextureSwitch`] and a renderer applies it per
/// frame; a writer cannot, and [`SelectTextureForm`] keeps one form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TextureForms {
    pub detail: u8,
    pub forms: Vec<TextureForm>,
}

/// One other form of a texture: its level and the entity holding it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TextureForm {
    pub detail: u8,
    pub texture: Entity,
}

impl TextureForms {
    /// The form kept for `choice`, as an index into [own texture, forms...]: the own texture for
    /// [`TextureChoice::Finest`]; for [`TextureChoice::AtMost`] the coarsest at level `n` or finer (the first listed of
    /// equal levels), the own texture when none is.
    pub fn choose(&self, choice: TextureChoice) -> usize {
        match choice {
            TextureChoice::Finest => 0,
            TextureChoice::AtMost(n) => std::iter::once(self.detail)
                .chain(self.forms.iter().map(|f| f.detail))
                .enumerate()
                .filter(|&(_, d)| d <= n)
                .fold(None, |best: Option<(usize, u8)>, (i, d)| match best {
                    Some((_, b)) if b >= d => best,
                    _ => Some((i, d)),
                })
                .map_or(0, |(i, _)| i),
        }
    }
}

fn has_texture(w: &World, e: Entity) -> bool {
    w.has::<Texture>(e) || w.has::<TextureFile>(e)
}

impl Component for TextureForms {
    const NAME: &'static str = "core.texture_forms";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.forms.iter().map(|f| f.texture));
    }
    /// On an entity with a texture; each form another alive entity, listed once, holding a texture and without forms of
    /// its own. [`SelectTextureForm`] checks it before it changes anything.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if !has_texture(w, e) {
            return Err("other forms of an entity without a texture".into());
        }
        let mut seen = BTreeSet::new();
        for f in &self.forms {
            if f.texture == e || !w.is_alive(f.texture) {
                return Err(format!("form {:?} is not another alive entity", f.texture));
            }
            if !seen.insert(f.texture) {
                return Err(format!("form {:?} is listed twice", f.texture));
            }
            if !has_texture(w, f.texture) {
                return Err(format!("form {:?} holds no texture", f.texture));
            }
            if w.has::<TextureForms>(f.texture) {
                return Err(format!("form {:?} has forms of its own", f.texture));
            }
        }
        Ok(())
    }
}

/// Which form [`SelectTextureForm`] keeps of each texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureChoice {
    /// The texture's own.
    Finest,
    /// The coarsest form at level `N` or finer; the texture's own when none is.
    AtMost(u8),
}

/// `select-texture-form{form=finest|N}` (`form` required: which stored form is drawn is the program's choice): every
/// texture with [`TextureForms`] is left with the one form [`TextureForms::choose`] takes. Another form than its own
/// replaces the texture's `core.texture` and `core.texture_file` with every component the form holds
/// ([`World::move_components`]), except the form's own `core.name` and `core.source`, which are left behind, counted:
/// the bindings and the name stay the texture's. The other forms are despawned and `core.texture_forms` removed. The
/// whole selection is checked before anything changes: an invalid one is an error and leaves the world as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectTextureForm {
    pub choice: TextureChoice,
}

/// `select-texture-form`'s declaration ([`crate::pass::PassRegistry::add`]).
pub const SELECT_TEXTURE_FORM: PassSpec = PassSpec {
    name: "select-texture-form",
    options: &[OptionSpec::required(
        "form",
        "finest",
        "finest or a level 0-255: the stored form each texture keeps (TextureChoice)",
    )],
    make: select_texture_form,
};

fn select_texture_form(g: &Given) -> Result<Box<dyn Pass>, String> {
    let choice = match g.required("form") {
        "finest" => TextureChoice::Finest,
        n => TextureChoice::AtMost(
            n.parse()
                .map_err(|_| format!("form={n} is not finest or a level 0-255"))?,
        ),
    };
    Ok(Box::new(SelectTextureForm { choice }))
}

impl Pass for SelectTextureForm {
    fn name(&self) -> &'static str {
        "select-texture-form"
    }
    fn options(&self) -> Options {
        let form = match self.choice {
            TextureChoice::Finest => "finest".to_owned(),
            TextureChoice::AtMost(n) => n.to_string(),
        };
        Options::from([("form".to_owned(), form)])
    }
    /// A texture kept in another form loses its own texture (and takes the form's).
    fn removes(&self) -> &[&'static str] {
        &[TextureForms::NAME, Texture::NAME, TextureFile::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Texture::NAME, TextureFile::NAME]
    }
    fn legalizes(&self) -> &[&'static str] {
        &[TextureForms::NAME]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let all: Vec<(Entity, TextureForms)> =
            w.iter::<TextureForms>().map(|(e, f)| (e, f.clone())).collect();
        let mut forms = BTreeSet::new();
        for (texture, held) in &all {
            held.verify(*texture, w)
                .map_err(|m| format!("{texture:?}: {m}"))?;
            for f in &held.forms {
                if !forms.insert(f.texture) {
                    return Err(format!("form {:?} belongs to more than one texture", f.texture));
                }
            }
        }
        if let Some((name, holder, form)) = w
            .references_to(&forms)
            .into_iter()
            .find(|(name, _, _)| *name != TextureForms::NAME)
        {
            return Err(format!("{name} on {holder:?} refers to form {form:?}"));
        }
        for (texture, held) in all {
            let chosen = held.choose(self.choice);
            match chosen.checked_sub(1).map(|k| held.forms[k]) {
                None => log.count("textures kept in their own form", 1),
                Some(f) => {
                    for (gone, name) in [
                        (w.remove::<Texture>(texture).is_some(), Texture::NAME),
                        (w.remove::<TextureFile>(texture).is_some(), TextureFile::NAME),
                    ] {
                        if gone {
                            log.record_removed(name);
                        }
                    }
                    // what stays with the texture whichever form is kept is its own: a form's own name or identity
                    for (left, name) in [
                        (w.remove::<Name>(f.texture).is_some(), Name::NAME),
                        (w.remove::<Source>(f.texture).is_some(), Source::NAME),
                    ] {
                        if left {
                            log.record_removed(name);
                            log.count(
                                &format!("{name} of the kept form left behind: the texture keeps its own"),
                                1,
                            );
                        }
                    }
                    let moved = w.move_components(f.texture, texture)?;
                    debug_assert!(moved, "checked alive above");
                    log.count("textures kept in another form", 1);
                }
            }
            for (k, f) in held.forms.iter().enumerate() {
                if k + 1 != chosen {
                    crate::ledger::despawn(w, f.texture, log, "texture forms left out");
                    log.count("texture forms left out", 1);
                }
            }
            w.remove::<TextureForms>(texture);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::PassRegistry;
    use crate::pbr::{PbrMaterial, TextureRef};
    use crate::{ColorSpace, TexFormat, TexRole};

    fn texture(side: u32, value: u8) -> Texture {
        Texture {
            width: side,
            height: side,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![value; (side * side * 4) as usize]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        }
    }

    /// A bound 8 x 8 texture with a 2 x 2 form at level 1, named apart, and a material binding the texture.
    fn world() -> (World, Entity, Entity, Entity) {
        let mut w = World::new();
        let own = w.spawn();
        w.insert(own, texture(8, 200));
        w.insert(own, Name("aet000_001_a".into()));
        let low = w.spawn();
        w.insert(low, texture(2, 50));
        w.insert(low, Name("aet000_001_a_l".into()));
        w.insert(
            own,
            TextureForms {
                detail: 0,
                forms: vec![TextureForm {
                    detail: 1,
                    texture: low,
                }],
            },
        );
        let material = w.spawn();
        w.insert(
            material,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(own)),
                ..PbrMaterial::default()
            },
        );
        (w, own, low, material)
    }

    fn run(choice: TextureChoice, w: &mut World) -> PassLog {
        let mut log = PassLog::default();
        SelectTextureForm { choice }.run(w, &mut log).unwrap();
        log
    }

    /// The finest keeps the bound texture and despawns its form; a level keeps the coarsest form at it or finer, moved
    /// onto the bound entity (the material still binds it and the name stays the texture's).
    #[test]
    fn a_texture_keeps_the_form_chosen_and_its_bindings() {
        let (mut w, own, low, material) = world();
        let log = run(TextureChoice::Finest, &mut w);
        assert_eq!(log.counts["textures kept in their own form"], 1);
        assert_eq!(w.get::<Texture>(own).unwrap().width, 8);
        assert!(!w.is_alive(low) && !w.has::<TextureForms>(own));

        let (mut w, own, low, material_2) = world();
        let log = run(TextureChoice::AtMost(1), &mut w);
        assert_eq!(log.counts["textures kept in another form"], 1);
        let kept = w.get::<Texture>(own).unwrap();
        assert_eq!((kept.width, kept.levels[0][0]), (2, 50));
        assert_eq!(w.get::<Name>(own).unwrap().0, "aet000_001_a");
        assert!(!w.is_alive(low) && !w.has::<TextureForms>(own));
        let bound = w
            .get::<PbrMaterial>(material_2)
            .unwrap()
            .base_color_texture
            .as_ref()
            .map(|t| t.texture);
        assert_eq!(bound, Some(own));
        let _ = material;

        // no form at level 0 or finer than the own texture: the own texture is kept
        let (mut w, own, _, _) = world();
        run(TextureChoice::AtMost(0), &mut w);
        assert_eq!(w.get::<Texture>(own).unwrap().width, 8);
    }

    /// A form a material binds directly, a form without a texture, and a form of two textures are refused before
    /// anything changes.
    #[test]
    fn an_invalid_selection_changes_nothing() {
        let (mut w, own, low, material) = world();
        w.get_mut::<PbrMaterial>(material).unwrap().base_color_texture = Some(TextureRef::new(low));
        let err = SelectTextureForm {
            choice: TextureChoice::Finest,
        }
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
        assert!(err.contains("refers to form"), "{err}");
        assert!(w.is_alive(low) && w.has::<TextureForms>(own));

        let (mut w, own, low, _) = world();
        w.remove::<Texture>(low);
        assert_eq!(
            w.get::<TextureForms>(own).unwrap().verify(own, &w),
            Err(format!("form {low:?} holds no texture"))
        );

        let (mut w, _, low, _) = world();
        let other = w.spawn();
        w.insert(other, texture(4, 1));
        w.insert(
            other,
            TextureForms {
                detail: 0,
                forms: vec![TextureForm {
                    detail: 1,
                    texture: low,
                }],
            },
        );
        let err = SelectTextureForm {
            choice: TextureChoice::Finest,
        }
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
        assert!(err.contains("belongs to more than one texture"), "{err}");
    }

    /// The pass is made from its options and states them back; `form` is required.
    #[test]
    fn the_pass_states_its_options() {
        let mut reg = PassRegistry::new();
        reg.add(&SELECT_TEXTURE_FORM);
        for form in ["finest", "1"] {
            let options = Options::from([("form".to_owned(), form.to_owned())]);
            assert_eq!(
                reg.make("select-texture-form", &options).unwrap().options(),
                options
            );
        }
        assert!(reg.make("select-texture-form", &Options::new()).is_err());
    }
}
