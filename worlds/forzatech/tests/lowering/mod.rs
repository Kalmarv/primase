//! `lower-forzatech-materials` as a program runs it: through the pass manager with its pass checks, so a component
//! the pass adds, removes or edits without declaring it fails the test rather than the first real build. Integration
//! tests reach it with `mod lowering;`.

use primase_core::World;
use primase_core::pass::{PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_forzatech::passes::LowerForzaMaterials;

/// Runs the pass over `world` with the pass checks on and takes its log into `log`.
pub fn lower(world: &mut World, log: &mut PassLog) {
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .with_pass_checks(true)
        .run(
            world,
            &Pipeline {
                passes: vec![Box::new(LowerForzaMaterials)],
            },
            &mut report,
        )
        .expect("lower-forzatech-materials");
    let step = report.steps.pop().expect("the one step");
    log.absorb(step.log);
}
