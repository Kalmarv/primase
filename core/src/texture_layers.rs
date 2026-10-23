//! A texture with more than one image: an array of 2D images, a cube map (or an array of them) or a volume. Beside
//! [`Texture`](crate::Texture), which stays one 2D image and is what the image tools, glTF and the web writers know: a
//! tool that handles only [`Texture`](crate::Texture) never meets a layered one. A shader binding may name either
//! ([`crate::shader::TextureSource`]), and the slot's dimension has to agree with the shape
//! ([`TextureShape::dimension`]).
//!
//! The pixels are `levels[level][layer]`: each level holds its layers in the shape's order, every layer exactly the
//! format's size for the level's width and height. How a source orders the same bytes (slice after slice, or mip
//! after mip, which of the six faces comes first) is the reader's business: it puts them in this order once.

use serde_json::{Value, json};

use crate::components::{ColorSpace, NormalConvention, TexRole};
use crate::json::JsonError;
use crate::shader::TextureDim;
use crate::texformat::TexFormat;
use crate::world::{Component, Entity};

/// What the layers of a [`TextureLayers`] are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureShape {
    /// `layers` images of the same size; every level has all of them.
    Array { layers: u32 },
    /// `cubes` cube maps (1 for a plain one), six faces each in Direct3D's order: +X, -X, +Y, -Y, +Z, -Z.
    Cube { cubes: u32 },
    /// A volume `depth` texels deep; each level halves the depth as it halves the width and height (down to 1).
    Volume { depth: u32 },
}

impl TextureShape {
    /// The dimension a shader resource reads this shape as, and whether the resource is arrayed: an array is an
    /// arrayed 2D texture, one cube a cube and several an arrayed cube, a volume a 3D texture.
    pub fn dimension(self) -> (TextureDim, bool) {
        match self {
            TextureShape::Array { .. } => (TextureDim::D2, true),
            TextureShape::Cube { cubes } => (TextureDim::Cube, cubes > 1),
            TextureShape::Volume { .. } => (TextureDim::D3, false),
        }
    }

    /// The images each level of this shape holds: the layers, six for each cube, or the depth halved `level` times.
    pub fn layers_at(self, level: usize) -> u32 {
        match self {
            TextureShape::Array { layers } => layers,
            TextureShape::Cube { cubes } => cubes.saturating_mul(6),
            TextureShape::Volume { depth } => u32::try_from(level)
                .ok()
                .and_then(|l| depth.checked_shr(l))
                .unwrap_or(0)
                .max(1),
        }
    }
}

/// Pixels or encoded blocks of a texture with several images ([`TextureShape`]).
#[derive(Clone, Debug, PartialEq)]
pub struct TextureLayers {
    pub shape: TextureShape,
    pub width: u32,
    pub height: u32,
    pub format: TexFormat,
    /// `levels[level][layer]`, largest level first; each layer exactly `format.level_size(w >> level, h >> level)`
    /// bytes (axes floor at 1) and each level [`TextureShape::layers_at`] layers.
    pub levels: Vec<Vec<Vec<u8>>>,
    pub color_space: ColorSpace,
    pub role: TexRole,
    /// For `TexRole::Normal`.
    pub normal: Option<NormalConvention>,
}

impl TextureLayers {
    /// The layer `layer` of level `level`.
    pub fn layer(&self, level: usize, layer: usize) -> Option<&[u8]> {
        self.levels.get(level)?.get(layer).map(Vec::as_slice)
    }
}

impl Component for TextureLayers {
    const NAME: &'static str = "core.texture_layers";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// Every layer of every level.
    fn payload_bytes(&self) -> u64 {
        self.levels.iter().flatten().map(|l| l.len() as u64).sum()
    }
    /// A size, a shape with something in it, at least one level and no more than the size has, each level the shape's
    /// number of layers and each layer exactly its format's size for the level's dimensions.
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err(format!("{} x {} pixels", self.width, self.height));
        }
        let (count, name, extent) = match self.shape {
            TextureShape::Array { layers } => (layers, "layers", 1),
            TextureShape::Cube { cubes } => (cubes, "cubes", 1),
            TextureShape::Volume { depth } => (depth, "depth", depth),
        };
        if count == 0 {
            return Err(format!("0 {name}"));
        }
        let most = self.width.max(self.height).max(extent).ilog2() + 1;
        if self.levels.is_empty() || self.levels.len() > most as usize {
            return Err(format!(
                "{} levels for {} x {} {:?} (1 to {most})",
                self.levels.len(),
                self.width,
                self.height,
                self.shape
            ));
        }
        for (i, level) in self.levels.iter().enumerate() {
            let shift = u32::try_from(i).unwrap_or(u32::MAX);
            let (w, h) = (
                self.width.checked_shr(shift).unwrap_or(0).max(1),
                self.height.checked_shr(shift).unwrap_or(0).max(1),
            );
            let layers = self.shape.layers_at(i);
            if level.len() != layers as usize {
                return Err(format!(
                    "level {i} has {} layers, a {:?} has {layers} there",
                    level.len(),
                    self.shape
                ));
            }
            let Some(size) = self.format.checked_level_size(w, h) else {
                return Err(format!("level {i}: {w} x {h} doesn't fit in memory"));
            };
            if let Some((l, bytes)) = level.iter().enumerate().find(|(_, b)| b.len() != size) {
                return Err(format!(
                    "level {i} layer {l} ({w} x {h} {:?}): {} bytes, not {size}",
                    self.format,
                    bytes.len()
                ));
            }
        }
        Ok(())
    }
    /// A summary, not the pixels.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "shape": self.shape,
            "width": self.width,
            "height": self.height,
            "format": format!("{:?}", self.format),
            "levels": self.levels.len(),
            "color_space": self.color_space,
            "role": self.role,
            "normal": self.normal,
        })))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "hand-worked fixtures")]
