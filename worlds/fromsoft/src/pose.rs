//! Pure pose arithmetic shared by the reader and the passes, with no I/O: they both use it and never each other.
use primase_core::Transform;
use primase_core::frame::{Axis, SourceFrame};

/// FromSoft's source frame: x, y and z negated, in metres.
pub(crate) const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);

/// `p` transformed by `t` (a point: rotation, scale and translation).
pub(crate) fn point(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| t.cols[0][i] * p[0] + t.cols[1][i] * p[1] + t.cols[2][i] * p[2] + t.cols[3][i])
}
