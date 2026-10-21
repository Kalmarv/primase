//! `prepare-reskate-pause-map{from=image | from=geometry,height=H,extent=region|content|collision|x0,y0,x1,y1,
//! leave-out=shadowless|placements|nothing[,prefixes=<prefix>;...],colour=mean|texture}`: the pause-menu map picture a map writer writes
//! (`ui/map_image.png`, `ui/map_image.json`, `ui/map_clouds.png`), chosen explicitly.
//!
//! - `from=image`: the scene's own picture (`skate.scene` `pause_map`: its image, decoded by the reader's loader into
//!   [`SkatePauseImage`], and the ground bounds it covers). An error when the scene names none or it was not loaded.
//! - `from=geometry`: a top-down picture of the map's drawn geometry (highest surface on top, shaded by slope; decals,
//!   glass and blended surfaces left out), `height` pixels high and 16:9, over `extent`: the scene's stated region
//!   (`region`; an error without one), the map's own content bounds as the writer measures them (`content`), the
//!   play space (`collision` with `max-extent=M`: the box of every placed collision soup no wider than M metres on any
//!   axis, [`primase_meshops::collision_query::collision_box`], so a level's skyline backdrops stay out of the
//!   picture; a map converted whole only, the scene's region left alone), or a ground rectangle; padded by 10% and at
//!   least 50 m high. Two more choices, both required:
//!   - `leave-out=shadowless` leaves out the triangles whose material casts no shadow (`skate.material`
//!     `cast_shadows` false: an indoor level's shell and ceiling, as `lower-reskate-shadows` lowered them), so
//!     the picture shows the room from above, counted in the writer's report. `leave-out=placements` with
//!     `prefixes` leaves out the triangles of the placements whose name starts with one of the prefixes (names as
//!     their reader wrote them, path separators included): the ceiling and walls only, while the floor and other
//!     structures that cast no shadow stay in the picture; counted. A prefix no placement's name starts with is an
//!     error on a whole map, and counted in a part of one (the part may hold none of it). `leave-out=nothing`
//!     draws every triangle.
//!   - `colour=mean` draws each triangle in its material's base texture's mean colour; `colour=texture` samples the
//!     base texture at each pixel through the triangle's coordinates (transforms are baked before `raise-skate`),
//!     times the material's base colour factor (in linear light), wrapping as the material says: bilinear, in the
//!     level of the texture's mip chain (at most 128 pixels on a side, box-filtered) whose texel best matches the
//!     pixel's footprint. A material with no texture loaded is drawn in its mean colour, reported by name.
//!
//!   The writer rasterises its prepared geometry into the picture as it encodes it; this pass decides that it does,
//!   at what size, over what and how.
//!
//! The pass records the choice in [`SkatePausePicture`]; without it the writer writes no pause-map picture. The
//! picture is the map's: in a part of a map converted in several, `extent=content` with no stated region is the map's
//! content extent merged over every part (`summarize-reskate-content`), recorded as a rectangle (whole metres, exact
//! in the record's f32), so every part draws over the same extent; a part without it is refused.

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Resource, World};

pub use crate::components::pause_map::{SkatePauseImage, SkatePausePicture};

pub use crate::components::pause_map::{PauseColour, PauseDrawing, PauseLeaveOut};

use crate::components::{SkateContentBounds, SkateScene};

/// What [`PrepareReskatePauseMap`] draws the picture over.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PauseExtent {
    /// The scene's stated region.
    Region,
    /// The map's own content bounds, as the writer measures them.
    Content,
    /// A ground rectangle `[east0, north0, east1, north1]`.
    Rect([f32; 4]),
    /// The play space: the box of every placed collision soup no wider than `max_extent` metres on any axis.
    Collision { max_extent: f32 },
}

/// Where [`PrepareReskatePauseMap`] takes the picture from.
#[derive(Clone, Debug, PartialEq)]
pub enum PauseSource {
    /// The scene's own picture.
    Image,
    /// The map's geometry, `height` pixels high (at least 9), over `extent`, drawn as `drawing` says.
    Geometry {
        height: usize,
        extent: PauseExtent,
        drawing: PauseDrawing,
    },
}

/// See the module documentation. Made by [`PrepareReskatePauseMap::new`]; from options by the same call.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskatePauseMap {
    source: PauseSource,
}

