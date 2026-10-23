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

use primase_core::pass::{Choice, Given, OptionSpec, Options, Pass, PassLog, PassSpec};
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
    fn from_given(g: &Given) -> SetMaterialModes {
        let opaque = g.choice::<Alpha>("alpha") == Alpha::Opaque;
        let double_sided = match g.choice::<Sides>("double-sided") {
            Sides::False => Some(false),
            Sides::True => Some(true),
            Sides::Keep => None,
        };
        SetMaterialModes::new(opaque, double_sided)
    }
}

/// Option `alpha`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Alpha {
    Opaque,
    Keep,
}

impl Choice for Alpha {
    const VALUES: &'static [(&'static str, Self)] = &[("opaque", Alpha::Opaque), ("keep", Alpha::Keep)];
}

/// Option `double-sided`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sides {
    False,
    True,
    Keep,
}

impl Choice for Sides {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("false", Sides::False),
        ("true", Sides::True),
        ("keep", Sides::Keep),
    ];
}

/// `set-material-modes`' declaration ([`primase_core::pass::PassRegistry::add`]): what a destination that draws one
/// material needs is the program's, both options required.
pub const SET_MATERIAL_MODES: PassSpec = PassSpec {
    name: "set-material-modes",
    options: &[
        OptionSpec::choice::<Alpha>("alpha", "every material made opaque, or its alpha mode kept"),
        OptionSpec::choice::<Sides>(
            "double-sided",
            "every material single-sided, double-sided, or as it is",
        ),
    ],
    make: |g| Ok(Box::new(SetMaterialModes::from_given(g))),
};

impl Pass for SetMaterialModes {
    fn name(&self) -> &'static str {
        "set-material-modes"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "alpha".to_owned(),
                if self.opaque { Alpha::Opaque } else { Alpha::Keep }
                    .text()
                    .to_owned(),
            ),
            (
                "double-sided".to_owned(),
                match self.double_sided {
                    Some(false) => Sides::False,
                    Some(true) => Sides::True,
                    None => Sides::Keep,
                }
                .text()
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
