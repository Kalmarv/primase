//! Resource views bound by slot ([`Views`]), answering a program's requests as Direct3D answers them where a view is
//! missing or an address falls outside it. The rules are the D3D11.3 functional specification's, stated here once:
//!
//! - **Nothing bound.** A sample from a slot with no texture bound returns 0 in all components (7.18.17), and so does a
//!   load ("Fetching from t# that has nothing bound to it returns 0 for all components", 22.4.6), a structured read
//!   (22.4.12) and a size query (22.4.14, `resinfo`). A buffer at an empty slot has no elements.
//! - **Out of range.** A load whose address or level is outside the view returns 0 in all components (22.4.6's
//!   out-of-bounds behaviour; every view here has all four components, so no missing-component default arises), a
//!   typed buffer's element past its end too, and a structured read of an element past the end returns 0 (22.4.12). A
//!   structured read whose byte offset is past its element is undefined there (22.4.12) and refused here.
//! - **Sampling.** A view holds one level, so every level of detail reads it; a sample is filtered as its sampler slot
//!   states ([`ViewSampler`]): the texel it falls in, or linear between the texels about it (four in 2D, eight in a
//!   volume), each axis addressed. A sampler slot nothing states is refused, never assumed.
//!
//! What a view is not asked for is refused by name: a gather, a comparison, a texel offset, a cube or 1D texture, a
//! layer other than 0 of a 2D view, a multisample or unordered-access load, a store or an atomic (their effect is the
//! caller's).

use std::collections::BTreeMap;

use primase_image::sampling::{Address, Decoded, Subtexel};

use crate::{Dim, ImageLoad, ImageQuery, ImageSample, Load, QueryKind, Sample, Sampler};

/// One bound view.
#[derive(Clone, Debug)]
pub enum View {
    /// One value everywhere: every sample, and the load of its one texel (a 1 x 1 x 1 texture).
    Uniform([f32; 4]),
    /// A 2D float texture of one level (one slice).
    Texture2d(Decoded),
    /// A 3D float texture of one level.
    Texture3d(Decoded),
    /// A typed buffer of four 32-bit lanes per element.
    Typed(Vec<[u32; 4]>),
    /// A structured buffer: elements of `stride` bytes, as 32-bit words.
    Structured { stride: u32, words: Vec<u32> },
}

/// How a sampler slot filters a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewFilter {
    /// The texel a coordinate falls in.
    Point,
    /// Linear between the texels about a coordinate, the texel coordinate resolved to `subtexel` precision.
    Linear { subtexel: Subtexel },
}

/// A sampler slot's state: its filter and the addressing along u, v and w.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewSampler {
    pub filter: ViewFilter,
    pub address: [Address; 3],
}

/// The views and sampler states bound by slot (see the module documentation).
#[derive(Clone, Debug, Default)]
pub struct Views {
    pub views: BTreeMap<u32, View>,
    pub samplers: BTreeMap<u32, ViewSampler>,
}

impl Views {
    fn sampler(&self, slot: u32) -> Result<ViewSampler, String> {
        self.samplers
            .get(&slot)
            .copied()
            .ok_or_else(|| format!("s{slot}: no sampler state bound"))
    }

    /// A sample of view `texture` at `coord` (`dim`'s axes) through sampler slot `sampler`.
    fn filtered(&self, texture: u32, sampler: u32, dim: Dim, coord: [f32; 3]) -> Result<[f32; 4], String> {
        let state = || self.sampler(sampler);
        let error = |e: primase_image::Error| format!("t{texture}: {e}");
        match (self.views.get(&texture), dim) {
            (None, _) => Ok([0.0; 4]),
            (Some(View::Uniform(c)), _) => Ok(*c),
            (Some(View::Texture2d(image)), Dim::D2) => {
                let state = state()?;
                let (uv, address) = ([coord[0], coord[1]], [state.address[0], state.address[1]]);
                match state.filter {
                    ViewFilter::Point => image.point(uv, address),
                    ViewFilter::Linear { subtexel } => image.bilinear_at(uv, address, subtexel),
                }
                .map_err(error)
            }
            (Some(View::Texture3d(image)), Dim::D3) => {
                let state = state()?;
                match state.filter {
                    ViewFilter::Point => image.point_3d(coord, state.address),
                    ViewFilter::Linear { subtexel } => image.linear_3d(coord, state.address, subtexel),
                }
                .map_err(error)
            }
            (Some(View::Typed(_) | View::Structured { .. }), _) => {
                Err(format!("t{texture}: a sample of a buffer"))
            }
            (Some(View::Texture2d(_) | View::Texture3d(_)), dim) => Err(format!(
                "t{texture}: a {dim:?} sample of a view of another dimension"
            )),
        }
    }
}