mod tests {
    use super::*;
    use crate::World;

    fn layers(shape: TextureShape, w: u32, h: u32, levels: usize) -> TextureLayers {
        let format = TexFormat::Rgba8Unorm;
        TextureLayers {
            shape,
            width: w,
            height: h,
            format,
            levels: (0..levels)
                .map(|i| {
                    let (lw, lh) = ((w >> i).max(1), (h >> i).max(1));
                    (0..shape.layers_at(i))
                        .map(|_| vec![0u8; format.level_size(lw, lh)])
                        .collect()
                })
                .collect(),
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        }
    }

    fn verify(t: &TextureLayers) -> Result<(), String> {
        let mut w = World::new();
        let e = w.spawn();
        t.verify(e, &w)
    }

    #[test]
    fn a_shape_says_how_many_images_each_level_holds() {
        assert_eq!(TextureShape::Array { layers: 5 }.layers_at(3), 5);
        assert_eq!(TextureShape::Cube { cubes: 1 }.layers_at(2), 6);
        assert_eq!(TextureShape::Cube { cubes: 3 }.layers_at(0), 18);
        let v = TextureShape::Volume { depth: 33 };
        assert_eq!([0, 1, 2, 5, 6, 40].map(|l| v.layers_at(l)), [33, 16, 8, 1, 1, 1]);
    }

    #[test]
    fn a_shape_is_the_dimension_a_shader_resource_reads() {
        assert_eq!(
            TextureShape::Array { layers: 1 }.dimension(),
            (TextureDim::D2, true)
        );
        assert_eq!(
            TextureShape::Cube { cubes: 1 }.dimension(),
            (TextureDim::Cube, false)
        );
        assert_eq!(
            TextureShape::Cube { cubes: 2 }.dimension(),
            (TextureDim::Cube, true)
        );
        assert_eq!(
            TextureShape::Volume { depth: 8 }.dimension(),
            (TextureDim::D3, false)
        );
    }

    #[test]
    fn each_level_holds_the_shapes_layers_of_the_formats_size() {
        assert_eq!(
            verify(&layers(TextureShape::Array { layers: 3 }, 8, 4, 4)),
            Ok(())
        );
        assert_eq!(verify(&layers(TextureShape::Cube { cubes: 1 }, 4, 4, 3)), Ok(()));
        assert_eq!(
            verify(&layers(TextureShape::Volume { depth: 5 }, 4, 4, 3)),
            Ok(())
        );
        // a volume's levels may run to its depth too: 2 x 2 x 8 has four levels
        assert_eq!(
            verify(&layers(TextureShape::Volume { depth: 8 }, 2, 2, 4)),
            Ok(())
        );

        let mut t = layers(TextureShape::Array { layers: 3 }, 8, 4, 2);
        t.levels[1].pop();
        assert!(verify(&t).unwrap_err().contains("level 1 has 2 layers"));
        let mut t = layers(TextureShape::Array { layers: 2 }, 8, 4, 2);
        t.levels[0][1].push(0);
        assert!(verify(&t).unwrap_err().contains("level 0 layer 1"));
        let mut v = layers(TextureShape::Volume { depth: 4 }, 4, 4, 2);
        v.levels[1].push(vec![0; 16]);
        assert!(verify(&v).unwrap_err().contains("level 1 has 3 layers"));
    }

    #[test]
    fn a_texture_with_nothing_in_it_or_too_many_levels_is_refused() {
        assert!(
            verify(&layers(TextureShape::Array { layers: 0 }, 4, 4, 1))
                .unwrap_err()
                .contains("0 layers")
        );
        assert!(
            verify(&layers(TextureShape::Cube { cubes: 0 }, 4, 4, 1))
                .unwrap_err()
                .contains("0 cubes")
        );
        assert!(
            verify(&layers(TextureShape::Array { layers: 1 }, 0, 4, 1))
                .unwrap_err()
                .contains("0 x 4")
        );
        let mut t = layers(TextureShape::Array { layers: 1 }, 4, 4, 3);
        t.levels.push(vec![vec![0; 4]]);
        assert!(verify(&t).unwrap_err().contains("4 levels for 4 x 4"));
        t.levels.clear();
        assert!(verify(&t).unwrap_err().contains("0 levels"));
    }

    #[test]
    fn a_layer_is_found_by_level_and_index() {
        let mut t = layers(TextureShape::Array { layers: 2 }, 2, 2, 1);
        t.levels[0][1][0] = 7;
        assert_eq!(t.layer(0, 1).map(|l| l[0]), Some(7));
        assert_eq!(t.layer(0, 2), None);
        assert_eq!(t.layer(1, 0), None);
        assert_eq!(t.payload_bytes(), 32);
    }
}
