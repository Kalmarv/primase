//! `lower-reskate-unlit{as=matte}`: unlit materials (`Shading::Unlit`: the colour drawn is the base colour, whatever
//! lights the scene) as lit materials skate draws, before `raise-skate`, which draws every surface lit. The form is the
//! program's choice, stated: `matte`, the base colour (factor and texture) as a fully rough dielectric (roughness 1,
//! metallic 0), the colour lit by the scene and never shiny; the slots an unlit material does not draw
//! (metallic-roughness, normal, occlusion, emission, specular) are dropped. `as=emission` (the colour drawn whatever
//! the light) is refused: skate-scene has no emissive surface the writer can write, so it would draw black; it is a
//! form to add when the writer maps one. Each material lowered is reported by name as an approximation.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{PbrMaterial, Shading};
use primase_core::{Entity, Name, World};

/// The lit form [`LowerReskateUnlit`] gives an unlit material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlitAs {
    /// The base colour, fully rough and dielectric.
    Matte,
}

/// See the module documentation. Made by [`LowerReskateUnlit::new`]; from options by the same call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerReskateUnlit {
    form: UnlitAs,
}

impl LowerReskateUnlit {
    pub fn new(form: UnlitAs) -> LowerReskateUnlit {
        LowerReskateUnlit { form }
    }

    pub fn form(&self) -> UnlitAs {
        self.form
    }

    /// From its options: `as=matte`, required (`emission` refused: module documentation).
    pub fn from_options(o: &Options) -> Result<LowerReskateUnlit, String> {
        if let Some(k) = o.keys().find(|k| k.as_str() != "as") {
            return Err(format!("unknown option {k}"));
        }
        match o.get("as").map(String::as_str) {
            Some("matte") => Ok(LowerReskateUnlit::new(UnlitAs::Matte)),
            Some("emission") => Err(
                "as=emission: skate-scene has no emissive surface the writer can write (it would draw black); as=matte"
                    .into(),
            ),
            Some(v) => Err(format!("as={v}: matte")),
            None => Err("as is required: matte (the base colour lit, fully rough)".into()),
        }
    }
}

/// [`LowerReskateUnlit::from_options`] as a pass factory.
pub fn lower_reskate_unlit(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(LowerReskateUnlit::from_options(o)?))
}

impl Pass for LowerReskateUnlit {
    fn name(&self) -> &'static str {
        "lower-reskate-unlit"
    }
    fn options(&self) -> Options {
        Options::from([(
            "as".to_owned(),
            match self.form {
                UnlitAs::Matte => "matte",
            }
            .to_owned(),
        )])
    }
    fn requires(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn approximates(&self) -> &'static str {
        "unlit materials drawn lit, fully rough"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let unlit: Vec<Entity> = w
            .iter::<PbrMaterial>()
            .filter(|(_, m)| m.shading == Shading::Unlit)
            .map(|(e, _)| e)
            .collect();
        for e in unlit {
            let name = w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone());
            let m = w.get_mut::<PbrMaterial>(e).expect("listed");
            match self.form {
                UnlitAs::Matte => {
                    m.shading = Shading::Lit;
                    m.metallic = 0.0;
                    m.roughness = 1.0;
                    m.metallic_roughness_texture = None;
                    m.normal_texture = None;
                    m.occlusion_texture = None;
                    m.specular = None;
                    m.emissive_texture = None;
                    m.emissive = [0.0; 3];
                    log.approximate(
                        &format!("unlit material drawn matte (lit, fully rough): {name}"),
                        1,
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pbr::TextureRef;

    /// matte: lit, roughness 1, metallic 0, the base colour kept, the other slots dropped, reported; a lit material
    /// is left as it is.
    #[test]
    fn matte_keeps_the_base_colour_fully_rough() {
        let mut w = World::new();
        let tex = w.spawn();
        let sign = w.spawn();
        w.insert(sign, Name("sign".into()));
        w.insert(
            sign,
            PbrMaterial {
                shading: Shading::Unlit,
                base_color: [0.2, 0.4, 0.6, 0.5],
                base_color_texture: Some(TextureRef::new(tex)),
                normal_texture: Some(TextureRef::new(tex)),
                metallic: 1.0,
                roughness: 0.1,
                ..PbrMaterial::default()
            },
        );
        let lit = w.spawn();
        w.insert(lit, PbrMaterial::default());
        let mut log = PassLog::default();
        LowerReskateUnlit::new(UnlitAs::Matte)
            .run(&mut w, &mut log)
            .unwrap();
        let m = w.get::<PbrMaterial>(sign).unwrap();
        assert_eq!(m.shading, Shading::Lit);
        assert_eq!((m.metallic, m.roughness), (0.0, 1.0));
        assert_eq!(m.base_color, [0.2, 0.4, 0.6, 0.5]);
        assert_eq!(m.base_color_texture.map(|r| r.texture), Some(tex));
        assert!(m.normal_texture.is_none());
        assert_eq!(*w.get::<PbrMaterial>(lit).unwrap(), PbrMaterial::default());
        assert_eq!(
            log.approximations["unlit material drawn matte (lit, fully rough): sign"],
            1
        );
    }

    /// The form is required and printed; emission is refused, naming why.
    #[test]
    fn the_form_is_required_and_emission_refused() {
        assert!(LowerReskateUnlit::from_options(&Options::new()).is_err());
        assert!(LowerReskateUnlit::from_options(&Options::from([("as".into(), "glow".into())])).is_err());
        let e =
            LowerReskateUnlit::from_options(&Options::from([("as".into(), "emission".into())])).unwrap_err();
        assert!(e.contains("no emissive surface"), "{e}");
        let p = LowerReskateUnlit::from_options(&Options::from([("as".into(), "matte".into())])).unwrap();
        assert_eq!(LowerReskateUnlit::from_options(&p.options()).unwrap(), p);
    }
}