impl Sampler for Views {
    fn sample(&mut self, r: Sample) -> Result<[f32; 4], String> {
        // a 2D view has one layer: an array's other layers are not bound here
        if r.layer.is_some_and(|l| l.round_ties_even() != 0.0)
            && matches!(self.views.get(&r.texture), Some(View::Texture2d(_)))
        {
            return Err(format!("t{}: layer {:?} of a 2D view", r.texture, r.layer));
        }
        self.filtered(r.texture, r.sampler, Dim::D2, [r.uv[0], r.uv[1], 0.0])
    }

    fn sample_image(&mut self, r: ImageSample<u32>) -> Result<[f32; 4], String> {
        if r.gather.is_some() || r.depth_ref.is_some() || r.depth || r.offset != [0; 3] {
            return Err(format!(
                "t{}: a gather, a comparison or a texel offset, which these views do not answer",
                r.texture
            ));
        }
        match r.dim {
            Dim::D2 => self.sample(Sample {
                texture: r.texture,
                sampler: r.sampler,
                uv: [r.coord[0], r.coord[1]],
                layer: r.layer,
                lod: crate::Lod::Implicit,
            }),
            Dim::D3 => self.filtered(r.texture, r.sampler, Dim::D3, r.coord),
            dim => Err(format!(
                "t{}: a {dim:?} sample, which these views do not answer",
                r.texture
            )),
        }
    }

    fn load(&mut self, r: Load) -> Result<[u32; 4], String> {
        let [x, y, z] = r.texel.map(i64::from);
        let bits = |c: [f32; 4]| c.map(f32::to_bits);
        Ok(match self.views.get(&r.texture) {
            None => [0; 4],
            Some(View::Uniform(c)) if (x, y, z, r.mip) == (0, 0, 0, 0) => bits(*c),
            Some(View::Uniform(_)) => [0; 4],
            Some(View::Texture2d(image) | View::Texture3d(image)) if r.mip == 0 => {
                image.texel(x, y, z).map_or([0; 4], bits)
            }
            Some(View::Texture2d(_) | View::Texture3d(_)) => [0; 4],
            // a buffer is addressed by its element; it has no levels
            Some(View::Typed(elements)) => usize::try_from(x)
                .ok()
                .and_then(|i| elements.get(i))
                .copied()
                .unwrap_or([0; 4]),
            Some(View::Structured { .. }) => {
                return Err(format!("t{}: a texel load of a structured buffer", r.texture));
            }
        })
    }

    fn load_image(&mut self, r: ImageLoad<u32>) -> Result<[u32; 4], String> {
        if r.storage || r.sample.is_some() {
            return Err(format!(
                "t{}: an unordered-access or multisample load, which these views do not answer",
                r.texture
            ));
        }
        if !matches!(r.dim, Dim::D2 | Dim::D3) {
            return Err(format!(
                "t{}: a {:?} load, which these views do not answer",
                r.texture, r.dim
            ));
        }
        self.load(Load {
            texture: r.texture,
            texel: [r.texel[0], r.texel[1], r.layer.unwrap_or(r.texel[2])],
            mip: r.mip.unwrap_or(0),
        })
    }

    fn structured(&mut self, slot: u32, element: u32, offset: u32) -> Result<u32, String> {
        Ok(match self.views.get(&slot) {
            None => 0,
            Some(View::Structured { stride, .. }) if offset.saturating_add(4) > *stride => {
                return Err(format!(
                    "t{slot}: a structured read at byte {offset} of an element of {stride} bytes (undefined)"
                ));
            }
            Some(View::Structured { stride, words }) => {
                let byte = u64::from(element) * u64::from(*stride) + u64::from(offset);
                usize::try_from(byte / 4)
                    .ok()
                    .and_then(|w| words.get(w))
                    .copied()
                    .unwrap_or(0)
            }
            Some(_) => {
                return Err(format!(
                    "t{slot}: a structured read of a view that is not a structured buffer"
                ));
            }
        })
    }

    fn buffer_elements(&mut self, slot: u32) -> Option<u32> {
        match self.views.get(&slot) {
            None => Some(0),
            Some(View::Typed(e)) => u32::try_from(e.len()).ok(),
            Some(View::Structured { stride, words }) if *stride > 0 => {
                u32::try_from(words.len() as u64 * 4 / u64::from(*stride)).ok()
            }
            Some(_) => None,
        }
    }

