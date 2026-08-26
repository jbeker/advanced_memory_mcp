// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Jeremy Beker

//! tokens.json loading and lookup. Port of `token_config.py`.
//!
//! Security note: unlike the Python version's dict lookup, `get` scans all
//! entries with a constant-time comparison so token validation does not leak
//! prefix-match timing. Token counts are small; the scan is negligible.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone)]
pub struct TokenEntry {
    pub file: String,
    pub mode: Mode,
}

impl TokenEntry {
    pub fn is_read_only(&self) -> bool {
        self.mode == Mode::ReadOnly
    }
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(default)]
    file: String,
    // Python defaults a missing mode to read-only.
    #[serde(default = "default_mode")]
    mode: String,
}

fn default_mode() -> String {
    "read-only".to_string()
}

#[derive(Debug)]
pub struct TokenConfig {
    entries: Vec<(String, TokenEntry)>,
}

impl TokenConfig {
    /// Load and validate tokens.json. Mirrors token_config.py's startup
    /// errors: missing file, non-object/empty config, empty `file`, bad
    /// `mode`.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Err(format!("Token config file not found: {}", path.display()));
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read token config {}: {e}", path.display()))?;
        let parsed: HashMap<String, RawEntry> = serde_json::from_str(&raw)
            .map_err(|_| "Token config must be a non-empty JSON object".to_string())?;
        if parsed.is_empty() {
            return Err("Token config must be a non-empty JSON object".to_string());
        }

        let mut entries = Vec::with_capacity(parsed.len());
        for (token, entry) in parsed {
            let mode = match entry.mode.as_str() {
                "read-only" => Mode::ReadOnly,
                "read-write" => Mode::ReadWrite,
                other => {
                    return Err(format!(
                        "Invalid mode '{other}': must be 'read-only' or 'read-write'"
                    ))
                }
            };
            if entry.file.is_empty() {
                return Err("Token entry 'file' must not be empty".to_string());
            }
            entries.push((token, TokenEntry { file: entry.file, mode }));
        }
        Ok(TokenConfig { entries })
    }

    /// Look up a token with constant-time comparison. The error message
    /// matches the Python implementation.
    pub fn get(&self, token: &str) -> Result<&TokenEntry, String> {
        let mut found: Option<&TokenEntry> = None;
        for (candidate, entry) in &self.entries {
            if candidate.as_bytes().ct_eq(token.as_bytes()).into() {
                found = Some(entry);
            }
        }
        found.ok_or_else(|| "Unknown or invalid authentication token.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn loads_and_looks_up_tokens() {
        let (_d, path) = write_config(
            r#"{"alice_aaa": {"file": "a.jsonl", "mode": "read-write"},
                "bob_bbb": {"file": "a.jsonl", "mode": "read-only"},
                "carol_ccc": {"file": "c.jsonl"}}"#,
        );
        let cfg = TokenConfig::load(&path).unwrap();
        assert_eq!(cfg.get("alice_aaa").unwrap().mode, Mode::ReadWrite);
        assert!(cfg.get("bob_bbb").unwrap().is_read_only());
        // Missing mode defaults to read-only, like Python.
        assert!(cfg.get("carol_ccc").unwrap().is_read_only());
        assert_eq!(
            cfg.get("nope").unwrap_err(),
            "Unknown or invalid authentication token."
        );
    }

    #[test]
    fn startup_validation_errors() {
        let (_d, missing) = write_config("{}");
        assert!(TokenConfig::load(&missing).unwrap_err().contains("non-empty"));

        let (_d2, bad_mode) =
            write_config(r#"{"t": {"file": "f.jsonl", "mode": "admin"}}"#);
        assert!(TokenConfig::load(&bad_mode).unwrap_err().contains("Invalid mode 'admin'"));

        let (_d3, empty_file) = write_config(r#"{"t": {"mode": "read-only"}}"#);
        assert!(TokenConfig::load(&empty_file)
            .unwrap_err()
            .contains("'file' must not be empty"));

        let err = TokenConfig::load(Path::new("/nonexistent/tokens.json")).unwrap_err();
        assert!(err.contains("not found"));
    }
}
