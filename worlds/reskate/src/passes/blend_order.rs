//! `prepare-reskate-blend-order{grid,order}`: the vertex-blend overlays of the map's render tiles (a lowered blend's chain
//! layers and the vertex-alpha decals) made objects of their own, drawn in rank order, as the writer used to make them
//! while it wrote.
//!
//! skate. draws a tile's blend layers and overlapping decals in an unstable order when they share its mesh. This pass
//! assembles the writer's tiles with the writer's own code (textures identified but not encoded) and, per tile with
//! such overlays ([`crate::blend_order`]: the chain's ranks, then each decal over the decals it overlaps that the tile
//! drew first), splits them by dominant axis, cuts them on a `grid`-metre grid (Studio's 200 m
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
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};

use crate::assembly::{EncodingOptions, blend_chains};
use crate::blend_order::ChainOrder;
use crate::components::SkateBlendChains;

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskateBlendOrder {
    grid: f32,
    order: ChainOrder,
    encoding: EncodingOptions,
}

impl PrepareReskateBlendOrder {
    /// Chain objects cut on a `grid`-metre grid (finite, 0 or more; 0: not cut), their ranks told apart by `order`
    /// ([`ChainOrder`]), for a map written with the writer's default encoding ([`EncodingOptions::default`]).
    pub fn new(grid: f32, order: ChainOrder) -> Result<PrepareReskateBlendOrder, String> {
        if !(grid.is_finite() && grid >= 0.0) {
            return Err(format!("grid={grid}: finite metres of 0 or more (0: not cut)"));
        }
        Ok(PrepareReskateBlendOrder {
            grid,
            order,
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

    pub fn order(&self) -> ChainOrder {
        self.order
    }

    /// From its options: `grid`, `order`, and the encoding's as `prepare-reskate-overlays` takes them.
    fn from_given(g: &Given) -> Result<PrepareReskateBlendOrder, String> {
        let grid = g.required("grid");
        let grid: f32 = grid.parse().map_err(|_| format!("grid={grid} is not a number"))?;
        let e = crate::passes::overlays::encoding_from(g)?;
        PrepareReskateBlendOrder::new(grid, g.choice("order"))?.for_encoding(&e)
    }
}

const BLEND_ORDER_OPTIONS: [OptionSpec; 9] = crate::passes::overlays::with_encoding([
    OptionSpec::required(
        "grid",
        "0",
        "metres, finite and 0 or more: the grid chain objects are cut on (0: not cut)",
    ),
    OptionSpec::choice::<ChainOrder>(
        "order",
        "how a tile's ranks are told apart in skate.'s sort: tie, one box and 0.2 mm lifts (measured on walls), or \
         stacked, each rank's box its own, metres apart and behind the surfaces",
    ),
]);

/// `prepare-reskate-blend-order`'s declaration ([`primase_core::pass::PassRegistry::add`]): the grid is the program's,
/// no default; the encoding the writer's unless stated.
pub const PREPARE_RESKATE_BLEND_ORDER: PassSpec = PassSpec {
    name: "prepare-reskate-blend-order",
    options: &BLEND_ORDER_OPTIONS,
    make: |g| Ok(Box::new(PrepareReskateBlendOrder::from_given(g)?)),
};

impl Pass for PrepareReskateBlendOrder {
    fn name(&self) -> &'static str {
        "prepare-reskate-blend-order"
    }
    fn options(&self) -> Options {
        let mut o = crate::passes::overlays::encoding_options(&self.encoding);
        o.insert("grid".into(), self.grid.to_string());
        o.insert("order".into(), primase_core::pass::Choice::text(self.order).into());
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
        let mut rec = blend_chains(w, &self.encoding, self.grid, self.order).map_err(|e| e.to_string())?;
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
