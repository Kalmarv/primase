//! The threads a run may use (`core.threads`): [`Threads`] is `primase_foundations::parallel`'s, a run setting the
//! program states on its pass manager ([`crate::pass::Settings`], [`crate::pass::PassManager::with_settings`]), which
//! states it in each world it runs. A pass that runs on them declares [`THREADS`] in its
//! [`crate::pass::Pass::settings`] and reads the world's statement with [`threads`].

pub use primase_foundations::parallel::Threads;

use crate::pass::{RunSetting, Setting, Unstated};
use crate::{Resource, World};

impl Resource for Threads {
    const NAME: &'static str = "core.threads";
}

impl RunSetting for Threads {
    fn text(&self) -> String {
        format!("threads {}", self.get())
    }
}

/// The thread count as a pass's setting ([`crate::pass::Pass::settings`]).
pub const THREADS: Setting = Setting::of::<Threads>();

/// The thread count the world `w` states; refused where it states none, since the program states how it runs: no
/// count stands in for one it did not state.
pub fn threads(w: &World) -> Result<Threads, Unstated> {
    w.resource::<Threads>()
        .copied()
        .ok_or(Unstated { name: Threads::NAME })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world states its thread count, and one that states none is refused by the setting's name.
    #[test]
    fn a_world_states_its_threads() {
        let mut w = World::new();
        let refused = threads(&w).unwrap_err();
        assert_eq!(refused, Unstated { name: "core.threads" });
        assert!(refused.to_string().contains("`core.threads`"), "{refused}");
        w.insert_resource(Threads::new(6).unwrap());
        assert_eq!(threads(&w).unwrap().get(), 6);
        assert_eq!(THREADS.text(&w).as_deref(), Some("threads 6"));
    }
}
