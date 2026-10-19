//! `prepare-reskate-overlays{step}`: the decal- and water-class overlays of the map's meshes stacked, as the writer
//! used to stack them while it wrote.
//!
//! The writer draws the world as meshes of its own: each render tile's placements merged, each instanced prototype in
//! its own frame. This pass assembles those meshes with the writer's own code (its tiles and prototypes, textures
//! identified but not encoded) and stacks each mesh's overlay pieces `step` metres apart, in front of the parallel
//! surfaces they overlap that should be under them ([`crate::overlay`]). It records the result by piece (a placement's
//! part, or a prototype's part as a variant row draws it) as `skate.overlay_stack`: each lifted piece's lift, which
//! piece lies under which, and the material every piece of a stacked mesh was drawn with, with the writer settings the
//! meshes depend on (render tile, instancing, limit, offset). The writer puts the stack on the same pieces again and
//! orders their materials by it; it refuses a stack made for other settings, pieces or materials. Without this pass
//! the writer stacks nothing (reported). Run it after every pass that changes geometry, materials or textures.

use primase_core::World;
use primase_core::pass::{Options, Pass, PassLog};

use crate::build::{EncodingOptions, overlay_stack};
use crate::dialect::SkateOverlayStack;

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskateOverlays {
    step: f32,
    encoding: EncodingOptions,
}

/// The writer settings a pass that assembles the writer's meshes groups them by (render tile, instancing, longest render
/// edge, limit, offset), checked: a render tile and limit above 0, counts of 0 or more, finite metres.
pub(crate) fn check_grouping(e: &EncodingOptions) -> Result<(), String> {
    if !(e.tile.is_finite() && e.tile > 0.0) || !(e.limit.is_finite() && e.limit > 0.0) {
        return Err(format!("tile={}, limit={}: metres above 0", e.tile, e.limit));
    }
    if e.min_instances < 0 || e.min_instance_tris < 0 {
        return Err("instancing thresholds are counts of 0 or more".into());
    }
    if e.max_render_edge.is_some_and(|m| !(m.is_finite() && m >= 0.0)) {
        return Err(format!(
            "max_render_edge={:?}: finite metres of 0 or more",
            e.max_render_edge
        ));
    }
    if !e.offset.iter().all(|v| v.is_finite()) {
        return Err(format!("offset {:?} is not finite metres", e.offset));
    }
    Ok(())
}

/// Those settings from a pass's options (`tile`, `min_instances`, `min_instance_tris`, `max_render_edge`, `limit`,
/// `offset_x`, `offset_y`; each optional, the writer's default otherwise); any other key is refused.
pub(crate) fn encoding_of(o: &Options) -> Result<EncodingOptions, String> {
    let known = [
        "tile",
        "min_instances",
        "min_instance_tris",
        "max_render_edge",
        "limit",
        "offset_x",
        "offset_y",
    ];
    if let Some(k) = o.keys().find(|k| !known.contains(&k.as_str())) {
        return Err(format!("unknown option {k}"));
    }
    let number = |k: &str| -> Result<Option<f64>, String> {
        o.get(k)
            .map(|v| v.parse::<f64>().map_err(|_| format!("{k}={v} is not a number")))
            .transpose()
    };
    let count = |k: &str| -> Result<Option<i64>, String> {
        o.get(k)
            .map(|v| v.parse::<i64>().map_err(|_| format!("{k}={v} is not a count")))
            .transpose()
    };
    let mut e = EncodingOptions::default();
    if let Some(v) = number("tile")? {
        e.tile = v as f32;
    }
    if let Some(v) = count("min_instances")? {
        e.min_instances = v;
    }
    if let Some(v) = count("min_instance_tris")? {
        e.min_instance_tris = v;
    }
    if let Some(v) = number("max_render_edge")? {
        e.max_render_edge = Some(v as f32);
    }
    if let Some(v) = number("limit")? {
        e.limit = v as f32;
    }
    if let Some(v) = number("offset_x")? {
        e.offset[0] = v;
    }
    if let Some(v) = number("offset_y")? {
        e.offset[1] = v;
    }
    check_grouping(&e)?;
    Ok(e)
}

