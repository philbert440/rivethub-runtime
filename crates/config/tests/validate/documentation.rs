use super::{BTreeMap, DeviationClass, Failed, GOLDEN_FILES, fs, golden_path, load_deviations};

pub(super) fn deviations_are_documented() -> Result<(), Failed> {
    let text = fs::read_to_string(golden_path("DEVIATIONS.md"))
        .map_err(|error| format!("DEVIATIONS.md: {error}"))?;
    for marker in [
        "Decision 15",
        "Decision 13",
        "Decision 12/20",
        "Decision 11",
        "Decision 9",
        "Decision 2",
        "Decision 16",
        "Decision 10",
    ] {
        if !text.contains(marker) {
            return Err(format!("DEVIATIONS.md must document {marker}").into());
        }
    }
    for file in GOLDEN_FILES {
        if !text.contains(&format!("{file}.jsonl")) {
            return Err(format!("DEVIATIONS.md must name {file}.jsonl").into());
        }
    }
    let rows = load_deviations().map_err(Failed::from)?;
    let mut counts: BTreeMap<(String, DeviationClass), usize> = BTreeMap::new();
    for row in &rows {
        let Some((file, _)) = row.id.split_once(':') else {
            return Err(format!("deviations row without a line number: {}", row.id).into());
        };
        *counts.entry((file.to_owned(), row.class)).or_default() += 1;
    }
    for ((file, class), count) in counts {
        let bullet = format!("- `{file}` — {count} {class}");
        if !text.contains(&bullet) {
            return Err(format!("DEVIATIONS.md must contain: {bullet}").into());
        }
    }
    Ok(())
}
