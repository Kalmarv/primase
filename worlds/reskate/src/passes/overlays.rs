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
//!
//! A part of a map converted in several stacks its own meshes, as the writer lays them out (the map's instancing and
//! region, merged over every part): its render tiles and the prototypes it draws. A prototype drawn by several parts is
//! stacked by each, the same; the writer checks they agree and counts it once.

use primase_core::World;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};

use crate::assembly::{EncodingOptions, overlay_stack};
use crate::components::SkateOverlayStack;

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

/// The writer's render tile, `tile`: one of the grouping settings a pass that assembles the writer's meshes takes
/// ([`ENCODING`]).
pub(crate) const TILE: OptionSpec = OptionSpec::optional(
    "tile",
    "200",
    "metres above 0: the edge of the render tiles the writer merges placements on",
    "200, the writer's",
);
/// `min_instances`.
pub(crate) const MIN_INSTANCES: OptionSpec = OptionSpec::optional(
    "min_instances",
    "3",
    "a count: a prototype placed at least this often, with min_instance_tris triangles, is written as instances",
    "3, the writer's",
);
/// `min_instance_tris`.
pub(crate) const MIN_INSTANCE_TRIS: OptionSpec = OptionSpec::optional(
    "min_instance_tris",
    "150",
    "a count: the triangles a prototype placed min_instances times needs to be written as instances",
    "150, the writer's",
);
/// `max_render_edge`.
pub(crate) const MAX_RENDER_EDGE: OptionSpec = OptionSpec::optional(
    "max_render_edge",
    "50",
    "metres, 0 or more: the longest render edge a written triangle keeps before it is split",
    "the scene's",
);
/// `limit`.
pub(crate) const LIMIT: OptionSpec = OptionSpec::optional(
    "limit",
    "4780",
    "metres: the map's half extent, Studio's limit",
    "4780, the writer's",
);
/// `offset_x`.
pub(crate) const OFFSET_X: OptionSpec = OptionSpec::optional(
    "offset_x",
    "0",
    "metres: added to every written east coordinate",
    "0, the writer's",
);
/// `offset_y`.
pub(crate) const OFFSET_Y: OptionSpec = OptionSpec::optional(
    "offset_y",
    "0",
    "metres: added to every written north coordinate",
    "0, the writer's",
);

/// The writer's grouping settings as options, each optional: the writer's default
/// ([`EncodingOptions::default`]) when not given.
pub(crate) const ENCODING: [OptionSpec; 7] = [
    TILE,
    MIN_INSTANCES,
    MIN_INSTANCE_TRIS,
    MAX_RENDER_EDGE,
    LIMIT,
    OFFSET_X,
    OFFSET_Y,
];

/// A pass's own options `own`, then [`ENCODING`]'s (`N` is their count together).
pub(crate) const fn with_encoding<const A: usize, const N: usize>(own: [OptionSpec; A]) -> [OptionSpec; N] {
    assert!(
        N == A + ENCODING.len(),
        "N is the pass's options and the encoding's"
    );
    let mut out = [TILE; N];
    let mut i = 0;
    while i < A {
        out[i] = own[i];
        i += 1;
    }
    let mut j = 0;
    while j < ENCODING.len() {
        out[A + j] = ENCODING[j];
        j += 1;
    }
    out
}

/// Those settings from a pass's options, whose declaration has [`ENCODING`]'s, through [`check_grouping`].
pub(crate) fn encoding_from(g: &Given) -> Result<EncodingOptions, String> {
    let number = |k: &str| -> Result<Option<f64>, String> {
        g.optional(k)
            .map(|v| v.parse::<f64>().map_err(|_| format!("{k}={v} is not a number")))
            .transpose()
    };
    let count = |k: &str| -> Result<Option<i64>, String> {
        g.optional(k)
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

/// Those settings as options, every one ([`encoding_from`] reads them back).
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

    /// From its options: `step` and the encoding's ([`encoding_from`]); through [`PrepareReskateOverlays::new`] and
    /// [`PrepareReskateOverlays::for_encoding`].
    fn from_given(g: &Given) -> Result<PrepareReskateOverlays, String> {
        let step = g.required("step");
        let step: f32 = step.parse().map_err(|_| format!("step={step} is not a number"))?;
        PrepareReskateOverlays::new(step)?.for_encoding(&encoding_from(g)?)
    }
}

const OVERLAYS_OPTIONS: [OptionSpec; 8] = with_encoding([OptionSpec::required(
    "step",
    "0",
    "metres, finite and 0 or more: between stacked overlays (0: none lifted, none ordered)",
)]);

/// `prepare-reskate-overlays`'s declaration ([`primase_core::pass::PassRegistry::add`]): the step is the program's, no
/// default; the encoding the writer's unless stated.
pub const PREPARE_RESKATE_OVERLAYS: PassSpec = PassSpec {
    name: "prepare-reskate-overlays",
    options: &OVERLAYS_OPTIONS,
    make: |g| Ok(Box::new(PrepareReskateOverlays::from_given(g)?)),
};

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
        if w.resource::<SkateOverlayStack>().is_some() {
            return Err(
                "prepare-reskate-overlays: the overlays are already stacked (skate.overlay_stack)".into(),
            );
        }
        let mut rec = overlay_stack(w, &self.encoding, self.step).map_err(|e| e.to_string())?;
        // a part: its own meshes' pieces (its placements', the prototypes it draws)
        let owned = super::Owned::of(w);
        rec.pieces.retain(|p, _| owned.contains(p.entity()));
        rec.under
            .retain(|(a, b)| owned.contains(a.entity()) && owned.contains(b.entity()));
        let kept: std::collections::BTreeSet<&str> = rec.pieces.values().map(|p| p.mesh.as_str()).collect();
        rec.meshes.retain(|m, _| kept.contains(m.as_str()));
        rec.left.retain(|(m, ..), _| kept.contains(m.as_str()));
        rec.totals = crate::map::sum_totals(rec.meshes.values());
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
            // which: per (material, the material it is left against), over the meshes (a piece with several such
            // surfaces counts in each pair)
            let mut pairs: std::collections::BTreeMap<(&str, &str), u64> = Default::default();
            for ((_, a, b), n) in &rec.left {
                *pairs.entry((a, b)).or_default() += u64::from(*n);
            }
            for ((a, b), n) in pairs {
                log.count(
                    &format!("overlay pieces left in the unclear band, by material and the material they are against: {a} against {b}"),
                    n,
                );
            }
        }
        w.insert_resource(rec);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What each encoding option's absence states is the writer's default, the value [`EncodingOptions::default`]
    /// holds.
    #[test]
    fn the_encoding_options_state_the_writers_defaults() {
        let d = EncodingOptions::default();
        for (spec, value) in [
            (TILE, d.tile.to_string()),
            (MIN_INSTANCES, d.min_instances.to_string()),
            (MIN_INSTANCE_TRIS, d.min_instance_tris.to_string()),
            (LIMIT, d.limit.to_string()),
            (OFFSET_X, d.offset[0].to_string()),
            (OFFSET_Y, d.offset[1].to_string()),
        ] {
            assert_eq!(
                spec.absent(),
                Some(format!("{value}, the writer's").as_str()),
                "{}",
                spec.key()
            );
        }
        assert_eq!(d.max_render_edge, None);
        assert_eq!(MAX_RENDER_EDGE.absent(), Some("the scene's"));
    }
}
