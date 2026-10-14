use crate::error::WorkerError;

pub(super) fn shared_length(vectors: &[Vec<f32>]) -> Option<usize> {
    vectors
        .iter()
        .find(|vector| !vector.is_empty())
        .map(Vec::len)
}

pub fn mean_pool(vectors: &[Vec<f32>], width: usize) -> Result<Option<Vec<f32>>, WorkerError> {
    if width == 0 {
        return Err(WorkerError::Width);
    }
    let Some(pooled_len) = shared_length(vectors) else {
        return Ok(None);
    };
    let mut acc = vec![0.0_f32; pooled_len];
    let mut count: u32 = 0;
    for vector in vectors {
        if vector.len() == pooled_len {
            for (slot, value) in acc.iter_mut().zip(vector) {
                *slot += *value;
            }
            count = next_count(count)?;
        }
    }
    if count == 0 {
        return Ok(None);
    }
    let divisor = divisor(count)?;
    for slot in &mut acc {
        *slot /= divisor;
    }
    if acc.len() > width {
        acc.truncate(width);
    }
    Ok(Some(acc))
}

fn next_count(count: u32) -> Result<u32, WorkerError> {
    let next = count.checked_add(1).ok_or(WorkerError::TextTooLong)?;
    u16::try_from(next).map_err(|_| WorkerError::TextTooLong)?;
    Ok(next)
}

fn divisor(count: u32) -> Result<f32, WorkerError> {
    let narrow = u16::try_from(count).map_err(|_| WorkerError::TextTooLong)?;
    Ok(f32::from(narrow))
}