impl PrepareReskatePauseMap {
    /// An error for a picture under 9 pixels high (16:9 needs a whole pixel across) or a rectangle that is not finite
    /// with east0 < east1 and north0 < north1.
    pub fn new(source: PauseSource) -> Result<PrepareReskatePauseMap, String> {
        if let PauseSource::Geometry {
            height,
            extent,
            drawing,
        } = &source
        {
            let (height, extent) = (*height, *extent);
            if let PauseLeaveOut::Placements(prefixes) = &drawing.leave_out
                && (prefixes.is_empty() || prefixes.iter().any(String::is_empty))
            {
                return Err("leave-out=placements names no prefix (prefixes separated by ;)".into());
            }
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

    pub fn source(&self) -> &PauseSource {
        &self.source
    }

    /// From its options, through [`PrepareReskatePauseMap::new`]: `from=image`, or `from=geometry` with `height`,
    /// `extent=region|content` (or `extent=collision` with `max-extent`) or `x0,y0,x1,y1`, `leave-out=shadowless|nothing|placements` (`placements` with
    /// `prefixes`, separated by `;`) and `colour=mean|texture`.
    pub fn from_options(o: &Options) -> Result<PrepareReskatePauseMap, String> {
        const KEYS: [&str; 11] = [
            "from",
            "height",
            "extent",
            "max-extent",
            "x0",
            "y0",
            "x1",
            "y1",
            "leave-out",
            "colour",
            "prefixes",
        ];
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
                let max_extent = num("max-extent")?;
                if max_extent.is_some() && o.get("extent").map(String::as_str) != Some("collision") {
                    return Err("max-extent goes with extent=collision".into());
                }
                let extent = match (o.get("extent").map(String::as_str), rect) {
                    (Some("collision"), [None, None, None, None]) => PauseExtent::Collision {
                        max_extent: max_extent
                            .filter(|m| m.is_finite() && *m > 0.0)
                            .ok_or("extent=collision requires max-extent (metres, above 0)")?,
                    },
                    (Some("region"), [None, None, None, None]) => PauseExtent::Region,
                    (Some("content"), [None, None, None, None]) => PauseExtent::Content,
                    (None, [Some(x0), Some(y0), Some(x1), Some(y1)]) => PauseExtent::Rect([x0, y0, x1, y1]),
                    _ => {
                        return Err(
                            "the extent is required with from=geometry: extent=region, extent=content, extent=collision with max-extent, or x0, y0, x1, y1"
                                .into(),
                        );
                    }
                };
                let leave_out = match (o.get("leave-out").map(String::as_str), o.get("prefixes")) {
                    (Some("shadowless"), None) => PauseLeaveOut::Shadowless,
                    (Some("nothing"), None) => PauseLeaveOut::Nothing,
                    (Some("placements"), Some(p)) => {
                        PauseLeaveOut::Placements(p.split(';').map(str::to_owned).collect())
                    }
                    (Some("placements"), None) => {
                        return Err(
                            "leave-out=placements requires prefixes (placement name prefixes, by ;)".into(),
                        );
                    }
                    (Some("shadowless" | "nothing"), Some(_)) => {
                        return Err("prefixes are leave-out=placements' only".into());
                    }
                    (Some(v), _) => return Err(format!("leave-out={v}: shadowless, placements or nothing")),
                    (None, _) => {
                        return Err(
                            "leave-out is required with from=geometry: shadowless (materials casting no shadow), \
                             placements (with prefixes) or nothing"
                                .into(),
                        );
                    }
                };
                let colour = match o.get("colour").map(String::as_str) {
                    Some("mean") => PauseColour::Mean,
                    Some("texture") => PauseColour::Texture,
                    Some(v) => return Err(format!("colour={v}: mean or texture")),
                    None => return Err("colour is required with from=geometry: mean or texture".into()),
                };
                PauseSource::Geometry {
                    height,
                    extent,
                    drawing: PauseDrawing { leave_out, colour },
                }
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

impl Pass for PrepareReskatePauseMap {
    fn name(&self) -> &'static str {
        "prepare-reskate-pause-map"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        match &self.source {
            PauseSource::Image => {
                o.insert("from".into(), "image".into());
            }
            PauseSource::Geometry {
                height,
                extent,
                drawing,
            } => {
                o.insert("from".into(), "geometry".into());
                o.insert("height".into(), height.to_string());
                o.insert(
                    "leave-out".into(),
                    match &drawing.leave_out {
                        PauseLeaveOut::Shadowless => "shadowless",
                        PauseLeaveOut::Placements(_) => "placements",
                        PauseLeaveOut::Nothing => "nothing",
                    }
                    .into(),
                );
                if let PauseLeaveOut::Placements(p) = &drawing.leave_out {
                    o.insert("prefixes".into(), p.iter().cloned().collect::<Vec<_>>().join(";"));
                }
                o.insert(
                    "colour".into(),
                    match drawing.colour {
                        PauseColour::Mean => "mean",
                        PauseColour::Texture => "texture",
                    }
                    .into(),
                );
                match extent {
                    PauseExtent::Region => {
                        o.insert("extent".into(), "region".into());
                    }
                    PauseExtent::Content => {
                        o.insert("extent".into(), "content".into());
                    }
                    PauseExtent::Collision { max_extent } => {
                        o.insert("extent".into(), "collision".into());
                        o.insert("max-extent".into(), max_extent.to_string());
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
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let picture = match &self.source {
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
            PauseSource::Geometry {
                height,
                extent,
                drawing,
            } => {
                if let PauseLeaveOut::Placements(prefixes) = &drawing.leave_out {
                    // a prefix naming nothing is a mistake; a part may hold none of a prefix's placements, counted
                    let names: Vec<&str> = w
                        .iter::<primase_core::Instance>()
                        .filter_map(|(e, _)| w.get::<primase_core::Name>(e).map(|n| n.0.as_str()))
                        .collect();
                    let part = w
                        .resource::<primase_core::convert::PartScope>()
                        .is_some_and(|s| !s.whole);
                    for p in prefixes {
                        if !names.iter().any(|n| n.starts_with(p.as_str())) {
                            if !part {
                                return Err(format!(
                                    "leave-out=placements: no placement's name starts with {p} (names are matched as \
                                     their reader wrote them, path separators included)"
                                ));
                            }
                            log.count(&format!("prefixes matching no placement of this part: {p}"), 1);
                        }
                    }
                }
                let extent = match *extent {
                    PauseExtent::Region => Some(w.resource::<SkateScene>().and_then(|s| s.region).ok_or(
                        "extent=region: the scene states no region (extent=content, or x0, y0, x1, y1)",
                    )?),
                    // the writer's region: the stated one, else the content's; a part takes the merged content's
                    PauseExtent::Content => {
                        let stated = w.resource::<SkateScene>().is_some_and(|s| s.region.is_some());
                        match w.resource::<SkateContentBounds>() {
                            _ if stated => None,
                            Some(b) => Some(b.extent().map(|v| v as f32)),
                            None => {
                                super::require_whole_map(w, self.name()).map_err(|e| {
                                    format!(
                                        "{e}: extent=content over a map in parts is its content merged over every \
                                         part (summarize-reskate-content)"
                                    )
                                })?;
                                None
                            }
                        }
                    }
                    PauseExtent::Rect(r) => Some(r),
                    // the play space: a part's soups are not the map's, so a map converted whole only
                    PauseExtent::Collision { max_extent } => {
                        super::require_whole_map(w, self.name()).map_err(|e| {
                            format!("{e}: extent=collision measures the whole map's collision")
                        })?;
                        let [lo, hi] = primase_meshops::collision_query::collision_box(
                            w,
                            f64::from(max_extent),
                        )
                        .ok_or_else(|| {
                            format!("extent=collision: no collision soup within {max_extent} m on every axis")
                        })?;
                        // ground frame (east, north) = core (x, -z)
                        Some([lo[0] as f32, (-hi[2]) as f32, hi[0] as f32, (-lo[2]) as f32])
                    }
                };
                log.count("pause-map pictures: drawn from the geometry", 1);
                SkatePausePicture::Geometry {
                    height: *height,
                    extent,
                    drawing: drawing.clone(),
                }
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
            o(&[
                ("from", "geometry"),
                ("height", "2160"),
                ("extent", "content"),
                ("leave-out", "shadowless"),
                ("colour", "texture"),
            ]),
            o(&[
                ("from", "geometry"),
                ("height", "720"),
                ("extent", "region"),
                ("leave-out", "nothing"),
                ("colour", "mean"),
            ]),
            o(&[
                ("from", "geometry"),
                ("height", "9"),
                ("x0", "0"),
                ("y0", "0"),
                ("x1", "10"),
                ("y1", "5"),
                ("leave-out", "nothing"),
                ("colour", "mean"),
            ]),
            o(&[
                ("from", "geometry"),
                ("height", "720"),
                ("extent", "content"),
                ("leave-out", "placements"),
                ("prefixes", "Level Bounds/Ceiling;Level Bounds/Longer Wall"),
                ("colour", "texture"),
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
            // the drawing choices are required, and checked
            o(&[
                ("from", "geometry"),
                ("height", "100"),
                ("extent", "content"),
                ("colour", "mean"),
            ]),
            o(&[
                ("from", "geometry"),
                ("height", "100"),
                ("extent", "content"),
                ("leave-out", "nothing"),
            ]),
            o(&[
                ("from", "geometry"),
                ("height", "100"),
                ("extent", "content"),
                ("leave-out", "ceilings"),
                ("colour", "mean"),
            ]),
            o(&[
                ("from", "geometry"),
                ("height", "100"),
                ("extent", "content"),
                ("leave-out", "nothing"),
                ("colour", "photo"),
            ]),
            o(&[("from", "image"), ("colour", "mean")]),
            // placements take prefixes, which the others do not take
            o(&[
                ("from", "geometry"),
                ("height", "100"),
                ("extent", "content"),
                ("leave-out", "placements"),
                ("colour", "mean"),
            ]),
            o(&[
                ("from", "geometry"),
                ("height", "100"),
                ("extent", "content"),
                ("leave-out", "shadowless"),
                ("prefixes", "Level Bounds/Ceiling"),
                ("colour", "mean"),
            ]),
            o(&[
                ("from", "geometry"),
                ("height", "100"),
                ("extent", "content"),
                ("leave-out", "placements"),
                ("prefixes", "a;"),
                ("colour", "mean"),
            ]),
        ] {
            assert!(PrepareReskatePauseMap::from_options(&bad).is_err(), "{bad:?}");
        }
    }

    /// leave-out=placements: a prefix no placement's name starts with is refused on a whole map; one that names a
    /// placement is recorded for the writer.
    #[test]
    fn a_placements_prefix_naming_nothing_is_refused() {
        let pass = |prefix: &str| {
            PrepareReskatePauseMap::new(PauseSource::Geometry {
                height: 720,
                extent: PauseExtent::Content,
                drawing: PauseDrawing {
                    leave_out: PauseLeaveOut::Placements([prefix.to_owned()].into()),
                    colour: PauseColour::Mean,
                },
            })
            .unwrap()
        };
        let mut w = World::new();
        let (proto, p) = (w.spawn(), w.spawn());
        w.insert(
            p,
            primase_core::Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(p, primase_core::Name("Level Bounds/Ceiling".into()));
        let e = pass("Level Bounds / Ceiling")
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(
            e.contains("no placement's name starts with Level Bounds / Ceiling"),
            "{e}"
        );
        pass("Level Bounds/")
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert!(matches!(
            w.resource::<SkatePausePicture>(),
            Some(SkatePausePicture::Geometry { drawing, .. })
                if drawing.leave_out == PauseLeaveOut::Placements(["Level Bounds/".to_owned()].into())
        ));
    }

    /// extent=collision: the box of the soups no wider than max-extent (a 300 m backdrop left out), in the ground frame
    /// (east, north = -z); the scene's region untouched; the options read back and max-extent is required.
    #[test]
    fn the_play_space_extent_leaves_out_backdrops() {
        let mut w = World::new();
        for corners in [
            [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.0, 20.0]],
            [[-150.0, 0.0, -50.0], [150.0, 0.0, -50.0], [0.0, 80.0, -50.0]],
        ] {
            let e = w.spawn();
            w.insert(
                e,
                primase_core::Collision {
                    positions: corners.to_vec(),
                    triangles: vec![[0, 1, 2]],
                    surfaces: vec![0],
                },
            );
        }
        let opts = o(&[
            ("from", "geometry"),
            ("height", "720"),
            ("extent", "collision"),
            ("max-extent", "100"),
            ("leave-out", "nothing"),
            ("colour", "mean"),
        ]);
        let pass = PrepareReskatePauseMap::from_options(&opts).unwrap();
        assert_eq!(pass.options(), opts);
        pass.run(&mut w, &mut PassLog::default()).unwrap();
        assert!(matches!(
            w.resource::<SkatePausePicture>(),
            Some(SkatePausePicture::Geometry { extent: Some(r), .. }) if *r == [0.0, -20.0, 10.0, 0.0]
        ));
        assert!(w.resource::<SkateScene>().is_none_or(|s| s.region.is_none()));
        let mut missing = opts.clone();
        missing.remove("max-extent");
        assert!(PrepareReskatePauseMap::from_options(&missing).is_err());
    }

    /// From the geometry over the scene's region, its content or a rectangle; from the scene's own image, which must be
    /// named and loaded; the loaded image is consumed.
    #[test]
    fn the_choice_is_recorded_for_the_writer() {
        let drawing = PauseDrawing {
            leave_out: PauseLeaveOut::Shadowless,
            colour: PauseColour::Texture,
        };
        let geometry = |extent| {
            PrepareReskatePauseMap::new(PauseSource::Geometry {
                height: 720,
                extent,
                drawing: drawing.clone(),
            })
            .unwrap()
        };
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
                extent: None,
                drawing: drawing.clone()
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
                extent: Some([0.0, 0.0, 100.0, 50.0]),
                drawing: drawing.clone()
            })
        );
        let image = PrepareReskatePauseMap::new(PauseSource::Image).unwrap();
        assert!(image.run(&mut w, &mut PassLog::default()).is_err());
        let mut scene = w.resource::<SkateScene>().unwrap().clone();
        scene.pause_map = Some(crate::components::PauseMap {
            image: crate::components::FileRef {
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
            file: None,
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
