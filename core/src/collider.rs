//! Collision as the source states it, before any triangles: shapes (`core.collider`) and heightfields
//! (`core.heightfield`). Lowering them to `core.collision` triangles is a pass (`tessellate-colliders`) that only the
//! outputs needing triangles run; the shapes follow Khronos `KHR_implicit_shapes`, so a glTF output can carry them.

use serde::Serialize;
use serde_json::{Value, json};

use crate::{Component, Entity, JsonError, Surfaces, World};

/// `core.collider`: a collision shape in the entity's own space, placed like `core.collision` (on a prototype, by its
/// instances; else by the entity's transform). The geometry is exact under the entity's full transform: a sphere
/// under a non-uniform scale is an ellipsoid. A source whose physics treats scale otherwise (PhysX scales a sphere's
/// radius by its largest axis) has its lowering pass choose the shape and transform that collide as the game does.
#[derive(Clone, Debug, PartialEq)]
pub struct Collider {
    pub shape: Shape,
    /// Into the world's `core.surfaces` (a heightfield's per-cell surfaces override it).
    pub surface: u16,
}

/// A collider's shape, in the entity's space, metres.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// Centred on `center`, axis-aligned in the entity's space.
    Box {
        center: [f32; 3],
        half_extents: [f32; 3],
    },
    Sphere {
        center: [f32; 3],
        radius: f32,
    },
    /// The segment `a`-`b` swept by `radius`: the caps are included in its length.
    Capsule {
        a: [f32; 3],
        b: [f32; 3],
        radius: f32,
    },
    /// A cylinder from `a` to `b` (a disc is a short one).
    Cylinder {
        a: [f32; 3],
        b: [f32; 3],
        radius: f32,
    },
    /// The convex hull of the points (no faces are stored: a consumer builds the hull).
    Convex {
        points: Vec<[f32; 3]>,
    },
    /// The entity's [`Heightfield`].
    Heightfield,
}

/// Which corners a heightfield cell's two triangles share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Diagonal {
    /// The cell's (column, row) corner and its (column + 1, row + 1) corner.
    Main,
    /// The cell's (column + 1, row) corner and its (column, row + 1) corner.
    Anti,
}

/// `core.heightfield`: a regular grid of heights in the entity's space: `columns` x `rows` samples along +X and +Z
/// from the origin, `spacing` metres apart (x, z), each a height along +Y. Cells are split into two triangles along
/// `diagonal`. A terrain's drawn surface and its collision are the same grid: drawing it is a later pass; colliding
/// with it is a [`Collider`] of shape [`Shape::Heightfield`] on the same entity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Heightfield {
    pub columns: u32,
    pub rows: u32,
    pub spacing: [f32; 2],
    /// Metres, row by row (a row along +X, rows along +Z).
    pub heights: Vec<f32>,
    pub diagonal: Diagonal,
    /// One per cell (row by row, `(columns - 1) * (rows - 1)`), true where there is no surface.
    pub holes: Option<Vec<bool>>,
    /// One per cell into `core.surfaces`; `None`: the collider's surface.
    pub surfaces: Option<Vec<u16>>,
}

impl Heightfield {
    /// The number of cells, `(columns - 1) * (rows - 1)`.
    pub fn cells(&self) -> usize {
        (self.columns.saturating_sub(1) as usize) * (self.rows.saturating_sub(1) as usize)
    }
}

fn finite(values: impl IntoIterator<Item = f32>, what: &str) -> Result<(), String> {
    match values.into_iter().find(|v| !v.is_finite()) {
        Some(v) => Err(format!("{what}: {v} is not finite")),
        None => Ok(()),
    }
}

fn positive(v: f32, what: &str) -> Result<(), String> {
    if v.is_finite() && v > 0.0 {
        Ok(())
    } else {
        Err(format!("{what} {v} is not positive"))
    }
}

