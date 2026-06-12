use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::project::{
    orchestration_ref, LEGACY_ORCHESTRATION_REF, ORCHESTRATION_END, ORCHESTRATION_START,
};

pub(crate) fn ensure_instruction_reference(path: &Path, force: bool) -> Result<bool> {
    let orchestration_ref = orchestration_ref()?;
    if !path.exists() {
        fs::write(path, format!("{orchestration_ref}\n"))?;
        return Ok(true);
    }

    let existing = fs::read_to_string(path)?;
    if !force && has_instruction_reference(&existing) && !has_legacy_orchestration(&existing) {
        return Ok(false);
    }

    let (without_ref, _) = remove_reference_prelude(&existing);
    let (without_legacy, _) = remove_orchestration_block(&without_ref);
    let body = without_legacy.trim();
    let updated = if body.is_empty() {
        format!("{orchestration_ref}\n")
    } else {
        format!("{orchestration_ref}\n\n---\n\n{body}\n")
    };

    if updated == existing {
        return Ok(false);
    }

    fs::write(path, updated)?;
    Ok(true)
}

pub(crate) fn has_instruction_reference(content: &str) -> bool {
    reference_prelude_position(content).is_some()
}

pub(crate) fn has_legacy_orchestration(content: &str) -> bool {
    content.contains(ORCHESTRATION_START) && content.contains(ORCHESTRATION_END)
}

pub(crate) fn remove_orchestration_block(content: &str) -> (String, bool) {
    let Some(start) = content.find(ORCHESTRATION_START) else {
        return (content.to_string(), false);
    };
    let Some(end) = content[start..]
        .find(ORCHESTRATION_END)
        .map(|offset| start + offset + ORCHESTRATION_END.len())
    else {
        return (content.to_string(), false);
    };
    let before = &content[..start];
    let mut after = content[end..].to_string();
    if before.trim().is_empty() {
        after = strip_leading_cduo_separator(&after);
    }
    (format!("{before}{after}").trim().to_string(), true)
}

pub(crate) fn remove_reference_prelude(content: &str) -> (String, bool) {
    let lines = content.lines().collect::<Vec<_>>();
    let Some(pos) = reference_prelude_position(content) else {
        return (content.to_string(), false);
    };

    let mut remaining = lines
        .into_iter()
        .enumerate()
        .filter_map(|(idx, line)| (idx != pos).then_some(line))
        .collect::<Vec<_>>()
        .join("\n");
    remaining = strip_leading_cduo_separator(&remaining);
    (remaining.trim().to_string(), true)
}

pub(crate) fn reference_prelude_position(content: &str) -> Option<usize> {
    let refs = known_orchestration_refs();
    let lines = content.lines().collect::<Vec<_>>();
    lines
        .iter()
        .position(|line| refs.iter().any(|reference| line.trim() == reference))
        .filter(|pos| lines[..*pos].iter().all(|line| line.trim().is_empty()))
}

pub(crate) fn known_orchestration_refs() -> Vec<String> {
    let mut refs = vec![LEGACY_ORCHESTRATION_REF.to_string()];
    if let Ok(reference) = orchestration_ref() {
        refs.insert(0, reference);
    }
    refs
}

pub(crate) fn strip_leading_cduo_separator(content: &str) -> String {
    let trimmed = content.trim_start();
    if trimmed == "---" {
        return String::new();
    }
    if let Some(rest) = trimmed.strip_prefix("---\n") {
        return rest.trim_start().to_string();
    }
    trimmed.to_string()
}

pub(crate) fn instruction_removal_target_exists(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)?;
    Ok(has_instruction_reference(&content) || has_legacy_orchestration(&content))
}

pub(crate) fn remove_instruction_reference(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(path)?;
    let (without_ref, ref_removed) = remove_reference_prelude(&content);
    let (without_block, legacy_removed) = remove_orchestration_block(&without_ref);
    let cleaned = without_block.trim().to_string();

    if !ref_removed && !legacy_removed {
        return Ok(false);
    }

    if cleaned.is_empty() {
        fs::remove_file(path)?;
    } else {
        fs::write(path, format!("{cleaned}\n"))?;
    }
    Ok(true)
}
