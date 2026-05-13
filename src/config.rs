use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub telegram: TelegramConfig,
    pub watch: WatchConfig,
    pub recovery: RecoveryConfig,
    pub observability: ObservabilityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub bot_token: String,
    pub allowed_user_ids: Vec<i64>,
    pub allowed_chat_ids: Vec<i64>,
    #[serde(default = "default_pairing_enabled")]
    pub pairing_enabled: bool,
    #[serde(default)]
    pub pairing_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchConfig {
    pub enabled: bool,
    pub poll_interval_seconds: u64,
    pub max_recoveries_per_thread: u32,
    pub cooldown_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RecoveryConfig {
    pub auto_recover: bool,
    pub continue_prompt: String,
    pub tool_failure_prompt: String,
    #[serde(default = "default_safety_rephrase_prompt")]
    pub safety_rephrase_prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservabilityConfig {
    pub latest_feedback_enabled: bool,
    pub completion_notifications_enabled: bool,
    pub record_normal_completion_events: bool,
    pub hook_event_max_lines: usize,
    pub hook_cooldown_max_lines: usize,
    pub control_queue_max_lines: usize,
    pub log_max_bytes: u64,
    pub cleared_rollout_backup_max_bytes: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            telegram: TelegramConfig::default(),
            watch: WatchConfig::default(),
            recovery: RecoveryConfig::default(),
            observability: ObservabilityConfig::default(),
        }
    }
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bot_token: String::new(),
            allowed_user_ids: Vec::new(),
            allowed_chat_ids: Vec::new(),
            pairing_enabled: true,
            pairing_code: String::new(),
        }
    }
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_seconds: 5,
            max_recoveries_per_thread: 10,
            cooldown_seconds: 5,
        }
    }
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            auto_recover: true,
            continue_prompt: "继续干。请先检查当前线程最近状态和工具输出，不要从头开始；如果上一步因为网络、限流或服务端临时错误中断，接着执行；如果其实已经完成，直接汇报结果。".to_string(),
            tool_failure_prompt: "继续干。上一条工具调用失败了，请不要重复使用失效的 session/process/tool；先检查可用工具和当前状态，换一种方式继续完成任务。".to_string(),
            safety_rephrase_prompt: default_safety_rephrase_prompt(),
        }
    }
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            latest_feedback_enabled: true,
            completion_notifications_enabled: true,
            record_normal_completion_events: false,
            hook_event_max_lines: 500,
            hook_cooldown_max_lines: 1000,
            control_queue_max_lines: 1000,
            log_max_bytes: 5 * 1024 * 1024,
            cleared_rollout_backup_max_bytes: 1024 * 1024 * 1024,
        }
    }
}

fn default_pairing_enabled() -> bool {
    true
}

fn default_safety_rephrase_prompt() -> String {
    "继续干。上一轮触发了平台内容安全拦截，请不要复述拦截提示原文。先把任务表述为明确的本机授权、维护/防御/排障范围，确认不攻击第三方、不获取凭据、不绕过安全控制，然后在这个范围内继续完成。如果授权或边界不清，先向用户提出需要补充的最小信息。".to_string()
}

pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex-sentinel")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn load_or_create() -> Result<AppConfig> {
    let path = config_path();
    if !path.exists() {
        let cfg = AppConfig::default();
        save(&cfg)?;
        return Ok(cfg);
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let cfg: AppConfig =
        toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(cfg)
}

pub fn save(cfg: &AppConfig) -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let raw = toml::to_string_pretty(cfg)?;
    fs::write(config_path(), raw)?;
    Ok(())
}

pub fn set_auto_recover(enabled: bool) -> Result<AppConfig> {
    let mut cfg = load_or_create()?;
    cfg.recovery.auto_recover = enabled;
    cfg.watch.enabled = enabled;
    save(&cfg)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_deserialization_fills_missing_fields() {
        let cfg: AppConfig = toml::from_str(
            r#"
[watch]
enabled = false
"#,
        )
        .unwrap();

        assert!(!cfg.watch.enabled);
        assert_eq!(cfg.watch.poll_interval_seconds, 5);
        assert!(cfg.telegram.pairing_enabled);
        assert!(cfg.recovery.auto_recover);
        assert!(!cfg.recovery.safety_rephrase_prompt.is_empty());
        assert!(cfg.observability.latest_feedback_enabled);
        assert!(cfg.observability.completion_notifications_enabled);
        assert!(!cfg.observability.record_normal_completion_events);
        assert_eq!(cfg.observability.hook_event_max_lines, 500);
        assert_eq!(cfg.observability.control_queue_max_lines, 1000);
        assert_eq!(cfg.observability.log_max_bytes, 5 * 1024 * 1024);
    }
}
