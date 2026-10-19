//! `prepare-reskate-blend-order{grid}`: the vertex-blend chain overlays of the map's render tiles made objects of their
//! own, drawn in chain order, as the writer used to make them while it wrote.
//!
//! skate. draws a tile's blend layers in an unstable order when they share its mesh. This pass assembles the writer's
//! tiles with the writer's own code (textures identified but not encoded) and, per tile with chain overlays
//! ([`crate::blend_order`]), splits them by dominant axis, cuts them on a `grid`-metre grid (Studio's 200 m
//! WorldPartition cell; 0: not cut), pads every rank of a cell and axis to one box and takes each axis's lift step (the
//! rank's objects lifted apart along it, the step widened where an f32 cannot hold it at the tile; a tile where no step
//! up to the largest holds is refused, its chain overlays left in its mesh: counted, an approximation). It records the
//! result by tile as `skate.blend_chains`: the pieces, the steps, and each object's materials, triangles clipped and
//! box, with the writer settings the tiles depend on. The writer builds the chain form from it (and the plain form a
//! tile takes past the object budget, which stays the writer's until the global preparation) and refuses tiles that
//! differ. Without this pass every tile's chain overlays stay in its mesh (reported). A map without chain overlays gets
//! an empty record. Run it after every pass that changes geometry, materials or textures.
//!
//! A part of a map converted in several records its own render tiles, as the writer lays them out (the map's instancing
//! and region, merged over every part).

use primase_core::World;
use primase_core::pass::{Options, Pass, PassLog};

use crate::assembly::{EncodingOptions, blend_chains};
use crate::components::SkateBlendChains;

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskateBlendOrder {
    grid: f32,
    encoding: EncodingOptions,
}

impl PrepareReskateBlendOrder {
    /// Chain objects cut on a `grid`-metre grid (finite, 0 or more; 0: not cut), for a map written with the writer's
    /// default encoding ([`EncodingOptions::default`]).
    pub fn new(grid: f32) -> Result<PrepareReskateBlendOrder, String> {
        if !(grid.is_finite() && grid >= 0.0) {
            return Err(format!("grid={grid}: finite metres of 0 or more (0: not cut)"));
        }
        Ok(PrepareReskateBlendOrder {
            grid,
            encoding: EncodingOptions::default(),
        })
    }

    /// For the encoding the map will be written with (its render tile, instancing, limit and offset), checked as
    /// `prepare-reskate-overlays` checks it.
    pub fn for_encoding(mut self, e: &EncodingOptions) -> Result<PrepareReskateBlendOrder, String> {
        crate::passes::overlays::check_grouping(e)?;
        self.encoding = crate::passes::overlays::grouping_only(e);
        Ok(self)
    }

    pub fn grid(&self) -> f32 {
        self.grid
    }

    /// From its options: `grid` required; the encoding's as `prepare-reskate-overlays` takes them.
    pub fn from_options(o: &Options) -> Result<PrepareReskateBlendOrder, String> {
        let mut rest = o.clone();
        let grid = rest
            .remove("grid")
            .ok_or("grid is required (metres of the grid chain objects are cut on; 0: not cut)")?;
        let grid: f32 = grid.parse().map_err(|_| format!("grid={grid} is not a number"))?;
        let e = crate::passes::overlays::encoding_of(&rest)?;
        PrepareReskateBlendOrder::new(grid)?.for_encoding(&e)
    }
}

/// [`PrepareReskateBlendOrder::from_options`] as a pass factory.
pub fn prepare_reskate_blend_order(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateBlendOrder::from_options(o)?))
}

impl Pass for PrepareReskateBlendOrder {
    fn name(&self) -> &'static str {
        "prepare-reskate-blend-order"
    }
    fn options(&self) -> Options {
        let mut o = crate::passes::overlays::encoding_options(&self.encoding);
        o.insert("grid".into(), self.grid.to_string());
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.blend_chains"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if w.resource::<SkateBlendChains>().is_some() {
            return Err(
                "prepare-reskate-blend-order: the blend chains are already chosen (skate.blend_chains)"
                    .into(),
            );
        }
        let mut rec = blend_chains(w, &self.encoding, self.grid).map_err(|e| e.to_string())?;
        // a part: its own tiles (each held whole by one part)
        let owned = super::Owned::of(w);
        rec.tiles
            .retain(|_, t| t.pieces.iter().all(|p| owned.contains(p.entity())));
        let (mut tiles, mut objects, mut clipped, mut refused) = (0u64, 0u64, 0u64, 0u64);
        for t in rec.tiles.values() {
            if t.steps.is_none() {
                refused += 1;
                continue;
            }
            tiles += 1;
            objects += t.groups.len() as u64;
            clipped += u64::from(t.clipped);
        }
        log.count("tiles with chain objects", tiles);
        log.count("chain objects", objects);
        log.count("chain triangles clipped at the grid", clipped);
        if refused > 0 {
            log.approximate(
                "tiles whose chain overlays stay in the tile, drawn in an unstable order: no lift step holds as an f32 at the tile",
                refused,
            );
        }
        w.insert_resource(rec);
        Ok(())
    }
}