fn surface_known(w: &World, s: u16) -> Result<(), String> {
    let known = w.resource::<Surfaces>().map_or(0, |t| t.0.len());
    if usize::from(s) < known {
        Ok(())
    } else {
        Err(format!("surface {s} past the world's {known} surfaces"))
    }
}

impl Component for Collider {
    const NAME: &'static str = "core.collider";
    /// The shape in full, a convex shape's points as a count.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        let shape = match &self.shape {
            Shape::Convex { points } => json!({"convex": {"points": points.len()}}),
            other => crate::json::to_value(other)?,
        };
        Ok(Some(json!({"shape": shape, "surface": self.surface})))
    }
    /// Finite values, positive extents and radii, a known surface; a heightfield shape needs the entity's
    /// `core.heightfield`, a convex one a point.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        surface_known(w, self.surface)?;
        match &self.shape {
            Shape::Box { center, half_extents } => {
                finite(*center, "box centre")?;
                half_extents
                    .iter()
                    .try_for_each(|&h| positive(h, "box half extent"))
            }
            Shape::Sphere { center, radius } => {
                finite(*center, "sphere centre")?;
                positive(*radius, "sphere radius")
            }
            Shape::Capsule { a, b, radius } | Shape::Cylinder { a, b, radius } => {
                finite(a.iter().chain(b).copied(), "axis ends")?;
                positive(*radius, "radius")
            }
            Shape::Convex { points } => {
                if points.is_empty() {
                    return Err("a convex shape without points".into());
                }
                finite(points.iter().flatten().copied(), "convex points")
            }
            Shape::Heightfield => {
                if w.has::<Heightfield>(e) {
                    Ok(())
                } else {
                    Err("a heightfield shape without the entity's core.heightfield".into())
                }
            }
        }
    }
}

