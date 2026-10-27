//! The PhysicsResource layout ReSkate Studio 20261012 writes (its compile-map's collision, `reskate_cli`
//! FUN_140160250): version 2, the word at 0x08 set, its collision template's 0x4C-byte body, and its template's part
//! ids. The format itself (reading, writing, encoding a triangle mesh as a shape) is `frostbite_formats::physics`;
//! this is only what Studio puts in the fields the format leaves to the writer.

use frostbite_formats::physics::{Part, PhysicsResource, Shape};

/// The 0x4C-byte body Studio writes into every PhysicsResource (its collision template's): a unit quaternion,
/// zeros, 1.0, 1, 0, -1, -1, -1, 0, 0, 0x100.
pub const STUDIO_BODY: [u8; 0x4C] = {
    let mut b = [0u8; 0x4C];
    let words: [(usize, u32); 7] = [
        (0x0C, 0x3F80_0000),
        (0x28, 0x3F80_0000),
        (0x2C, 1),
        (0x34, 0xBF80_0000),
        (0x38, 0xBF80_0000),
        (0x3C, 0xBF80_0000),
        (0x48, 0x100),
    ];
    let mut i = 0;
    while i < words.len() {
        let (at, w) = words[i];
        let le = w.to_le_bytes();
        b[at] = le[0];
        b[at + 1] = le[1];
        b[at + 2] = le[2];
        b[at + 3] = le[3];
        i += 1;
    }
    b
};

/// The three ids Studio writes into each part (its template's, `DAT_1404edca0`). The game's own parts carry others
/// (`0x859F4B83, 0x2AF24804, 0x859F4B83` on the build kit's).
pub const STUDIO_PART_IDS: [u32; 3] = [0x859F_4B83, 0xE668_6C9C, 0xBD95_7D4A];

/// A resource in Studio's layout: each shape a triangle-mesh part at its position with its packed material.
pub fn studio_resource(shapes: Vec<(Shape, [f32; 3], u32)>) -> PhysicsResource {
    let mut out = PhysicsResource {
        version: 2,
        word08: 1,
        body: STUDIO_BODY.to_vec(),
        shapes: Vec::with_capacity(shapes.len()),
        parts: Vec::with_capacity(shapes.len()),
    };
    for (i, (shape, position, material)) in shapes.into_iter().enumerate() {
        out.shapes.push(shape);
        out.parts
            .push(Part::mesh(i as u32, position, STUDIO_PART_IDS, material));
    }
    out
}
