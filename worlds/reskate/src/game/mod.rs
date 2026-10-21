//! The game's own assets, read from its data folder (`Data`, through [`primase_frostbite::folder`]): what skate. itself
//! poses and animates characters with, for programs that preview or build for it.
//!
//! Frame: the game's data is in core's frame (metres, right-handed, +Y up), read with no conversion: primase's
//! scene interchange is in that frame (`crate::components`), Studio compiles its positions unchanged into the MeshSets
//! the game draws (the vertex-blend gate, `tests/postcompile_vbrecord.rs`), and maps built that way are drawn
//! unmirrored. A Frostbite `LinearTransform` (`right`, `up`, `forward`, `trans`: the rows of a row-vector matrix) is
//! the affine map whose columns are those four vectors.

pub mod clip;
pub mod eclipse;
pub mod rig;
pub mod skeleton;

use primase_core::Transform;
use primase_frostbite::ebx::Value;

/// A `Vec3` object's `x`, `y`, `z`, as f32 (the game stores f32; the EBX reader widens them).
fn vec3(v: &Value, what: &str) -> Result<[f32; 3], String> {
    let Value::Object(o) = v else {
        return Err(format!("{what} is not a Vec3"));
    };
    let c = |k: &str| match o.field(k) {
        Some(Value::Float(x)) => Ok(*x as f32),
        _ => Err(format!("{what}.{k} is not a float")),
    };
    Ok([c("x")?, c("y")?, c("z")?])
}

/// A `LinearTransform` object as the affine map it is (see the module documentation).
pub(crate) fn linear_transform(v: &Value, what: &str) -> Result<Transform, String> {
    let Value::Object(o) = v else {
        return Err(format!("{what} is not a LinearTransform"));
    };
    let get = |k: &str| {
        o.field(k)
            .ok_or_else(|| format!("{what} has no {k}"))
            .and_then(|v| vec3(v, &format!("{what}.{k}")))
    };
    Ok(Transform {
        cols: [get("right")?, get("up")?, get("forward")?, get("trans")?],
    })
}
