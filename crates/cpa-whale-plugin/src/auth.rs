use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct HostAuthListResponse {
    #[serde(default)]
    pub files: Vec<HostAuthEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HostAuthEntry {
    #[serde(default)]
    pub auth_index: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default, rename = "type")]
    pub auth_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub unavailable: bool,
}

#[derive(Debug, Clone)]
pub struct SanitizedAccount {
    pub auth_index: String,
    pub provider: String,
    pub status: String,
    pub unavailable: bool,
}

pub fn sanitize_accounts(entries: Vec<HostAuthEntry>) -> Vec<SanitizedAccount> {
    let mut out = entries
        .into_iter()
        .filter_map(|entry| {
            let auth_index = entry.auth_index.trim().to_string();
            if auth_index.is_empty() {
                return None;
            }
            let provider = if entry.provider.trim().is_empty() {
                entry.auth_type.trim().to_string()
            } else {
                entry.provider.trim().to_string()
            };
            Some(SanitizedAccount {
                auth_index,
                provider: if provider.is_empty() {
                    "unknown".into()
                } else {
                    provider
                },
                status: if entry.disabled {
                    "disabled".into()
                } else if entry.status.trim().is_empty() {
                    "unknown".into()
                } else {
                    entry.status.trim().to_string()
                },
                unavailable: entry.unavailable || entry.disabled,
            })
        })
        .collect::<Vec<_>>();
    out.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then(left.auth_index.cmp(&right.auth_index))
    });
    out
}

pub fn deterministic_labels(accounts: &[SanitizedAccount]) -> HashMap<String, String> {
    let mut counters = HashMap::<String, usize>::new();
    let mut labels = HashMap::new();
    for account in accounts {
        let provider = title_case(&account.provider);
        let counter = counters.entry(provider.clone()).or_default();
        *counter += 1;
        labels.insert(
            account.auth_index.clone(),
            format!("{provider} {}", letter(*counter)),
        );
    }
    labels
}

fn letter(index: usize) -> String {
    if index == 0 {
        return "?".into();
    }
    let mut value = index;
    let mut out = String::new();
    while value > 0 {
        value -= 1;
        out.insert(0, (b'A' + (value % 26) as u8) as char);
        value /= 26;
    }
    out
}

fn title_case(value: &str) -> String {
    let value = value.trim();
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_accounts_without_exposing_identity() {
        let accounts = sanitize_accounts(vec![
            HostAuthEntry {
                auth_index: "b".into(),
                provider: "codex".into(),
                auth_type: String::new(),
                status: "active".into(),
                disabled: false,
                unavailable: false,
            },
            HostAuthEntry {
                auth_index: "a".into(),
                provider: "codex".into(),
                auth_type: String::new(),
                status: "active".into(),
                disabled: false,
                unavailable: false,
            },
        ]);
        let labels = deterministic_labels(&accounts);
        assert_eq!(labels.get("a").map(String::as_str), Some("Codex A"));
        assert_eq!(labels.get("b").map(String::as_str), Some("Codex B"));
    }
}
