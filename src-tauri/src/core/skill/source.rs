//! 技能来源注册表（ADR-0008）：记录「哪个技能来自哪个 URL 的哪个 commit」
//!
//! 落盘于 `%APPDATA%\dsh-launcher\skill-sources.json`。
//!
//! **命名约束**：文件名**不得**取 `skills.json` —— 该名字已被退役的 ADR-0005 共享模块
//! 占用（存 `preferred_mode` / `last_applied`）。两者语义无关，混用会造成读写串扰。
//!
//! **职责边界**：本模块只**记录事实**，不驱动任何自动更新（用记明确不要自动更新）。
//! 「检查更新」是纯手动动作，且**永不自动写盘**（见 `update.rs`）。
//! 文件缺失或损坏时返回空注册表，绝不阻塞技能管理主功能 —— 磁盘上的技能文件才是事实源。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 注册表 schema 版本
pub const SCHEMA_VERSION: u32 = 1;

/// 一个已导入技能所归属的来源仓库
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    /// 仓库标签（用户在批量导入时填写的友好名，可为空则回退到 URL）
    #[serde(default)]
    pub name: Option<String>,
    /// 来源仓库 URL（用户输入的原样保留）
    pub url: String,
    /// 导入时解析到的 commit sha（用于判断是否有更新）
    #[serde(default)]
    pub commit: Option<String>,
    /// 导入时间（Unix 秒字符串，沿用插件注册表的做法，避免引入时间库）
    pub imported_at: String,
    /// 最近一次「检查更新」时间（Unix 秒字符串）
    #[serde(default)]
    pub checked_at: Option<String>,
    /// 最近一次检查发现的更新数量（仅记录，不驱动任何自动行为）
    #[serde(default)]
    pub last_update_count: usize,
    /// 本次来源导入的技能名列表
    #[serde(default)]
    pub skills: Vec<String>,
}

impl SourceRecord {
    /// 展示名：优先仓库标签，空则回退到 URL
    pub fn display_name(&self) -> &str {
        self.name.as_deref().filter(|n| !n.trim().is_empty()).unwrap_or(&self.url)
    }
}

/// 来源注册表
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRegistry {
    pub schema_version: u32,
    #[serde(default)]
    pub sources: Vec<SourceRecord>,
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            sources: Vec::new(),
        }
    }
}

impl SourceRegistry {
    /// 注册表文件路径（与 config.json 同目录）
    pub fn path() -> PathBuf {
        crate::core::config::AppConfig::config_path()
            .parent()
            .map(|dir| dir.join("skill-sources.json"))
            .unwrap_or_else(|| PathBuf::from("skill-sources.json"))
    }

    /// 读取注册表；不存在或损坏时返回空注册表（不阻塞功能）
    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    /// 从指定路径读取（可单测）
    pub fn load_from(path: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<SourceRegistry>(&raw) {
            Ok(mut registry) => {
                if registry.schema_version == 0 {
                    registry.schema_version = SCHEMA_VERSION;
                }
                registry
            }
            // 损坏 → 空注册表（磁盘上的技能文件仍是事实源，注册表只是元数据）
            Err(_) => Self::default(),
        }
    }

    /// 原子写（临时文件 + 删旧 + rename，与 config.json / plugins.json 同模式）
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }

    /// 写入指定路径（可单测）
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("替换 {} 失败: {e}", path.display())
        })
    }

    /// 按 URL 查找
    pub fn find(&self, url: &str) -> Option<&SourceRecord> {
        self.sources.iter().find(|s| s.url == url)
    }

    /// 插入或更新（按 URL 唯一）
    pub fn upsert(&mut self, record: SourceRecord) {
        match self.sources.iter_mut().find(|s| s.url == record.url) {
            Some(existing) => *existing = record,
            None => self.sources.push(record),
        }
    }

    /// 查某个技能名来自哪个来源（供 UI 显示来源徽章）
    pub fn source_of_skill(&self, skill_name: &str) -> Option<&SourceRecord> {
        self.sources
            .iter()
            .find(|s| s.skills.iter().any(|n| n == skill_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(url: &str, skills: &[&str]) -> SourceRecord {
        SourceRecord {
            name: None,
            url: url.to_string(),
            commit: Some("abc123".to_string()),
            imported_at: "1700000000".to_string(),
            checked_at: None,
            last_update_count: 0,
            skills: skills.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn 缺文件返回空注册表() {
        let dir = std::env::temp_dir().join("dsh-src-test-missing");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("skill-sources.json");
        let reg = SourceRegistry::load_from(&path);
        assert!(reg.sources.is_empty());
        assert_eq!(reg.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn 损坏_json_返回空注册表而不报错() {
        let dir = std::env::temp_dir().join("dsh-src-test-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("skill-sources.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let reg = SourceRegistry::load_from(&path);
        assert!(reg.sources.is_empty(), "损坏文件应降级为空注册表");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 存取往返() {
        let dir = std::env::temp_dir().join("dsh-src-test-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("skill-sources.json");
        let mut reg = SourceRegistry::default();
        reg.upsert(record("https://github.com/mattpocock/skills", &["grill-me"]));
        reg.save_to(&path).unwrap();
        let loaded = SourceRegistry::load_from(&path);
        assert_eq!(loaded.sources.len(), 1);
        assert_eq!(loaded.sources[0].url, "https://github.com/mattpocock/skills");
        assert_eq!(loaded.sources[0].skills, vec!["grill-me"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_按_url_覆盖而非追加() {
        let mut reg = SourceRegistry::default();
        reg.upsert(record("https://x/y", &["a"]));
        reg.upsert(record("https://x/y", &["a", "b"]));
        assert_eq!(reg.sources.len(), 1, "同 URL 应覆盖");
        assert_eq!(reg.sources[0].skills.len(), 2);
    }

    #[test]
    fn 按技能名反查来源() {
        let mut reg = SourceRegistry::default();
        reg.upsert(record("https://x/y", &["alpha", "beta"]));
        assert_eq!(
            reg.source_of_skill("beta").map(|s| s.url.as_str()),
            Some("https://x/y")
        );
        assert!(reg.source_of_skill("gamma").is_none());
    }

    #[test]
    fn schema_version_为_0_时被提升() {
        let dir = std::env::temp_dir().join("dsh-src-test-schema");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("skill-sources.json");
        std::fs::write(&path, r#"{"schemaVersion":0,"sources":[]}"#).unwrap();
        let reg = SourceRegistry::load_from(&path);
        assert_eq!(reg.schema_version, SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
