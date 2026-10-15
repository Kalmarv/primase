/// Derived coordinates use four f32 epsilon units at max(1, |reference|).
/// This is a scale-aware bound: cancellation can make a small result differ by
/// many component ULPs even when the original operations differ by a few ULPs.
#[derive(Default, Debug)]
pub struct Floats {
    pub checked: u64,
    pub exact: u64,
    pub within: u64,
    pub max_abs: f64,
    pub max_scaled_epsilon: f64,
}
impl Floats {
    pub fn compare(&mut self, a: f32, b: f32) {
        self.compare_at_scale(a, b, b.abs().max(1.));
    }
    pub fn compare_at_scale(&mut self, a: f32, b: f32, scale: f32) {
        self.checked += 1;
        let diff = (a as f64 - b as f64).abs();
        self.max_abs = self.max_abs.max(diff);
        self.max_scaled_epsilon = self
            .max_scaled_epsilon
            .max(diff / (f32::EPSILON as f64 * scale as f64));
        if a == b {
            self.exact += 1;
        } else {
            let budget = 4. * f32::EPSILON as f64 * scale as f64;
            assert!(
                a.is_finite() && b.is_finite() && diff <= budget,
                "derived value {a} vs {b}: {diff} > {budget}"
            );
            self.within += 1;
        }
    }
    pub fn report(&self, label: &str) {
        eprintln!(
            "{label}: checked={} exact={} within_4_epsilon_scaled={} failed=0 max_abs={} max_scaled_epsilon={}",
            self.checked, self.exact, self.within, self.max_abs, self.max_scaled_epsilon
        );
    }
}