    fn query_image(&mut self, r: ImageQuery<u32>) -> Result<[u32; 4], String> {
        let size = match self.views.get(&r.texture) {
            None => return Ok([0; 4]),
            Some(View::Uniform(_)) => [1, 1, 1],
            Some(View::Texture2d(image) | View::Texture3d(image)) => {
                let [w, h] = image.dimensions();
                [w, h, image.depth()].map(|n| u32::try_from(n).unwrap_or(u32::MAX))
            }
            Some(View::Typed(_) | View::Structured { .. }) => {
                return Err(format!("t{}: a texture query of a buffer", r.texture));
            }
        };
        Ok(match r.kind {
            QueryKind::Size if r.level.unwrap_or(0) == 0 => [size[0], size[1], size[2], 0],
            // a level past the view's one: as an empty slot (resinfo)
            QueryKind::Size => [0; 4],
            QueryKind::Levels | QueryKind::Layers | QueryKind::Samples => [1, 0, 0, 0],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageLod, Lod};

    /// A 2 x 2 texture, texel (x, y) = (x, y, 0, 1), at t3; a 2 x 2 x 2 volume, texel (x, y, z) = (x, y, z, 1), at t4.
    fn views(filter: ViewFilter, address: Address) -> Views {
        let plane = (0..4)
            .map(|i| [(i % 2) as f32, (i / 2) as f32, 0.0, 1.0])
            .collect();
        let volume = (0..8)
            .map(|i| [(i % 2) as f32, (i / 2 % 2) as f32, (i / 4) as f32, 1.0])
            .collect();
        Views {
            views: BTreeMap::from([
                (3, View::Texture2d(Decoded::from_texels(2, 2, plane).unwrap())),
                (4, View::Texture3d(Decoded::volume(2, 2, 2, volume).unwrap())),
                (5, View::Typed(vec![[7; 4], [8; 4]])),
                (
                    6,
                    View::Structured {
                        stride: 8,
                        words: vec![10, 11, 12, 13],
                    },
                ),
                (7, View::Uniform([0.25; 4])),
            ]),
            samplers: BTreeMap::from([(
                0,
                ViewSampler {
                    filter,
                    address: [address; 3],
                },
            )]),
        }
    }

    fn sample(v: &mut Views, texture: u32, uv: [f32; 2]) -> Result<[f32; 4], String> {
        v.sample(Sample {
            texture,
            sampler: 0,
            uv,
            layer: None,
            lod: Lod::Implicit,
        })
    }

    fn sample_3d(v: &mut Views, texture: u32, coord: [f32; 3]) -> Result<[f32; 4], String> {
        v.sample_image(ImageSample {
            texture,
            sampler: 0,
            dim: Dim::D3,
            coord,
            layer: None,
            lod: ImageLod::Implicit,
            offset: [0; 3],
            gather: None,
            depth_ref: None,
            depth: false,
        })
    }

    fn load(v: &mut Views, texture: u32, texel: [i32; 3], mip: i32) -> [u32; 4] {
        v.load(Load { texture, texel, mip }).unwrap()
    }

    #[test]
    fn samples_are_filtered_as_their_sampler_states() {
        let point = ViewFilter::Point;
        let linear = ViewFilter::Linear {
            subtexel: Subtexel::Exact,
        };
        let mut v = views(point, Address::Clamp);
        assert_eq!(sample(&mut v, 3, [0.75, 0.25]), Ok([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(sample(&mut v, 3, [1.75, -3.0]), Ok([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(sample_3d(&mut v, 4, [0.25, 0.75, 0.75]), Ok([0.0, 1.0, 1.0, 1.0]));
        let mut w = views(point, Address::Wrap);
        assert_eq!(sample(&mut w, 3, [-0.25, 0.25]), Ok([1.0, 0.0, 0.0, 1.0]));
        let mut l = views(linear, Address::Clamp);
        assert_eq!(sample(&mut l, 3, [0.5, 0.5]), Ok([0.5, 0.5, 0.0, 1.0]));
        assert_eq!(sample_3d(&mut l, 4, [0.5; 3]), Ok([0.5, 0.5, 0.5, 1.0]));
        // a uniform view is its value, whatever the coordinate, dimension or layer
        assert_eq!(sample(&mut l, 7, [9.0, -9.0]), Ok([0.25; 4]));
        assert_eq!(sample_3d(&mut l, 7, [0.5; 3]), Ok([0.25; 4]));
    }

    #[test]
    fn what_is_not_bound_reads_zero() {
        let mut v = views(ViewFilter::Point, Address::Clamp);
        assert_eq!(sample(&mut v, 9, [0.5, 0.5]), Ok([0.0; 4]));
        assert_eq!(sample_3d(&mut v, 9, [0.5; 3]), Ok([0.0; 4]));
        assert_eq!(load(&mut v, 9, [0, 0, 0], 0), [0; 4]);
        assert_eq!(v.structured(9, 0, 0), Ok(0));
        assert_eq!(v.buffer_elements(9), Some(0));
        let query = ImageQuery {
            texture: 9,
            dim: Dim::D2,
            kind: QueryKind::Size,
            level: None,
        };
        assert_eq!(v.query_image(query), Ok([0; 4]));
        assert_eq!(
            v.query_image(ImageQuery { texture: 4, ..query }),
            Ok([2, 2, 2, 0])
        );
    }

    #[test]
    fn a_load_outside_its_view_reads_zero() {
        let mut v = views(ViewFilter::Point, Address::Clamp);
        let one = 1f32.to_bits();
        assert_eq!(load(&mut v, 3, [1, 1, 0], 0), [one, one, 0, one]);
        assert_eq!(load(&mut v, 3, [2, 0, 0], 0), [0; 4]);
        assert_eq!(load(&mut v, 3, [0, -1, 0], 0), [0; 4]);
        assert_eq!(load(&mut v, 3, [0, 0, 1], 0), [0; 4], "a layer past the view");
        assert_eq!(load(&mut v, 3, [0, 0, 0], 1), [0; 4], "a level past the view");
        assert_eq!(load(&mut v, 4, [1, 0, 1], 0), [one, 0, one, one]);
        assert_eq!(load(&mut v, 4, [0, 0, 2], 0), [0; 4]);
        assert_eq!(load(&mut v, 5, [1, 0, 0], 0), [8; 4]);
        assert_eq!(load(&mut v, 5, [2, 0, 0], 0), [0; 4]);
        assert_eq!(load(&mut v, 7, [0, 0, 0], 0), [0.25f32.to_bits(); 4]);
        assert_eq!(load(&mut v, 7, [1, 0, 0], 0), [0; 4]);
        // a structured element past the end is 0; a byte offset past the element is undefined, refused
        assert_eq!(v.structured(6, 1, 4), Ok(13));
        assert_eq!(v.structured(6, 2, 0), Ok(0));
        assert!(v.structured(6, 0, 8).is_err());
        assert_eq!(v.buffer_elements(6), Some(2));
        assert_eq!(v.buffer_elements(5), Some(2));
    }

    #[test]
    fn what_the_views_do_not_answer_is_refused() {
        let mut v = views(ViewFilter::Point, Address::Clamp);
        // no sampler state at s1
        assert!(
            v.sample(Sample {
                texture: 3,
                sampler: 1,
                uv: [0.5; 2],
                layer: None,
                lod: Lod::Implicit,
            })
            .is_err()
        );
        // a 2D view sampled as a volume and the other way; a buffer sampled
        assert!(sample_3d(&mut v, 3, [0.5; 3]).is_err());
        assert!(sample(&mut v, 4, [0.5; 2]).is_err());
        assert!(sample(&mut v, 5, [0.5; 2]).is_err());
        // a layer other than 0 of a 2D view
        assert!(
            v.sample(Sample {
                texture: 3,
                sampler: 0,
                uv: [0.5; 2],
                layer: Some(1.0),
                lod: Lod::Implicit,
            })
            .is_err()
        );
        let gather = ImageSample {
            texture: 3,
            sampler: 0,
            dim: Dim::D2,
            coord: [0.5; 3],
            layer: None,
            lod: ImageLod::Implicit,
            offset: [0; 3],
            gather: Some(0),
            depth_ref: None,
            depth: false,
        };
        assert!(v.sample_image(gather).is_err());
        assert!(
            v.sample_image(ImageSample {
                gather: None,
                offset: [1, 0, 0],
                ..gather
            })
            .is_err()
        );
        assert!(v.structured(3, 0, 0).is_err());
        assert!(
            v.load(Load {
                texture: 6,
                texel: [0; 3],
                mip: 0
            })
            .is_err()
        );
    }
}
