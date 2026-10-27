//! A skeleton asset (`AntSkeletonAsset`, the EBX documents `animation/dingo/*_skeleton`) read into its joints
//! ([`read`]): facts of the hierarchy only.
//!
//! The asset's `Joints` array holds, for each joint, the offset of its name in the asset's `NameTable` (a table of
//! zero-terminated names), the FNV-1 hash of that name (`NameHash`), its parent, first child and next sibling joint
//! (-1 for none) and a type. Every joint's hash is checked against its name and every parent, child and sibling
//! against the joint count, with no cycle through parents, as the reader's refusals. The asset states no rest
//! transforms: those are in the rig's default pose, keyed by degree-of-freedom ids whose naming is not read, so a
//! joint here has none.

use crate::ebx::{Document, Object, Value};
use crate::{Error, Result};

/// The root type of the skeleton asset document.
pub const ROOT: &str = "AntSkeletonAsset";

/// One joint of a skeleton asset, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkeletonJoint {
    pub name: String,
    /// The FNV-1 hash of the name, as stored (checked).
    pub name_hash: u32,
    pub parent: Option<u32>,
    pub child: Option<u32>,
    pub sibling: Option<u32>,
    /// The joint's `Type`, as stored.
    pub kind: i64,
}

/// FNV-1 (32 bit) of `name`, the hash the asset stores of a joint's name.
pub fn name_hash(name: &str) -> u32 {
    name.bytes()
        .fold(0x811c_9dc5u32, |h, b| h.wrapping_mul(0x0100_0193) ^ u32::from(b))
}

fn bad(detail: impl Into<String>) -> Error {
    Error::Format {
        what: "skeleton asset",
        offset: 0,
        detail: detail.into(),
    }
}

fn int(v: &Value, what: &str) -> Result<i64> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::UInt(u) => i64::try_from(*u).map_err(|_| bad(format!("{what} {u} out of range"))),
        other => Err(bad(format!("{what} is {other:?}, not an integer"))),
    }
}

fn array<'a>(o: &'a Object, field: &str) -> Result<&'a [Value]> {
    match o.field(field) {
        Some(Value::Array(v, _)) => Ok(v),
        _ => Err(bad(format!("no {field} array"))),
    }
}

/// The joints of the document if it is a skeleton asset, `None` if its root type is another; a skeleton asset that
/// does not hold to the module's checks is an error naming the first that fails.
pub fn read(doc: &Document) -> Result<Option<(String, Vec<SkeletonJoint>)>> {
    if doc.root_type != ROOT {
        return Ok(None);
    }
    let root = doc
        .instances
        .first()
        .and_then(|i| i.object.as_ref())
        .ok_or_else(|| bad("no root instance"))?;
    let name = match root.field("Name") {
        Some(Value::Str(s)) => String::from_utf8_lossy(s).into_owned(),
        _ => return Err(bad("no Name")),
    };
    let table: Vec<u8> = array(root, "NameTable")?
        .iter()
        .map(|v| int(v, "NameTable byte").and_then(|b| u8::try_from(b).map_err(|_| bad("NameTable byte"))))
        .collect::<Result<_>>()?;
    let stored = array(root, "Joints")?;
    let count = stored.len();
    let link = |o: &Object, field: &str, joint: usize| -> Result<Option<u32>> {
        let at = int(
            o.field(field)
                .ok_or_else(|| bad(format!("joint {joint}: no {field}")))?,
            field,
        )?;
        match at {
            -1 => Ok(None),
            0.. if (at as usize) < count => Ok(Some(at as u32)),
            _ => Err(bad(format!("joint {joint}: {field} {at} of {count} joints"))),
        }
    };
    let mut joints = Vec::with_capacity(count);
    for (i, v) in stored.iter().enumerate() {
        let Value::Object(o) = v else {
            return Err(bad(format!("joint {i} is not an object")));
        };
        let at = usize::try_from(int(
            o.field("Name")
                .ok_or_else(|| bad(format!("joint {i}: no Name")))?,
            "Name",
        )?)
        .map_err(|_| bad(format!("joint {i}: name offset")))?;
        let end = table
            .get(at..)
            .and_then(|t| t.iter().position(|&b| b == 0))
            .ok_or_else(|| bad(format!("joint {i}: name at {at} of a table of {}", table.len())))?;
        let joint_name = String::from_utf8_lossy(&table[at..at + end]).into_owned();
        let hash = u32::try_from(int(
            o.field("NameHash")
                .ok_or_else(|| bad(format!("joint {i}: no NameHash")))?,
            "NameHash",
        )?)
        .map_err(|_| bad(format!("joint {i}: NameHash")))?;
        if hash != name_hash(&joint_name) {
            return Err(bad(format!(
                "joint {i} {joint_name:?}: hash {hash:#x}, not the FNV-1 of its name"
            )));
        }
        joints.push(SkeletonJoint {
            name: joint_name,
            name_hash: hash,
            parent: link(o, "Parent", i)?,
            child: link(o, "Child", i)?,
            sibling: link(o, "Sibling", i)?,
            kind: int(
                o.field("Type")
                    .ok_or_else(|| bad(format!("joint {i}: no Type")))?,
                "Type",
            )?,
        });
    }
    verify_hierarchy(&joints).map_err(bad)?;
    Ok(Some((name, joints)))
}

/// No cycle through the parents.
pub fn verify_hierarchy(joints: &[SkeletonJoint]) -> std::result::Result<(), String> {
    for (i, _) in joints.iter().enumerate() {
        let mut at = joints[i].parent;
        let mut steps = 0usize;
        while let Some(p) = at {
            steps += 1;
            if steps > joints.len() {
                return Err(format!(
                    "joint {i}: the parents lead back to a joint already passed"
                ));
            }
            at = joints
                .get(p as usize)
                .ok_or_else(|| format!("joint {i}: parent {p}"))?
                .parent;
        }
    }
    Ok(())
}