impl Component for Heightfield {
    const NAME: &'static str = "core.heightfield";
    /// The grid's shape and options, the heights, holes and surfaces as counts.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "columns": self.columns,
            "rows": self.rows,
            "spacing": crate::json::to_value(&self.spacing)?,
            "heights": self.heights.len(),
            "diagonal": crate::json::to_value(&self.diagonal)?,
            "holes": self.holes.as_ref().map(|h| h.iter().filter(|&&x| x).count()),
            "surfaces": self.surfaces.as_ref().map(Vec::len),
        })))
    }
    /// At least 2 x 2 samples, positive spacing, finite heights, one height per sample, and holes and surfaces (known
    /// to the world) one per cell.
    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        if self.columns < 2 || self.rows < 2 {
            return Err(format!(
                "{} x {} samples: a heightfield has at least 2 x 2",
                self.columns, self.rows
            ));
        }
        self.spacing.iter().try_for_each(|&s| positive(s, "spacing"))?;
        let samples = self.columns as usize * self.rows as usize;
        if self.heights.len() != samples {
            return Err(format!("{} heights for {samples} samples", self.heights.len()));
        }
        finite(self.heights.iter().copied(), "heights")?;
        let cells = self.cells();
        if let Some(h) = &self.holes
            && h.len() != cells
        {
            return Err(format!("{} holes for {cells} cells", h.len()));
        }
        if let Some(s) = &self.surfaces {
            if s.len() != cells {
                return Err(format!("{} surfaces for {cells} cells", s.len()));
            }
            s.iter().try_for_each(|&s| surface_known(w, s))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let mut w = World::new();
        let mut s = Surfaces::default();
        s.index("concrete").unwrap();
        w.insert_resource(s);
        w
    }

    fn field(columns: u32, rows: u32) -> Heightfield {
        Heightfield {
            columns,
            rows,
            spacing: [1.0, 2.0],
            heights: vec![0.0; (columns * rows) as usize],
            diagonal: Diagonal::Main,
            holes: None,
            surfaces: None,
        }
    }

    #[test]
    fn shapes_verify_their_sizes_surfaces_and_heightfield() {
        let mut w = world();
        let ok = [
            Shape::Box {
                center: [0.0; 3],
                half_extents: [1.0, 2.0, 3.0],
            },
            Shape::Sphere {
                center: [0.0; 3],
                radius: 0.5,
            },
            Shape::Capsule {
                a: [0.0; 3],
                b: [0.0, 2.0, 0.0],
                radius: 0.5,
            },
            Shape::Convex {
                points: vec![[0.0; 3]],
            },
        ];
        for shape in ok {
            let e = w.spawn();
            w.insert(e, Collider { shape, surface: 0 });
        }
        let terrain = w.spawn();
        w.insert(terrain, field(3, 2));
        w.insert(
            terrain,
            Collider {
                shape: Shape::Heightfield,
                surface: 0,
            },
        );
        assert!(w.verify_components().is_empty(), "{:?}", w.verify_components());
        let bad = [
            (
                Shape::Sphere {
                    center: [0.0; 3],
                    radius: 0.0,
                },
                0,
                "sphere radius 0 is not positive",
            ),
            (
                Shape::Box {
                    center: [f32::NAN, 0.0, 0.0],
                    half_extents: [1.0; 3],
                },
                0,
                "box centre: NaN is not finite",
            ),
            (
                Shape::Convex { points: Vec::new() },
                0,
                "a convex shape without points",
            ),
            (
                Shape::Heightfield,
                0,
                "a heightfield shape without the entity's core.heightfield",
            ),
            (
                Shape::Sphere {
                    center: [0.0; 3],
                    radius: 1.0,
                },
                1,
                "surface 1 past the world's 1 surfaces",
            ),
        ];
        for (shape, surface, message) in bad {
            let mut w = world();
            let e = w.spawn();
            w.insert(e, Collider { shape, surface });
            assert_eq!(w.verify_components(), [("core.collider", e, message.to_owned())]);
        }
    }

    #[test]
    fn a_heightfield_has_a_height_per_sample_and_a_hole_per_cell() {
        for (f, message) in [
            (field(1, 2), "1 x 2 samples: a heightfield has at least 2 x 2"),
            (
                Heightfield {
                    heights: vec![0.0; 5],
                    ..field(3, 2)
                },
                "5 heights for 6 samples",
            ),
            (
                Heightfield {
                    holes: Some(vec![false; 3]),
                    ..field(3, 2)
                },
                "3 holes for 2 cells",
            ),
            (
                Heightfield {
                    surfaces: Some(vec![0, 4]),
                    ..field(3, 2)
                },
                "surface 4 past the world's 1 surfaces",
            ),
        ] {
            let mut w = world();
            let e = w.spawn();
            w.insert(e, f);
            assert_eq!(
                w.verify_components(),
                [("core.heightfield", e, message.to_owned())]
            );
        }
    }

    #[test]
    fn json_shows_shapes_and_counts() {
        let c = Collider {
            shape: Shape::Capsule {
                a: [0.0; 3],
                b: [0.0, 1.0, 0.0],
                radius: 0.25,
            },
            surface: 0,
        };
        assert_eq!(
            c.to_json().unwrap(),
            Some(
                json!({"shape": {"capsule": {"a": [0.0, 0.0, 0.0], "b": [0.0, 1.0, 0.0], "radius": 0.25}}, "surface": 0})
            )
        );
        let c = Collider {
            shape: Shape::Convex {
                points: vec![[0.0; 3]; 4],
            },
            surface: 0,
        };
        assert_eq!(
            c.to_json().unwrap(),
            Some(json!({"shape": {"convex": {"points": 4}}, "surface": 0}))
        );
        let h = Heightfield {
            holes: Some(vec![true, false]),
            ..field(3, 2)
        };
        assert_eq!(
            h.to_json().unwrap(),
            Some(
                json!({"columns": 3, "rows": 2, "spacing": [1.0, 2.0], "heights": 6, "diagonal": "main", "holes": 1, "surfaces": null})
            )
        );
    }
}
