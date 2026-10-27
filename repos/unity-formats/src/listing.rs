//! A player's objects listed by class and name from its object tables: the class of each object is in the table, its
//! name is read from the object's first bytes ([`Player::field`]: the fields before `m_Name` are walked, nothing
//! after it), so listing every mesh, texture and material of a player decodes none of them whole.

use std::collections::BTreeMap;

use crate::Result;
use crate::player::{ObjRef, Player};

/// One object of a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectEntry {
    pub object: ObjRef,
    pub class_id: i32,
    /// The class name from the type tree package; `None` for a class id the package lacks for this Unity version.
    pub class: Option<String>,
    /// Its `m_Name`; `None` for a class without one, or one whose fields before it do not decode (counted in
    /// [`Listing::unnamed`]). An object stored with an empty name has `Some("")`.
    pub name: Option<String>,
    /// The object's size in the file, in bytes.
    pub size: u32,
}

/// What [`list`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// The objects of the classes asked for, by file name then path id.
    pub entries: Vec<ObjectEntry>,
    /// Objects left without a name, by why: `class has no m_Name`, or the decoding error.
    pub unnamed: BTreeMap<String, u64>,
}

/// The objects of `player` whose class is one of `classes` (class names such as `Mesh`; every object when `None`),
/// each with its name where its class has one. A class name the package does not know for a file's Unity version
/// matches nothing there. Objects of a class id the package lacks are listed only when every object is asked for.
pub fn list(player: &Player, classes: Option<&[&str]>) -> Result<Listing> {
    let mut listing = Listing::default();
    for (file, f) in &player.files {
        for o in &f.objects {
            let object = ObjRef {
                file: file.clone(),
                path_id: o.path_id,
            };
            let class = player.class_name(&object).map(str::to_owned);
            if let Some(wanted) = classes
                && !class.as_deref().is_some_and(|c| wanted.contains(&c))
            {
                continue;
            }
            let name = match class.as_deref() {
                // no layout for the class: nothing to walk
                None => {
                    *listing
                        .unnamed
                        .entry("class unknown to the type tree package".into())
                        .or_default() += 1;
                    None
                }
                Some(_) => match player.field(&object, "m_Name") {
                    Ok(Some(v)) => v.as_str().map(str::to_owned),
                    Ok(None) => {
                        *listing.unnamed.entry("class has no m_Name".into()).or_default() += 1;
                        None
                    }
                    Err(e) => {
                        *listing.unnamed.entry(e.to_string()).or_default() += 1;
                        None
                    }
                },
            };
            listing.entries.push(ObjectEntry {
                object,
                class_id: o.class_id,
                class,
                name,
                size: o.size,
            });
        }
    }
    Ok(listing)
}
