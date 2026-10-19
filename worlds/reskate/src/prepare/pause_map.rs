//! `prepare-reskate-pause-map{from=image | from=geometry,height=H,extent=region|content|x0,y0,x1,y1}`: the pause-menu
//! map picture a map writer writes (`ui/map_image.png`, `ui/map_image.json`, `ui/map_clouds.png`), chosen explicitly.
//!
//! - `from=image`: the scene's own picture (`skate.scene` `pause_map`: its image, decoded by the reader's loader into
//!   [`SkatePauseImage`], and the ground bounds it covers). An error when the scene names none or it was not loaded.
//! - `from=geometry`: a top-down picture of the map's drawn geometry (each triangle in its material's texture mean
//!   colour, highest surface on top, shaded by slope; decals, glass and blended surfaces left out), `height` pixels
//!   high and 16:9, over `extent`: the scene's stated region (`region`; an error without one), the map's own content
//!   bounds as the writer measures them (`content`), or a ground rectangle; padded by 10% and at least 50 m high.
//!   The writer rasterises its prepared geometry into the picture as it encodes it; this pass decides that it does,
//!   at what size and over what.
//!
//! The pass records the choice in [`SkatePausePicture`]; without it the writer writes no pause-map picture. The
//! picture is the map's: a part of a conversion in several is refused.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Resource, World};

use crate::dialect::SkateScene;

/// The scene's own pause-map picture, decoded (RGBA8, row 0 north), as the reader's loader puts it in the world.
#[derive(Clone, Debug, PartialEq)]
pub struct SkatePauseImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Resource for SkatePauseImage {
    const NAME: &'static str = "skate.pause_image";
}

/// The pause-map picture a map writer writes ([`PrepareReskatePauseMap`]).
#[derive(Clone, Debug, PartialEq)]
pub enum SkatePausePicture {
    /// This picture (RGBA8, row 0 north) covering ground `bounds` `[east0, north0, east1, north1]`.
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        bounds: [f32; 4],
    },
    /// The map's geometry drawn top-down, `height` pixels high, over ground `extent` (`None`: the map's own content
    /// bounds as the writer measures them).
    Geometry { height: usize, extent: Option<[f32; 4]> },
}

impl Resource for SkatePausePicture {
    const NAME: &'static str = "skate.pause_picture";
}

/// What [`PrepareReskatePauseMap`] draws the picture over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PauseExtent {
    /// The scene's stated region.
    Region,
    /// The map's own content bounds, as the writer measures them.
    Content,
    /// A ground rectangle `[east0, north0, east1, north1]`.
    Rect([f32; 4]),
}

/// Where [`PrepareReskatePauseMap`] takes the picture from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PauseSource {
    /// The scene's own picture.
    Image,
    /// The map's geometry, `height` pixels high (at least 9), over `extent`.
    Geometry { height: usize, extent: PauseExtent },
}

/// See the module documentation. Made by [`PrepareReskatePauseMap::new`]; from options by the same call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrepareReskatePauseMap {
    source: PauseSource,
}

impl PrepareReskatePauseMap {
    /// An error for a picture under 9 pixels high (16:9 needs a whole pixel across) or a rectangle that is not finite
    /// with east0 < east1 and north0 < north1.
    pub fn new(source: PauseSource) -> Result<PrepareReskatePauseMap, String> {
        if let PauseSource::Geometry { height, extent } = source {
            if height < 9 {
                return Err(format!("height {height}: at least 9 pixels"));
            }
            if let PauseExtent::Rect(r) = extent {
                let [x0, y0, x1, y1] = r;
                if !r.iter().all(|v| v.is_finite()) || x0 >= x1 || y0 >= y1 {
                    return Err(format!(
                        "rectangle {r:?}: x0, y0, x1, y1 finite, with x0 < x1 and y0 < y1"
                    ));
                }
            }
        }
        Ok(PrepareReskatePauseMap { source })
    }

    pub fn source(&self) -> PauseSource {
        self.source
    }

