//! The names of the module's operation functions (`primase_shader::ops`): the name of an image atomic function says the
//! operation, and no other function name is taken for one.
use primase_shader::ops::{IMAGE_ATOMIC_OPS, image_atomic_name, image_atomic_op, is_image_atomic};

#[test]
fn an_image_atomic_functions_name_says_its_operation() {
    for op in IMAGE_ATOMIC_OPS {
        for dim in ["1d", "2d", "3d", "cube"] {
            for arrayed in [false, true] {
                for signed in [false, true] {
                    let name = image_atomic_name(op, dim, arrayed, signed);
                    assert_eq!(image_atomic_op(&name), Some(op), "{name}");
                    assert!(name.ends_with(if signed { "_i32" } else { "_u32" }), "{name}");
                    assert_eq!(name.contains("_array_"), arrayed, "{name}");
                }
            }
        }
    }
    assert_eq!(
        image_atomic_name("or", "2d", true, false),
        "image_atomic_or_2d_array_u32"
    );
}

#[test]
fn no_other_function_is_an_image_atomic() {
    for name in [
        "main",
        "trace_ray",
        "image_atomic",
        "image_atomic_",
        "image_atomic_compare_2d_u32",
        "my_image_atomic_add_2d_u32",
    ] {
        assert!(!is_image_atomic(name), "{name}");
    }
}
