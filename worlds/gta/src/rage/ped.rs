//! A ped's variation info (`CPedVariationInfo`, the `.ymt` beside a component ped's `.ydd` and `.yft`): which of the
//! twelve component slots the ped has, and for each the drawables it offers with their texture variations, as stored.
//!
//! Read from a decoded META root ([`crate::rage::meta::Meta::root`]) by field name, as [`crate::rage::map`] reads a
//! map; a missing field or one of another kind is an error. The drawables the info names are the entries of the ped's
//! drawable dictionary, which stores them by the hash of a name the game builds from the slot and the drawable's index
//! ([`entry_name`]): [`PedVariations::entries`] resolves each against the dictionary's hashes and refuses a drawable
//! with no entry or with two. The selection sets, per-component records (`compInfos`) and prop info stay in the META
//! tree; their counts are kept.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::rage::meta::{Struct, Value};
use crate::{Result, invalid};

/// The twelve component slots, in the order `availComp` lists them (the game's `PV_COMP_*`).
pub const COMPONENT_SLOTS: [&str; 12] = [
    "head", "berd", "hair", "uppr", "lowr", "hand", "feet", "teef", "accs", "task", "decl", "jbib",
];

/// The suffixes a drawable's entry name ends with, in the order tried: `r` and `u` on most, `m` on 100 of the 4,316
/// entries of the install's 563 component peds (all `uppr` and `lowr`).
pub const ENTRY_SUFFIXES: [char; 3] = ['r', 'u', 'm'];

/// A drawable's entry name in the ped's dictionary: `<slot>_<index, three digits>_<suffix>`.
pub fn entry_name(slot: usize, drawable: usize, suffix: char) -> Option<String> {
    Some(format!("{}_{drawable:03}_{suffix}", COMPONENT_SLOTS.get(slot)?))
}

fn err(detail: impl ToString) -> crate::Error {
    invalid("ped variations", 0, detail)
}

fn boolean(s: &Struct, name: &str) -> Result<bool> {
    match s.get(name)? {
        Value::Bool(b) => Ok(*b),
        _ => Err(err(format!("{name} is not a boolean"))),
    }
}

fn byte(s: &Struct, name: &str) -> Result<u8> {
    match s.get(name)? {
        Value::U8(b) => Ok(*b),
        _ => Err(err(format!("{name} is not a byte"))),
    }
}

fn structs<'a>(s: &'a Struct, name: &str) -> Result<Vec<&'a Struct>> {
    s.array(name)?
        .iter()
        .map(|v| match v {
            Value::Struct(s) => Ok(s),
            _ => Err(err(format!("{name} holds something other than structures"))),
        })
        .collect()
}

fn named(s: &Struct, want: &str) -> Result<()> {
    if primase_hash::joaat_exact(want.as_bytes()) == s.name {
        Ok(())
    } else {
        Err(err(format!("structure {:#010x} where {want} belongs", s.name)))
    }
}

/// One texture variation of a drawable: its id and distribution, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PedTexture {
    pub id: u8,
    pub distribution: u8,
}

/// One drawable a component offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PedDrawable {
    /// `propMask`, as stored (which props the drawable hides, by the game's bits).
    pub prop_mask: u8,
    pub alternatives: u8,
    pub textures: Vec<PedTexture>,
    pub owns_cloth: bool,
}

/// One component slot's data: how many texture variations it has and the drawables it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PedComponent {
    pub textures: u8,
    pub drawables: Vec<PedDrawable>,
}

/// A ped's variation info.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PedVariations {
    pub has_texture_variations: bool,
    pub has_drawable_variations: bool,
    pub has_low_lods: bool,
    pub is_super_lod: bool,
    /// Per slot ([`COMPONENT_SLOTS`]), the index into [`PedVariations::components`] of its data; `None` for a slot the
    /// ped does not have (stored as 255).
    pub slots: [Option<u8>; 12],
    pub components: Vec<PedComponent>,
    /// The DLC pack's name hash (0: none).
    pub dlc_name: u32,
    /// How many selection sets and per-component records the info holds (their contents stay in the META tree).
    pub selection_sets: usize,
    pub component_infos: usize,
}

/// A drawable of the info and its entry in the ped's dictionary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PedEntry {
    pub slot: usize,
    pub drawable: usize,
    pub suffix: char,
    /// The entry's index in the dictionary.
    pub entry: usize,
}

