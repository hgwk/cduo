use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn claude_settings_candidates(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = vec![cwd.join(".claude").join("settings.local.json")];
    if let Some(home) = std::env::var_os("HOME") {
        let home_claude = PathBuf::from(home).join(".claude");
        paths.push(home_claude.join("settings.json"));
        paths.push(home_claude.join("settings.local.json"));
    }
    paths
}

pub(super) fn claude_startup_hooks_report(paths: &[PathBuf]) -> String {
    let mut found = Vec::new();
    let mut invalid = Vec::new();
    for path in paths {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
            invalid.push(display_path(path));
            continue;
        };
        let count = count_hook_commands(&settings, "SessionStart");
        if count > 0 {
            found.push(format!("{} ({count})", display_path(path)));
        }
    }

    let count = found
        .iter()
        .filter_map(|entry| {
            entry
                .rsplit_once('(')
                .and_then(|(_, count)| count.trim_end_matches(')').parse::<usize>().ok())
        })
        .sum::<usize>();
    let hooks = if found.is_empty() {
        "! Claude startup hooks: none found in checked settings".to_string()
    } else {
        format!(
            "! Claude startup hooks: found {} command(s) in {}",
            count,
            found.join(", ")
        )
    };
    let hooks = if count > 1 {
        format!(
            "{hooks}\n  hint: multiple Claude SessionStart hooks can run duplicate startup work; review the listed settings file(s) and keep only intentional hooks. cduo relay uses the project Stop hook installed by `cduo init`."
        )
    } else {
        hooks
    };
    if invalid.is_empty() {
        hooks
    } else {
        format!("{hooks}; invalid JSON in {}", invalid.join(", "))
    }
}

pub(super) fn claude_stop_pair_id_report(paths: &[PathBuf]) -> String {
    let mut stop_count = 0;
    let mut pair_aware = Vec::new();
    let mut invalid = Vec::new();
    for path in paths {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
            invalid.push(display_path(path));
            continue;
        };
        let count = count_hook_commands(&settings, "Stop");
        stop_count += count;
        let aware = count_stop_pair_aware_commands(&settings);
        if aware > 0 {
            pair_aware.push(format!("{} ({aware}/{count})", display_path(path)));
        }
    }

    let report = if stop_count == 0 {
        "! Claude Stop hook pair id: no Stop hook found; run `cduo init`".to_string()
    } else if pair_aware.is_empty() {
        "! Claude Stop hook pair id: missing CDUO_PAIR_ID in Stop hook; run `cduo init`".to_string()
    } else {
        format!(
            "✓ Claude Stop hook pair id: found pair-aware hook in {}",
            pair_aware.join(", ")
        )
    };
    if invalid.is_empty() {
        report
    } else {
        format!("{report}; invalid JSON in {}", invalid.join(", "))
    }
}

pub(super) fn count_hook_commands(settings: &serde_json::Value, hook_name: &str) -> usize {
    hook_commands(settings, hook_name).count()
}

fn count_stop_pair_aware_commands(settings: &serde_json::Value) -> usize {
    hook_commands(settings, "Stop")
        .filter(|command| command.contains("CDUO_PAIR_ID"))
        .count()
}

fn hook_commands<'a>(
    settings: &'a serde_json::Value,
    hook_name: &str,
) -> impl Iterator<Item = &'a str> {
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get(hook_name))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("hooks").and_then(serde_json::Value::as_array))
        .flatten()
        .filter_map(|hook| {
            (hook.get("type").and_then(serde_json::Value::as_str) == Some("command"))
                .then(|| hook.get("command").and_then(serde_json::Value::as_str))
                .flatten()
        })
        .filter(|command| !command.trim().is_empty())
}

pub(super) fn display_path(path: &Path) -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return path.display().to_string();
    };
    path.strip_prefix(&cwd)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}
