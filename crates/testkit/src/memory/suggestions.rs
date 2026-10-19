use kernel::memory::MemoryStoreError;
use kernel::memory::TagSuggestKind;
use kernel::memory::TagSuggestionStored;
use kernel::memory::TagSuggestionWrite;

pub(super) fn count(write: &TagSuggestionWrite) -> Result<TagSuggestionStored, MemoryStoreError> {
    let mut tags = 0u64;
    let mut removals = 0u64;
    for row in &write.rows {
        match row.kind {
            TagSuggestKind::Add => tags = add_count(tags)?,
            TagSuggestKind::Remove => removals = add_count(removals)?,
        }
    }
    Ok(TagSuggestionStored {
        tags,
        removals,
        vocabulary: u64::try_from(write.vocabulary.len()).map_err(|_| {
            MemoryStoreError::InvalidRequest {
                message: "vocabulary count overflowed".to_owned(),
            }
        })?,
    })
}

fn add_count(total: u64) -> Result<u64, MemoryStoreError> {
    total
        .checked_add(1)
        .ok_or_else(|| MemoryStoreError::InvalidRequest {
            message: "tag suggestion count overflowed".to_owned(),
        })
}
