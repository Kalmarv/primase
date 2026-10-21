//! `set-material-modes{alpha=opaque|keep,double-sided=false|true|keep}` (both required): every `pbr.material` given the
//! stated alpha mode and double-sidedness, for a destination that draws with one opaque material (`pack-texture-atlas`
//! refuses materials that differ in either).
//!
//! `alpha=opaque` makes a masked or blended material opaque; `keep` leaves the alpha mode. `double-sided=false|true`
//! sets the flag on every material; `keep` leaves it. A material that already has the stated mode is not touched.
//!
//! Reported: each material changed, by its name (its entity when it has none) and what changed, counted; and as
//! approximations, by kind, because what is drawn changes: a masked material made opaque draws the texels its cut-out
//! discarded, a blended one draws opaque, a double-sided one made single-sided no longer draws its back faces, and a
//! single-sided one made double-sided draws them (lit with the flipped normal).

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial};
use primase_core::{Name, World};

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetMaterialModes {
    opaque: bool,
    double_sided: Option<bool>,
}

impl SetMaterialModes {
    /// Make every material opaque when `opaque`, and set its double-sidedness when `double_sided` is `Some`.
    pub fn new(opaque: bool, double_sided: Option<bool>) -> SetMaterialModes {
        SetMaterialModes { opaque, double_sided }
    }

    /// From its options, both required: `alpha=opaque|keep`, `double-sided=false|true|keep`.
    pub fn from_options(o: &Options) -> Result<SetMaterialModes, String> {
        if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "alpha" | "double-sided")) {
            return Err(format!("unknown option {k}"));
        }
        let opaque = match o.get("alpha").map(String::as_str) {
            Some("opaque") => true,
            Some("keep") => false,
            Some(v) => return Err(format!("alpha={v}: opaque or keep")),
            None => return Err("set-material-modes needs alpha=opaque|keep".into()),
        };
        let double_sided = match o.get("double-sided").map(String::as_str) {
            Some("true") => Some(true),
            Some("false") => Some(false),
            Some("keep") => None,
            Some(v) => return Err(format!("double-sided={v}: false, true or keep")),
            None => return Err("set-material-modes needs double-sided=false|true|keep".into()),
        };
        Ok(SetMaterialModes::new(opaque, double_sided))
    }
}

impl Pass for SetMaterialModes {
    fn name(&self) -> &'static str {
        "set-material-modes"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "alpha".to_owned(),
                if self.opaque { "opaque" } else { "keep" }.to_owned(),
            ),
            (
                "double-sided".to_owned(),
                self.double_sided
                    .map_or("keep", |d| if d { "true" } else { "false" })
                    .to_owned(),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn approximates(&self) -> &'static str {
        "materials drawn with another alpha mode or double-sidedness than the source's (cut-out texels, translucency \
         and back faces change)"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let names: Vec<(primase_core::Entity, String)> = w
            .iter::<PbrMaterial>()
            .map(|(e, _)| {
                (
                    e,
                    w.get::<Name>(e).map_or_else(|| format!("{e:?}"), |n| n.0.clone()),
                )
            })
            .collect();
        for (e, name) in names {
            let m = w.get_mut::<PbrMaterial>(e).expect("listed");
            if self.opaque && m.alpha_mode != AlphaMode::Opaque {
                let (what, kind) = match m.alpha_mode {
                    AlphaMode::Mask { .. } => {
                        ("masked", "masked materials made opaque (cut-out texels drawn)")
                    }
                    _ => (
                        "blended",
                        "blended materials made opaque (drawn without translucency)",
                    ),
                };
                m.alpha_mode = AlphaMode::Opaque;
                log.count(&format!("material {name}: {what} made opaque"), 1);
                log.approximate(kind, 1);
            }
            if let Some(d) = self.double_sided
                && m.double_sided != d
            {
                m.double_sided = d;
                if d {
                    log.count(&format!("material {name}: made double-sided"), 1);
                    log.approximate("single-sided materials made double-sided (back faces drawn)", 1);
                } else {
                    log.count(&format!("material {name}: made single-sided"), 1);
                    log.approximate(
                        "double-sided materials made single-sided (back faces not drawn)",
                        1,
                    );
                }
            }
        }
        Ok(())
    }
}