/// Those settings as options, every one ([`encoding_of`] reads them back).
pub(crate) fn encoding_options(e: &EncodingOptions) -> Options {
    let mut o = Options::from([
        ("tile".to_owned(), e.tile.to_string()),
        ("min_instances".to_owned(), e.min_instances.to_string()),
        ("min_instance_tris".to_owned(), e.min_instance_tris.to_string()),
        ("limit".to_owned(), e.limit.to_string()),
        ("offset_x".to_owned(), e.offset[0].to_string()),
        ("offset_y".to_owned(), e.offset[1].to_string()),
    ]);
    if let Some(m) = e.max_render_edge {
        o.insert("max_render_edge".into(), m.to_string());
    }
    o
}

/// Only the grouping settings of `e`: what the pass's result depends on.
pub(crate) fn grouping_only(e: &EncodingOptions) -> EncodingOptions {
    EncodingOptions {
        tile: e.tile,
        min_instances: e.min_instances,
        min_instance_tris: e.min_instance_tris,
        max_render_edge: e.max_render_edge,
        limit: e.limit,
        offset: e.offset,
        ..EncodingOptions::default()
    }
}

impl PrepareReskateOverlays {
    /// Overlays `step` metres apart (finite, 0 or more; 0: none lifted, none ordered), for a map written with the
    /// writer's default encoding ([`EncodingOptions::default`]).
    pub fn new(step: f32) -> Result<PrepareReskateOverlays, String> {
        if !(step.is_finite() && step >= 0.0) {
            return Err(format!("step={step}: finite metres of 0 or more"));
        }
        Ok(PrepareReskateOverlays {
            step,
            encoding: EncodingOptions::default(),
        })
    }

    /// For the encoding the map will be written with (its render tile, instancing, limit and offset), checked
    /// ([`check_grouping`]).
    pub fn for_encoding(mut self, e: &EncodingOptions) -> Result<PrepareReskateOverlays, String> {
        check_grouping(e)?;
        self.encoding = grouping_only(e);
        Ok(self)
    }

    pub fn step(&self) -> f32 {
        self.step
    }

    /// From its options: `step` required; the encoding's ([`encoding_of`]) optional; through
    /// [`PrepareReskateOverlays::new`] and [`PrepareReskateOverlays::for_encoding`].
    pub fn from_options(o: &Options) -> Result<PrepareReskateOverlays, String> {
        let mut rest = o.clone();
        let step = rest
            .remove("step")
            .ok_or("step is required (metres between stacked overlays; 0: none)")?;
        let step: f32 = step.parse().map_err(|_| format!("step={step} is not a number"))?;
        let e = encoding_of(&rest)?;
        PrepareReskateOverlays::new(step)?.for_encoding(&e)
    }
}

/// [`PrepareReskateOverlays::from_options`] as a pass factory.
pub fn prepare_reskate_overlays(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateOverlays::from_options(o)?))
}

impl Pass for PrepareReskateOverlays {
    fn name(&self) -> &'static str {
        "prepare-reskate-overlays"
    }
    fn options(&self) -> Options {
        let mut o = encoding_options(&self.encoding);
        o.insert("step".into(), self.step.to_string());
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.overlay_stack"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        super::require_one_part(w, self.name())?;
        if w.resource::<SkateOverlayStack>().is_some() {
            return Err(
                "prepare-reskate-overlays: the overlays are already stacked (skate.overlay_stack)".into(),
            );
        }
        let rec = overlay_stack(w, &self.encoding, self.step).map_err(|e| e.to_string())?;
        log.count("overlay pieces stacked", rec.totals.pieces.max(0) as u64);
        log.count(
            "overlay pieces lifted",
            rec.pieces.values().filter(|p| p.shift > 0.0).count() as u64,
        );
        log.count(
            "overlay relations (one piece under another)",
            rec.under.len() as u64,
        );
        if rec.totals.conflicts_left > 0 {
            log.count(
                "overlay pieces whose lift could not clear every surface",
                rec.totals.conflicts_left as u64,
            );
        }
        w.insert_resource(rec);
        Ok(())
    }
}