    /// From its options, through [`PrepareReskatePauseMap::new`]: `from=image`, or `from=geometry` with `height` and
    /// `extent=region|content` or `x0,y0,x1,y1`.
    pub fn from_options(o: &Options) -> Result<PrepareReskatePauseMap, String> {
        const KEYS: [&str; 7] = ["from", "height", "extent", "x0", "y0", "x1", "y1"];
        if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
        let source = match o.get("from").map(String::as_str) {
            Some("image") => {
                if let Some(k) = o.keys().find(|k| *k != "from") {
                    return Err(format!("from=image takes no {k}"));
                }
                PauseSource::Image
            }
            Some("geometry") => {
                let height = o
                    .get("height")
                    .ok_or("height is required (pixels) with from=geometry")?;
                let height = height
                    .parse()
                    .map_err(|_| format!("height={height} is not a number of pixels"))?;
                let num = |k: &str| -> Result<Option<f32>, String> {
                    o.get(k)
                        .map(|v| v.parse().map_err(|_| format!("{k}={v} is not a number")))
                        .transpose()
                };
                let rect = [num("x0")?, num("y0")?, num("x1")?, num("y1")?];
                let extent = match (o.get("extent").map(String::as_str), rect) {
                    (Some("region"), [None, None, None, None]) => PauseExtent::Region,
                    (Some("content"), [None, None, None, None]) => PauseExtent::Content,
                    (None, [Some(x0), Some(y0), Some(x1), Some(y1)]) => PauseExtent::Rect([x0, y0, x1, y1]),
                    _ => {
                        return Err(
                            "the extent is required with from=geometry: extent=region, extent=content or x0, y0, x1, y1"
                                .into(),
                        );
                    }
                };
                PauseSource::Geometry { height, extent }
            }
            Some(v) => return Err(format!("from={v}: image or geometry")),
            None => return Err("from is required: image or geometry".into()),
        };
        PrepareReskatePauseMap::new(source)
    }
}

/// [`PrepareReskatePauseMap::from_options`] as a pass factory.
pub fn prepare_reskate_pause_map(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskatePauseMap::from_options(o)?))
}

/// An error when `w` is one part of a conversion in several.
fn one_map(w: &World, pass: &str) -> Result<(), String> {
    let context = w
        .resource::<primase_core::convert::PartScope>()
        .is_some_and(|s| !s.context.is_empty());
    let groups = w
        .resource::<primase_core::convert::PartitionPlan>()
        .map_or(0, |p| p.groups.len());
    if context || groups > 1 {
        return Err(format!(
            "{pass} works on the whole map: this is one part of a conversion in several"
        ));
    }
    Ok(())
}

