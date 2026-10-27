//! The threads a run may use, as the program states them in the world (`core.threads`): [`Threads`] is
//! `primase_foundations::parallel`'s; a pass reads the world's statement with [`threads`].

use primase_foundations::parallel::Threads;

use crate::{Resource, World};

impl Resource for Threads {
    const NAME: &'static str = "core.threads";
}

/// What the world `w` states, one when it states nothing.
pub fn threads(w: &World) -> Threads {
    w.resource::<Threads>().copied().unwrap_or(Threads::ONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world states its thread count; one when it states none.
    #[test]
    fn a_world_states_its_threads() {
        let mut w = World::new();
        assert_eq!(threads(&w), Threads::ONE);
        w.insert_resource(Threads::new(6).unwrap());
        assert_eq!(threads(&w).get(), 6);
    }
}
