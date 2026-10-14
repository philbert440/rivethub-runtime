const DECAY_LAMBDA: f64 = 0.05;
const REINFORCEMENT_ALPHA: f64 = 0.01;
const REINFORCEMENT_CAP: u32 = 25;
const W_FTS: f64 = 0.3;
const W_SEMANTIC: f64 = 0.3;
const W_TEMPORAL: f64 = 0.3;
const W_IMPORTANCE: f64 = 0.1;

#[must_use]
pub fn recency_boost(age_days: f64, access_count: u32, importance: f64) -> f64 {
    temporal_decay(age_days, access_count).mul_add(W_TEMPORAL, importance * W_IMPORTANCE)
}

#[must_use]
pub fn composite_score(arm_score: f64, semantic: f64, boost: f64) -> f64 {
    arm_score.mul_add(W_FTS, semantic.mul_add(W_SEMANTIC, boost))
}

fn temporal_decay(age_days: f64, access_count: u32) -> f64 {
    let reinforced = f64::from(access_count.min(REINFORCEMENT_CAP));
    (-DECAY_LAMBDA * age_days).exp() * REINFORCEMENT_ALPHA.mul_add(reinforced, 1.0)
}

#[cfg(test)]
mod tests {
    use super::composite_score;
    use super::recency_boost;
    use super::temporal_decay;

    const TOLERANCE: f64 = 1e-12;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= TOLERANCE,
            "{actual} is not within {TOLERANCE} of {expected}"
        );
    }

    #[test]
    fn a_fresh_unread_row_keeps_its_full_weight() {
        assert_close(temporal_decay(0.0, 0), 1.0);
    }

    #[test]
    fn decay_halves_after_the_half_life() {
        let half_life = 2.0_f64.ln() / 0.05;
        assert_close(temporal_decay(half_life, 0), 0.5);
        assert_close(temporal_decay(2.0 * half_life, 0), 0.25);
    }

    #[test]
    fn decay_falls_monotonically_with_age() {
        let ages = [0.0, 0.5, 1.0, 7.0, 30.0, 365.0];
        for (younger, older) in ages.iter().zip(ages.iter().skip(1)) {
            assert!(temporal_decay(*younger, 3) > temporal_decay(*older, 3));
        }
    }

    #[test]
    fn each_access_adds_one_percent_up_to_the_cap() {
        assert_close(temporal_decay(0.0, 1), 1.01);
        assert_close(temporal_decay(0.0, 10), 1.1);
        assert_close(temporal_decay(0.0, 25), 1.25);
        assert_close(temporal_decay(0.0, 26), 1.25);
        assert_close(temporal_decay(0.0, u32::MAX), 1.25);
    }

    #[test]
    fn reinforcement_scales_the_decayed_weight() {
        let age = 10.0;
        assert_close(temporal_decay(age, 5), temporal_decay(age, 0) * 1.05);
    }

    #[test]
    fn recency_boost_weighs_decay_and_importance() {
        assert_close(recency_boost(0.0, 0, 0.0), 0.3);
        assert_close(recency_boost(0.0, 0, 0.7), 0.37);
        assert_close(recency_boost(0.0, 25, 0.5), 0.425);
        assert_close(recency_boost(f64::INFINITY, 0, 0.6), 0.06);
    }

    #[test]
    fn composite_score_weighs_the_arm_and_semantic_scores_and_adds_the_boost() {
        assert_close(composite_score(0.0, 0.0, 0.0), 0.0);
        assert_close(composite_score(1.0, 0.0, 0.0), 0.3);
        assert_close(composite_score(0.0, 1.0, 0.0), 0.3);
        assert_close(composite_score(0.5, 0.25, 0.1), 0.325);
    }
}
