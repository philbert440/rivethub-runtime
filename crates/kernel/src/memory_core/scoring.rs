pub const DECAY_LAMBDA: f64 = 0.05;
pub const REINFORCEMENT_ALPHA: f64 = 0.01;
pub const REINFORCEMENT_CAP: u32 = 25;
pub const W_FTS: f64 = 0.3;
pub const W_SEMANTIC: f64 = 0.3;
pub const W_TEMPORAL: f64 = 0.3;
pub const W_IMPORTANCE: f64 = 0.1;

#[expect(clippy::suboptimal_flops, reason = "mirrors recorded scoring")]
#[must_use]
pub fn temporal_decay(age_days: f64, access_count: f64) -> f64 {
    let reinforced = access_count.min(f64::from(REINFORCEMENT_CAP));
    (-DECAY_LAMBDA * age_days).exp() * (1.0 + REINFORCEMENT_ALPHA * reinforced)
}

#[expect(clippy::suboptimal_flops, reason = "mirrors recorded scoring")]
#[must_use]
pub fn composite_score(arm_score: f64, semantic: f64, boost: f64) -> f64 {
    arm_score * W_FTS + semantic * W_SEMANTIC + boost
}
