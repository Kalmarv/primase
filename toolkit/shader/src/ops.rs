//! Operations the module's statements cannot say, asked for by calling functions of the module that have no body (the
//! way a ray tracing program asks for the pipeline's operations: [`crate::rt`]). The call is the operation, its
//! arguments the operands, and a consumer that runs the operation reads the calls. A function's body is a return of a
//! zero value (or empty), so nothing that runs the module as written runs the operation, and the WGSL writer refuses
//! a module that has one, by name.
//!
//! # Image atomics with the old value
//!
//! naga's `Statement::ImageAtomic` has no result: the old word of the texel is not available to the program. A program
//! that reads it (`imm_atomic_*` on a typed texture view in DXBC, `AtomicBinOp` with a used result in DXIL) calls
//! `image_atomic_<op>_<dim>[_array]_<scalar>` instead:
//!
//! | argument | |
//! |----------|---|
//! | the image | the storage texture (a global of the image type) |
//! | the coordinate | `i32`, or a vector of 2 or 3 of them for a 2D or 3D texture |
//! | the layer | `i32`, of an arrayed texture only |
//! | the value | `u32` or `i32` as the scalar says |
//!
//! and the result is the word as it was before the operation, of the same scalar. The operations are
//! [`IMAGE_ATOMIC_OPS`]; `<dim>` is `1d`, `2d`, `3d` or `cube`. The names say the shape of the call so that one name
//! is one signature; the operation and the scalar are all a consumer needs from them ([`image_atomic_op`]).
//!
//! When naga's statement has a result, a lifter switches to it and these functions go away.

/// What an image atomic function's name starts with.
pub const IMAGE_ATOMIC_PREFIX: &str = "image_atomic_";

/// The atomic operations of an image atomic function, by name.
pub const IMAGE_ATOMIC_OPS: [&str; 8] = ["add", "sub", "and", "or", "xor", "min", "max", "exchange"];

/// The name of the function that asks for image atomic operation `op` (one of [`IMAGE_ATOMIC_OPS`]) on a texture of
/// dimension `dim` (`1d`, `2d`, `3d`, `cube`), arrayed or not, on `u32` words or `i32` ones.
pub fn image_atomic_name(op: &str, dim: &str, arrayed: bool, signed: bool) -> String {
    format!(
        "{IMAGE_ATOMIC_PREFIX}{op}_{dim}{}_{}",
        if arrayed { "_array" } else { "" },
        if signed { "i32" } else { "u32" }
    )
}

/// The operation an image atomic function does, from its name; `None` for any other function.
pub fn image_atomic_op(name: &str) -> Option<&'static str> {
    let rest = name.strip_prefix(IMAGE_ATOMIC_PREFIX)?;
    let op = rest.split('_').next()?;
    IMAGE_ATOMIC_OPS.iter().copied().find(|o| *o == op)
}

/// Whether the name of a function is an image atomic function's.
pub fn is_image_atomic(name: &str) -> bool {
    image_atomic_op(name).is_some()
}
