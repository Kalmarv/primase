//! Frostbite's lowerings: its dialect to core meanings, each choice the program's.

pub mod buffer_tables;
pub mod clip_position;
mod resolve_shaders;

pub use resolve_shaders::{RESOLVE_FROSTBITE_SHADERS, ResolveFrostbiteShaders};

use primase_core::pass::PassRegistry;

/// Register Frostbite's passes by name.
pub fn register(reg: &mut PassRegistry) {
    reg.add(&RESOLVE_FROSTBITE_SHADERS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::PassManager;

    /// The passes are registered by their declarations, with no options, and a pipeline names them.
    #[test]
    fn the_frostbite_passes_register_by_name() {
        let mut registry = PassRegistry::new();
        register(&mut registry);
        let manager = PassManager::new(registry);
        assert!(manager.registry.parse("resolve-frostbite-shaders").is_ok());
        assert!(manager.registry.parse("resolve-frostbite-shaders{x=y}").is_err());
        assert!(!manager.registry.requires_options("resolve-frostbite-shaders"));
        assert!(
            manager
                .registry
                .describe("resolve-frostbite-shaders")
                .is_some_and(|d| d == "resolve-frostbite-shaders")
        );
    }
}
