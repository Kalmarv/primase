//! Explicit scheduling choices for any simulation implementation.
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::{Resource, World};
use serde::Serialize;

/// Fixed step, startup settling, and per-frame work limit, supplied by a program's registered pass.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SimulationSchedule {
    pub step_seconds: f64,
    pub pre_roll_steps: u32,
    pub max_steps_per_frame: u32,
}
impl SimulationSchedule {
    pub fn new(step_seconds: f64, pre_roll_steps: u32, max_steps_per_frame: u32) -> Result<Self, String> {
        if !step_seconds.is_finite() || step_seconds <= 0. {
            return Err("simulation step-seconds must be finite and positive".into());
        }
        if max_steps_per_frame == 0 {
            return Err("simulation max-steps-per-frame must be positive".into());
        }
        Ok(Self {
            step_seconds,
            pre_roll_steps,
            max_steps_per_frame,
        })
    }
}
impl Resource for SimulationSchedule {
    const NAME: &'static str = "animation.simulation_schedule";
    fn verify(&self, _: &World) -> Result<(), String> {
        Self::new(self.step_seconds, self.pre_roll_steps, self.max_steps_per_frame).map(|_| ())
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// State every scheduling choice rather than imposing an engine rate in a viewer.
#[derive(Clone, Copy, Debug)]
pub struct ConfigureSimulations(pub SimulationSchedule);
impl ConfigureSimulations {
    fn from_given(g: &Given) -> Result<Self, String> {
        let seconds = g
            .required("step-seconds")
            .parse()
            .map_err(|_| "step-seconds is not a number")?;
        let pre_roll = g
            .required("pre-roll")
            .parse()
            .map_err(|_| "pre-roll is not a step count")?;
        let max_steps = g
            .required("max-steps-per-frame")
            .parse()
            .map_err(|_| "max-steps-per-frame is not a step count")?;
        Ok(Self(SimulationSchedule::new(seconds, pre_roll, max_steps)?))
    }
}
/// Registered scheduling operation. The program owns all three choices; no reader or client fills them implicitly.
pub const CONFIGURE_SIMULATIONS: PassSpec = PassSpec {
    name: "configure-simulations",
    options: &[
        OptionSpec::required(
            "step-seconds",
            "0.03333333333333333",
            "fixed simulation timestep in seconds",
        ),
        OptionSpec::required(
            "pre-roll",
            "30",
            "steps used to settle a newly started simulation",
        ),
        OptionSpec::required("max-steps-per-frame", "8", "per-frame catch-up work cap"),
    ],
    make: |g| Ok(Box::new(ConfigureSimulations::from_given(g)?)),
};
impl Pass for ConfigureSimulations {
    fn name(&self) -> &'static str {
        "configure-simulations"
    }
    fn options(&self) -> Options {
        Options::from([
            ("step-seconds".into(), self.0.step_seconds.to_string()),
            ("pre-roll".into(), self.0.pre_roll_steps.to_string()),
            (
                "max-steps-per-frame".into(),
                self.0.max_steps_per_frame.to_string(),
            ),
        ])
    }
    fn provides(&self) -> &[&'static str] {
        &[SimulationSchedule::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[SimulationSchedule::NAME]
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        self.0.verify(world)?;
        world.insert_resource(self.0);
        log.count("simulation schedules stated", 1);
        log.count("simulation pre-roll steps", u64::from(self.0.pre_roll_steps));
        log.count(
            "simulation steps allowed per frame",
            u64::from(self.0.max_steps_per_frame),
        );
        Ok(())
    }
}
