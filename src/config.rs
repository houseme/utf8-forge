// Copyright 2026 houseme
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! TOML config file support (`--config <PATH>`).
//!
//! Precedence: CLI flags > config values > built-in defaults. Lists replace
//! (never concatenate) across layers so the effective whitelist stays
//! auditable. Unknown keys, malformed TOML, and wrong types are fatal input
//! errors — a typo that silently does nothing is worse than a loud failure.
//! Deliberately absent from the schema: `root`, `report`, `dry_run`, `url`,
//! `output`, `quiet`, `verbose`, `ignore_file` — options that select what
//! gets written where or flip write behavior must stay visible on the
//! command line.

use std::path::Path;

use anyhow::{Context, bail};
use serde::Deserialize;

use crate::transcoder::TargetEncoding;

/// Whole config document. Every section and field is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeConfig {
    /// Whitelist / exclusion overrides (`[filter]`).
    #[serde(default)]
    pub filter: FilterSection,
    /// Conversion options (`[convert]`).
    #[serde(default)]
    pub convert: ConvertSection,
    /// Run options (`[options]`).
    #[serde(default)]
    pub options: OptionsSection,
}

/// `[filter]` section: mirrors the corresponding CLI flags.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterSection {
    /// Whitelist override. `None` = key absent (the default whitelist stays
    /// in effect); `Some(list)` replaces it, and an empty list is a fatal
    /// input error at merge time — silently scanning nothing is worse than a
    /// loud failure.
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_dirs: Vec<String>,
    #[serde(default)]
    pub exclude_ext: Vec<String>,
    #[serde(default)]
    pub exclude_globs: Vec<String>,
    #[serde(default)]
    pub include_rc: bool,
    #[serde(default)]
    pub include_msbuild: bool,
    #[serde(default)]
    pub include_hidden: bool,
}

/// `[convert]` section: conversion options for `convert` and `fix`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConvertSection {
    #[serde(default)]
    pub target: Option<String>,
    /// Backup directory override. An empty string is a fatal input error at
    /// merge time — an unnamed directory can never be a usable target.
    #[serde(default)]
    pub backup_dir: Option<String>,
}

/// `[options]` section: run sizing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionsSection {
    #[serde(default)]
    pub max_file_size: Option<u64>,
    #[serde(default)]
    pub threads: Option<usize>,
}

/// Load and parse the config file at `path`. Fatal on missing / unreadable /
/// malformed input (exit 2) — the caller treats any `Err` as fatal.
pub fn load(path: &Path) -> anyhow::Result<ForgeConfig> {
    if !path.is_file() {
        bail!("config file `{}` not found", path.display());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config file `{}`", path.display()))?;
    let cfg: ForgeConfig = toml::from_str(&text)
        .with_context(|| format!("config file `{}` is not valid", path.display()))?;
    Ok(cfg)
}

impl ForgeConfig {
    /// Parse `[convert].target` with the same grammar as the CLI value.
    /// Invalid values name the config key so the operator can find the line.
    pub fn target(&self) -> anyhow::Result<Option<TargetEncoding>> {
        match &self.convert.target {
            None => Ok(None),
            Some(raw) => TargetEncoding::parse(raw).map(Some).ok_or_else(|| {
                anyhow::anyhow!(
                    "config [convert].target: invalid target encoding `{raw}` (expected utf-8 | utf-8-bom | gb18030 | utf-16le | utf-16be)"
                )
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_document() {
        let cfg: ForgeConfig = toml::from_str(
            r#"
[filter]
extensions = ["c", "cpp"]
exclude_dirs = ["legacy"]
include_rc = true

[convert]
target = "utf-16le"
backup_dir = "/tmp/bk"

[options]
max_file_size = 1024
threads = 4
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.filter.extensions,
            Some(vec!["c".to_string(), "cpp".to_string()])
        );
        assert!(cfg.filter.include_rc);
        assert_eq!(cfg.options.max_file_size, Some(1024));
        assert_eq!(cfg.target().unwrap(), Some(TargetEncoding::Utf16Le));
    }

    #[test]
    fn empty_document_is_valid() {
        let cfg: ForgeConfig = toml::from_str("").unwrap();
        assert!(cfg.filter.extensions.is_none());
        assert_eq!(cfg.target().unwrap(), None);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = toml::from_str::<ForgeConfig>(
            "[filter]
typo_key = 1
",
        )
        .unwrap_err();
        assert!(err.to_string().contains("typo_key"), "{err}");
    }

    #[test]
    fn invalid_target_value_is_rejected_with_key_name() {
        let cfg: ForgeConfig = toml::from_str("[convert]\ntarget = \"gbk\"\n").unwrap();
        let err = cfg.target().unwrap_err().to_string();
        assert!(err.contains("[convert].target"), "{err}");
    }
}