impl Pass for PrepareReskatePauseMap {
    fn name(&self) -> &'static str {
        "prepare-reskate-pause-map"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        match self.source {
            PauseSource::Image => {
                o.insert("from".into(), "image".into());
            }
            PauseSource::Geometry { height, extent } => {
                o.insert("from".into(), "geometry".into());
                o.insert("height".into(), height.to_string());
                match extent {
                    PauseExtent::Region => {
                        o.insert("extent".into(), "region".into());
                    }
                    PauseExtent::Content => {
                        o.insert("extent".into(), "content".into());
                    }
                    PauseExtent::Rect(r) => {
                        for (k, v) in ["x0", "y0", "x1", "y1"].into_iter().zip(r) {
                            o.insert(k.into(), v.to_string());
                        }
                    }
                }
            }
        }
        o
    }
    fn provides(&self) -> &[&'static str] {
        &[SkatePausePicture::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[SkatePauseImage::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        one_map(w, self.name())?;
        let picture = match self.source {
            PauseSource::Image => {
                let bounds = w
                    .resource::<SkateScene>()
                    .and_then(|s| s.pause_map.as_ref())
                    .map(|p| p.bounds)
                    .ok_or("from=image: the scene names no pause_map picture")?;
                let image = w
                    .resource::<SkatePauseImage>()
                    .ok_or("from=image: the scene's pause_map picture was not loaded (interchange::load)")?;
                log.count("pause-map pictures: the scene's own", 1);
                SkatePausePicture::Image {
                    width: image.width,
                    height: image.height,
                    rgba: image.rgba.clone(),
                    bounds,
                }
            }
            PauseSource::Geometry { height, extent } => {
                let extent = match extent {
                    PauseExtent::Region => Some(w.resource::<SkateScene>().and_then(|s| s.region).ok_or(
                        "extent=region: the scene states no region (extent=content, or x0, y0, x1, y1)",
                    )?),
                    PauseExtent::Content => None,
                    PauseExtent::Rect(r) => Some(r),
                };
                log.count("pause-map pictures: drawn from the geometry", 1);
                SkatePausePicture::Geometry { height, extent }
            }
        };
        w.remove_resource::<SkatePauseImage>();
        w.insert_resource(picture);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(kv: &[(&str, &str)]) -> Options {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn options_round_trip_and_bad_ones_refused() {
        for good in [
            o(&[("from", "image")]),
            o(&[("from", "geometry"), ("height", "2160"), ("extent", "content")]),
            o(&[("from", "geometry"), ("height", "720"), ("extent", "region")]),
            o(&[
                ("from", "geometry"),
                ("height", "9"),
                ("x0", "0"),
                ("y0", "0"),
                ("x1", "10"),
                ("y1", "5"),
            ]),
        ] {
            assert_eq!(
                PrepareReskatePauseMap::from_options(&good).unwrap().options(),
                good
            );
        }
        for bad in [
            o(&[]),
            o(&[("from", "image"), ("height", "10")]),
            o(&[("from", "geometry"), ("extent", "content")]),
            o(&[("from", "geometry"), ("height", "8"), ("extent", "content")]),
            o(&[("from", "geometry"), ("height", "100")]),
            o(&[("from", "geometry"), ("height", "100"), ("extent", "sea")]),
            o(&[("from", "drawing")]),
        ] {
            assert!(PrepareReskatePauseMap::from_options(&bad).is_err(), "{bad:?}");
        }
    }

    /// From the geometry over the scene's region, its content or a rectangle; from the scene's own image, which must be
    /// named and loaded; the loaded image is consumed.
    #[test]
    fn the_choice_is_recorded_for_the_writer() {
        let geometry =
            |extent| PrepareReskatePauseMap::new(PauseSource::Geometry { height: 720, extent }).unwrap();
        let mut w = World::new();
        assert!(
            geometry(PauseExtent::Region)
                .run(&mut w, &mut PassLog::default())
                .is_err()
        );
        geometry(PauseExtent::Content)
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert_eq!(
            w.resource::<SkatePausePicture>(),
            Some(&SkatePausePicture::Geometry {
                height: 720,
                extent: None
            })
        );
        w.insert_resource(SkateScene {
            name: "t".into(),
            region: Some([0.0, 0.0, 100.0, 50.0]),
            ..Default::default()
        });
        geometry(PauseExtent::Region)
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert_eq!(
            w.resource::<SkatePausePicture>(),
            Some(&SkatePausePicture::Geometry {
                height: 720,
                extent: Some([0.0, 0.0, 100.0, 50.0])
            })
        );
        let image = PrepareReskatePauseMap::new(PauseSource::Image).unwrap();
        assert!(image.run(&mut w, &mut PassLog::default()).is_err());
        let mut scene = w.resource::<SkateScene>().unwrap().clone();
        scene.pause_map = Some(crate::dialect::PauseMap {
            image: crate::dialect::FileRef {
                path: "ui/front.png".into(),
                sha256: None,
            },
            image_file: "front.png".into(),
            bounds: [-10.0, -10.0, 10.0, 10.0],
            other: Default::default(),
        });
        w.insert_resource(scene);
        assert!(image.run(&mut w, &mut PassLog::default()).is_err(), "not loaded");
        w.insert_resource(SkatePauseImage {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 255],
        });
        image.run(&mut w, &mut PassLog::default()).unwrap();
        assert_eq!(
            w.resource::<SkatePausePicture>(),
            Some(&SkatePausePicture::Image {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 255],
                bounds: [-10.0, -10.0, 10.0, 10.0]
            })
        );
        assert!(w.resource::<SkatePauseImage>().is_none());
    }
}
