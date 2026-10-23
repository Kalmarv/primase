//! Frostbite's lowerings: its dialect to core meanings, each choice the program's.

mod resolve_shaders;

pub use resolve_shaders::ResolveFrostbiteShaders;

use primase_core::pass::PassRegistry;

/// Register Frostbite's passes by name.
pub fn register(reg: &mut PassRegistry) {
    reg.register("resolve-frostbite-shaders", |o| {
        Ok(Box::new(ResolveFrostbiteShaders::from_options(o)?))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::PassManager;

    /// The passes are registered by name, with no options, and a pipeline names them.
    #[test]
    fn the_frostbite_passes_register_by_name() {
        let mut registry = PassRegistry::new();
        register(&mut registry);
        let manager = PassManager::new(registry);
        assert!(manager.registry.parse("resolve-frostbite-shaders").is_ok());
        assert!(manager.registry.parse("resolve-frostbite-shaders{x=y}").is_err());
    }
}
