//! Regions of the map the crate names. Hand-picked places, labelled so: each is a rectangle chosen for a gate or a
//! viewer, not read from the game. A caller states the region it reads; naming one here is a way to say it.

/// A named rectangle of the map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    /// The name the crate gives it, stable.
    pub name: &'static str,
    /// What to call it for a person.
    pub label: &'static str,
    /// `[x0, y0, x1, y1]` in the game's units (metres, x east and y north).
    pub rect: [f32; 4],
    /// Why it was picked.
    pub note: &'static str,
}

/// Legion Square: the first gate region.
pub const LEGION: Region = Region {
    name: "legion",
    label: "Legion Square",
    rect: [95.0, -1034.0, 295.0, -834.0],
    note: "the first gate region (hand-picked)",
};

/// Vespucci Beach, from the skatepark along the boardwalk to Muscle Beach: the second gate region.
pub const VESPUCCI: Region = Region {
    name: "vespucci",
    label: "Vespucci Beach",
    rect: [-1500.0, -1650.0, -1150.0, -1330.0],
    note: "the second gate region (hand-picked)",
};

/// The regions the crate names, in the order they were first used.
pub const REGIONS: &[Region] = &[LEGION, VESPUCCI];

/// The region the crate names `name`.
pub fn region_named(name: &str) -> Option<&'static Region> {
    REGIONS.iter().find(|r| r.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rectangles are well formed and the names do not repeat.
    #[test]
    fn the_regions_are_rectangles_with_distinct_names() {
        for r in REGIONS {
            assert!(r.rect[0] < r.rect[2] && r.rect[1] < r.rect[3], "{}", r.name);
            assert_eq!(region_named(r.name), Some(r));
        }
        assert_eq!(REGIONS.len(), 2);
    }
}
