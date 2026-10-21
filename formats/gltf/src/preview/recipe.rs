//! The recipe: the choices a site made from a manifest, handed back to a program that replays them as passes.
//!
//! A recipe holds only ids the manifest states ([`Choice::id`](super::manifest::Choice::id) and one of its options),
//! so it names no entity and needs no world to be written, stored or compared. [`Recipe::pipeline`] turns it into the
//! passes that apply it: `select-outfit` with exactly these choices, the same pass a program runs by hand
//! (`select-outfit{choices=outfit:coat;hat:on}`). A choice the recipe does not make stays open, resolved by whatever
//! the program runs after (`resolve-state`, with its own defaults and its own named choices): nothing is chosen for
//! the site.

use std::collections::BTreeMap;

use primase_core::pass::Pipeline;
use primase_core::state::SelectOutfit;
use serde::{Deserialize, Serialize};

use super::manifest::{Manifest, VERSION};
use crate::{Error, Result};

/// `format` of every recipe.
pub const FORMAT: &str = "primase.preview-recipe";

/// The chosen options, by choice id, for the manifest version they were chosen from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// [`FORMAT`].
    pub format: String,
    /// The version of the manifest the ids are from ([`VERSION`]).
    pub manifest_version: u32,
    /// Choice id -> option id.
    pub choices: BTreeMap<String, String>,
}

fn invalid(item: impl Into<String>, detail: impl Into<String>) -> Error {
    Error::Invalid {
        item: item.into(),
        detail: detail.into(),
    }
}

impl Recipe {
    /// An empty recipe: nothing chosen.
    pub fn new() -> Recipe {
        Recipe {
            format: FORMAT.to_owned(),
            manifest_version: VERSION,
            choices: BTreeMap::new(),
        }
    }

    /// Choose `option` for `choice`, replacing an earlier choice of it.
    pub fn choose(&mut self, choice: impl Into<String>, option: impl Into<String>) -> &mut Recipe {
        self.choices.insert(choice.into(), option.into());
        self
    }

    /// The recipe as JSON, indented.
    pub fn to_json(&self) -> Result<String> {
        let mut text = serde_json::to_string_pretty(self).map_err(|e| invalid("recipe", e.to_string()))?;
        text.push('\n');
        Ok(text)
    }

    /// Read a recipe, refusing another format or manifest version and unknown fields.
    pub fn from_slice(bytes: &[u8]) -> Result<Recipe> {
        let r: Recipe = serde_json::from_slice(bytes).map_err(|e| invalid("recipe", e.to_string()))?;
        if r.format != FORMAT {
            return Err(invalid("recipe", format!("format {:?}, not {FORMAT}", r.format)));
        }
        if r.manifest_version != VERSION {
            return Err(invalid(
                "recipe",
                format!(
                    "made from manifest version {}, this reader knows {VERSION}",
                    r.manifest_version
                ),
            ));
        }
        Ok(r)
    }

    /// Whether every choice is one the manifest states, with one of its options, naming the first that is not.
    pub fn check(&self, manifest: &Manifest) -> Result<()> {
        for (choice, option) in &self.choices {
            let c = manifest.choice(choice).ok_or_else(|| {
                invalid(
                    format!("recipe choice {choice}"),
                    "the manifest has no such choice",
                )
            })?;
            if !c.options.contains(option) {
                return Err(invalid(
                    format!("recipe choice {choice}"),
                    format!("{option:?} is not one of its options ({})", c.options.join(", ")),
                ));
            }
        }
        Ok(())
    }

    /// The manifest's choices this recipe does not make, in the manifest's order.
    pub fn undecided<'m>(&self, manifest: &'m Manifest) -> Vec<&'m str> {
        manifest
            .choices
            .iter()
            .map(|c| c.id.as_str())
            .filter(|id| !self.choices.contains_key(*id))
            .collect()
    }

    /// The passes that apply the recipe to a world read and prepared as the manifest's was: `select-outfit` with its
    /// choices, nothing for an empty recipe. Checked against the manifest first ([`Recipe::check`]).
    pub fn pipeline(&self, manifest: &Manifest) -> Result<Pipeline> {
        self.check(manifest)?;
        let mut pipeline = Pipeline::default();
        if !self.choices.is_empty() {
            pipeline.passes.push(Box::new(SelectOutfit {
                choices: self.choices.clone(),
            }));
        }
        Ok(pipeline)
    }
}

impl Default for Recipe {
    fn default() -> Recipe {
        Recipe::new()
    }
}
