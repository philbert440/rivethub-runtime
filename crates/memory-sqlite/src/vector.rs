pub(crate) fn normalize(vector: &mut [f32]) -> bool {
    let norm = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return false;
    }
    for value in vector {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "B5 embeddings are stored as normalized float32 values"
        )]
        let normalized = (f64::from(*value) / norm) as f32;
        *value = normalized;
    }
    true
}

pub(crate) fn pack(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
