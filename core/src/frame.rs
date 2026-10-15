//! The world frame: metres, right-handed, +Y up, -Z north (glTF's frame, and skate-scene v1's).
//!
//! The world is always in this frame. A reader converts from its game's frame on the way in and a writer to its
//! target's on the way out, with the conversions here (signed axis permutations, unit scale, handedness and the
//! winding flip that goes with a mirror).
