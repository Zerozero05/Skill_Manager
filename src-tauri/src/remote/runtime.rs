//! Classify immutable GitHub package entries using the same dependency rules as local folders.
use super::github::{validate_relative, Entry, Snapshot, MAX_TOTAL};
use crate::local::{runtime_file_selection_with_reader, RuntimeFileSelection};
use std::collections::{BTreeMap, BTreeSet};

pub fn inspect(
    snap: &Snapshot,
    entries: &BTreeMap<String, Entry>,
    selected: Option<&BTreeSet<String>>,
) -> Result<RuntimeFileSelection, String> {
    let all = entries
        .iter()
        .filter(|(path, entry)| {
            validate_relative(path).is_ok() && matches!(entry.mode.as_str(), "100644" | "100755")
        })
        .map(|(path, entry)| (path.clone(), entry.sha.clone()))
        .collect();
    let mut total = 0usize;
    runtime_file_selection_with_reader(&all, selected, |path| {
        crate::network::check_cancelled()?;
        let entry = entries
            .get(path)
            .ok_or_else(|| format!("Package file is missing: {path}"))?;
        if entry.size > MAX_TOTAL.saturating_sub(total) as u64 {
            return Err("Dependency inspection exceeds the 32 MiB package limit".into());
        }
        let bytes = snap.read_entry(entry)?;
        total = total
            .checked_add(bytes.len())
            .filter(|n| *n <= MAX_TOTAL)
            .ok_or("Dependency inspection exceeds the 32 MiB package limit")?;
        String::from_utf8(bytes).map_err(|_| format!("Dependency file is not UTF-8 text: {path}"))
    })
}
