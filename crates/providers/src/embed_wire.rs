use serde::Deserialize;
use wire::JsonObject;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EmbedPayload {
    Body(EmbedBody),
    Text(String),
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedBody {
    #[serde(default)]
    pub data: Option<EmbedData>,
    #[serde(default)]
    pub embeddings: Option<Vec<Option<EmbeddingField>>>,
    #[serde(default)]
    pub vectors: Option<Vec<Option<EmbeddingField>>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EmbedData {
    Rows(Vec<Option<EmbedRow>>),
    Text(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbedRow {
    #[serde(default)]
    pub embedding: Option<EmbeddingField>,
    #[serde(default)]
    pub index: Option<IndexField>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EmbeddingField {
    Vector(Vec<Option<Cell>>),
    Text(String),
    Object(JsonObject),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Cell {
    Number(f64),
    Text(String),
    Object(JsonObject),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum IndexField {
    Whole(i64),
    Fraction(f64),
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizeRejection {
    NotNumbers,
    DimsMismatch { got: usize, expected: u32 },
}

#[must_use]
pub fn parse_embed_response(payload: Option<&EmbedPayload>, count: usize) -> Vec<Option<Vec<f64>>> {
    let mut vectors: Vec<Option<Vec<f64>>> = vec![None; count];
    if let Some(EmbedPayload::Body(body)) = payload {
        if let Some(rows) = &body.embeddings {
            fill_positional(&mut vectors, rows);
        } else if let Some(rows) = &body.vectors {
            fill_positional(&mut vectors, rows);
        } else if let Some(EmbedData::Rows(rows)) = &body.data {
            fill_indexed(&mut vectors, rows);
        }
    }
    vectors
}

fn fill_positional(vectors: &mut [Option<Vec<f64>>], rows: &[Option<EmbeddingField>]) {
    for (slot, row) in rows.iter().enumerate().take(vectors.len()) {
        if let Some(slot_vector) = vectors.get_mut(slot) {
            *slot_vector = embedding_value(row.as_ref());
        }
    }
}

fn fill_indexed(vectors: &mut [Option<Vec<f64>>], rows: &[Option<EmbedRow>]) {
    for row in rows.iter().flatten() {
        let Some(vector) = embedding_value(row.embedding.as_ref()) else {
            continue;
        };
        let slot = match &row.index {
            Some(IndexField::Whole(slot)) => *slot,
            Some(IndexField::Fraction(_)) => continue,
            Some(IndexField::Text(_)) | None => 0,
        };
        let Some(slot) = usize::try_from(slot)
            .ok()
            .filter(|slot| *slot < vectors.len())
        else {
            continue;
        };
        if let Some(slot_vector) = vectors.get_mut(slot) {
            *slot_vector = Some(vector);
        }
    }
}

fn embedding_value(field: Option<&EmbeddingField>) -> Option<Vec<f64>> {
    let Some(EmbeddingField::Vector(cells)) = field else {
        return None;
    };
    if cells.is_empty() {
        return None;
    }
    cells
        .iter()
        .map(|cell| match cell {
            Some(Cell::Number(value)) => Some(*value),
            Some(Cell::Text(_) | Cell::Object(_)) | None => None,
        })
        .collect()
}

pub fn check_dims(count: usize, expected_dims: Option<u32>) -> Result<(), NormalizeRejection> {
    if let Some(dims) = expected_dims
        && usize::try_from(dims).is_ok_and(|dims| count != dims)
    {
        return Err(NormalizeRejection::DimsMismatch {
            got: count,
            expected: dims,
        });
    }
    Ok(())
}

pub fn normalize_embed_vector_checked(
    vector: Option<&[Option<Cell>]>,
    expected_dims: Option<u32>,
    truncate_dims: Option<u32>,
) -> Result<Vec<f64>, NormalizeRejection> {
    let Some(cells) = vector else {
        return Err(NormalizeRejection::NotNumbers);
    };
    let mut values = Vec::with_capacity(cells.len());
    for cell in cells {
        match cell {
            Some(Cell::Number(value)) => values.push(*value),
            Some(Cell::Text(_) | Cell::Object(_)) | None => {
                return Err(NormalizeRejection::NotNumbers);
            }
        }
    }
    check_dims(values.len(), expected_dims)?;
    if let Some(dims) = truncate_dims
        && let Ok(dims) = usize::try_from(dims)
        && values.len() > dims
    {
        values.truncate(dims);
    }
    Ok(values)
}

#[must_use]
pub fn normalize_embed_vector(
    vector: Option<&[Option<Cell>]>,
    expected_dims: Option<u32>,
    truncate_dims: Option<u32>,
) -> Option<Vec<f64>> {
    normalize_embed_vector_checked(vector, expected_dims, truncate_dims).ok()
}