impl PedVariations {
    pub fn from_root(s: &Struct) -> Result<PedVariations> {
        named(s, "CPedVariationInfo")?;
        let avail = s.array("availComp")?;
        if avail.len() != COMPONENT_SLOTS.len() {
            return Err(err(format!("{} component slots, expected 12", avail.len())));
        }
        let mut slots = [None; 12];
        for (slot, v) in slots.iter_mut().zip(avail) {
            *slot = match v {
                Value::U8(255) => None,
                Value::U8(i) => Some(*i),
                _ => return Err(err("availComp holds something other than bytes")),
            };
        }
        let components = structs(s, "aComponentData3")?
            .into_iter()
            .map(|c| {
                named(c, "CPVComponentData")?;
                let drawables = structs(c, "aDrawblData3")?
                    .into_iter()
                    .map(|d| {
                        named(d, "CPVDrawblData")?;
                        let textures = structs(d, "aTexData")?
                            .into_iter()
                            .map(|t| {
                                named(t, "CPVTextureData")?;
                                Ok(PedTexture {
                                    id: byte(t, "texId")?,
                                    distribution: byte(t, "distribution")?,
                                })
                            })
                            .collect::<Result<_>>()?;
                        let cloth = match d.get("clothData")? {
                            Value::Struct(c) => boolean(c, "ownsCloth")?,
                            _ => return Err(err("clothData is not a structure")),
                        };
                        Ok(PedDrawable {
                            prop_mask: byte(d, "propMask")?,
                            alternatives: byte(d, "numAlternatives")?,
                            textures,
                            owns_cloth: cloth,
                        })
                    })
                    .collect::<Result<_>>()?;
                Ok(PedComponent {
                    textures: byte(c, "numAvailTex")?,
                    drawables,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let info = PedVariations {
            has_texture_variations: boolean(s, "bHasTexVariations")?,
            has_drawable_variations: boolean(s, "bHasDrawblVariations")?,
            has_low_lods: boolean(s, "bHasLowLODs")?,
            is_super_lod: boolean(s, "bIsSuperLOD")?,
            slots,
            components,
            dlc_name: s.hash("dlcName")?,
            selection_sets: s.array("aSelectionSets")?.len(),
            component_infos: s.array("compInfos")?.len(),
        };
        info.check()?;
        Ok(info)
    }

    /// Every slot names a component of the info, and every component belongs to one slot.
    fn check(&self) -> Result<()> {
        let mut used = vec![false; self.components.len()];
        for (slot, c) in self.slots.iter().enumerate() {
            let Some(c) = c else { continue };
            let seen = used.get_mut(usize::from(*c)).ok_or_else(|| {
                err(format!(
                    "slot {slot} names component {c} of {}",
                    self.components.len()
                ))
            })?;
            if std::mem::replace(seen, true) {
                return Err(err(format!("component {c} is the data of two slots")));
            }
        }
        if let Some(c) = used.iter().position(|u| !u) {
            return Err(err(format!("component {c} belongs to no slot")));
        }
        Ok(())
    }

    /// The component a slot has.
    pub fn component(&self, slot: usize) -> Option<&PedComponent> {
        self.components.get(usize::from((*self.slots.get(slot)?)?))
    }

    /// Each drawable of the info resolved to its entry in a dictionary whose entries have name hashes `hashes` (in
    /// the dictionary's order): by [`entry_name`] with each of [`ENTRY_SUFFIXES`]. A drawable with no entry or with
    /// more than one is an error, and so is an entry no drawable names.
    pub fn entries(&self, hashes: &[u32]) -> Result<Vec<PedEntry>> {
        let mut out = Vec::new();
        for slot in 0..COMPONENT_SLOTS.len() {
            let Some(component) = self.component(slot) else {
                continue;
            };
            for drawable in 0..component.drawables.len() {
                let mut found = ENTRY_SUFFIXES.iter().flat_map(|&suffix| {
                    let name = entry_name(slot, drawable, suffix)?;
                    let hash = primase_hash::joaat(name.as_bytes());
                    let entry = hashes.iter().position(|&h| h == hash)?;
                    Some(PedEntry {
                        slot,
                        drawable,
                        suffix,
                        entry,
                    })
                });
                let (Some(first), None) = (found.next(), found.next()) else {
                    return Err(err(format!(
                        "{} {drawable}: none or several of its names are entries of the dictionary",
                        COMPONENT_SLOTS.get(slot).copied().unwrap_or("?")
                    )));
                };
                out.push(first);
            }
        }
        if let Some(entry) = (0..hashes.len()).find(|e| !out.iter().any(|p| p.entry == *e)) {
            return Err(err(format!(
                "dictionary entry {entry} is no drawable of the info"
            )));
        }
        Ok(out)
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod tests {
    use super::*;
    use primase_hash::joaat_exact as h;

    fn st(name: &str, fields: Vec<(&str, Value)>) -> Struct {
        Struct {
            name: h(name.as_bytes()),
            fields: fields.into_iter().map(|(n, v)| (h(n.as_bytes()), v)).collect(),
        }
    }

    fn drawable(textures: &[u8]) -> Value {
        Value::Struct(st(
            "CPVDrawblData",
            vec![
                ("propMask", Value::U8(17)),
                ("numAlternatives", Value::U8(0)),
                (
                    "aTexData",
                    Value::Array(
                        textures
                            .iter()
                            .map(|&t| {
                                Value::Struct(st(
                                    "CPVTextureData",
                                    vec![("texId", Value::U8(t)), ("distribution", Value::U8(255))],
                                ))
                            })
                            .collect(),
                    ),
                ),
                (
                    "clothData",
                    Value::Struct(st("CPVClothData", vec![("ownsCloth", Value::Bool(false))])),
                ),
            ],
        ))
    }

    /// A ped with a head (two drawables) and an upper body (one), the slots between them absent.
    fn root() -> Struct {
        let component = |tex: u8, drawables: Vec<Value>| {
            Value::Struct(st(
                "CPVComponentData",
                vec![
                    ("numAvailTex", Value::U8(tex)),
                    ("aDrawblData3", Value::Array(drawables)),
                ],
            ))
        };
        let mut avail = vec![Value::U8(255); 12];
        avail[0] = Value::U8(0);
        avail[3] = Value::U8(1);
        st(
            "CPedVariationInfo",
            vec![
                ("bHasTexVariations", Value::Bool(true)),
                ("bHasDrawblVariations", Value::Bool(true)),
                ("bHasLowLODs", Value::Bool(false)),
                ("bIsSuperLOD", Value::Bool(false)),
                ("availComp", Value::Array(avail)),
                (
                    "aComponentData3",
                    Value::Array(vec![
                        component(4, vec![drawable(&[1, 1]), drawable(&[2])]),
                        component(1, vec![drawable(&[0])]),
                    ]),
                ),
                ("aSelectionSets", Value::Array(vec![])),
                ("compInfos", Value::Array(vec![])),
                ("dlcName", Value::Hash(0)),
            ],
        )
    }

    /// Hand-worked: slot 0 is `head`, slot 3 `uppr`; the head's drawables are `head_000_r` and `head_001_r`, the upper
    /// body's `uppr_000_m` (the one suffix in three that is not the head's).
    #[test]
    fn drawables_resolve_to_entries_by_slot_index_and_suffix() {
        let info = PedVariations::from_root(&root()).unwrap();
        assert_eq!(info.slots[0], Some(0));
        assert_eq!(info.slots[3], Some(1));
        assert_eq!(info.component(0).unwrap().drawables.len(), 2);
        assert_eq!(
            info.component(0).unwrap().drawables[0].textures[1],
            PedTexture {
                id: 1,
                distribution: 255
            }
        );
        let hash = |n: &str| primase_hash::joaat(n.as_bytes());
        let hashes = [hash("uppr_000_m"), hash("head_001_r"), hash("head_000_r")];
        let entries = info.entries(&hashes).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.slot, e.drawable, e.suffix, e.entry))
                .collect::<Vec<_>>(),
            [(0, 0, 'r', 2), (0, 1, 'r', 1), (3, 0, 'm', 0)]
        );
        // a drawable without an entry, an entry no drawable names, and a drawable with two entries are errors
        assert!(info.entries(&hashes[..2]).is_err());
        let extra = [hashes[0], hashes[1], hashes[2], hash("feet_000_r")];
        assert!(
            info.entries(&extra)
                .unwrap_err()
                .to_string()
                .contains("no drawable")
        );
        let both = [hashes[0], hashes[1], hashes[2], hash("head_000_u")];
        assert!(info.entries(&both).unwrap_err().to_string().contains("several"));
    }

    /// A slot naming a component that is not there, or two slots sharing one, or a component no slot has, is refused.
    #[test]
    fn slots_and_components_must_agree() {
        let with_slot = |slot: usize, v: u8| {
            let mut r = root();
            if let Some((_, Value::Array(a))) = r.fields.iter_mut().find(|(n, _)| *n == h(b"availComp")) {
                a[slot] = Value::U8(v);
            }
            r
        };
        assert!(
            PedVariations::from_root(&with_slot(5, 9))
                .unwrap_err()
                .to_string()
                .contains("names component 9")
        );
        assert!(
            PedVariations::from_root(&with_slot(5, 0))
                .unwrap_err()
                .to_string()
                .contains("two slots")
        );
        assert!(
            PedVariations::from_root(&with_slot(3, 255))
                .unwrap_err()
                .to_string()
                .contains("belongs to no slot")
        );
        let mut other = root();
        other.name = h(b"CMapData");
        assert!(PedVariations::from_root(&other).is_err());
    }
}
