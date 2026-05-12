use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryKind {
    None,
    RetryLater,
    RetrySoon,
    ManualOnly,
    Reauth,
    SwitchModel,
    ToolRetryWithDifferentPath,
    SafetyRephrase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryDecision {
    pub kind: RecoveryKind,
    pub auto_allowed: bool,
    pub delay_seconds: u64,
    pub label: String,
    pub reason: String,
}

impl RecoveryDecision {
    pub fn none() -> Self {
        Self {
            kind: RecoveryKind::None,
            auto_allowed: false,
            delay_seconds: 0,
            label: "No recovery needed".to_string(),
            reason: "No actionable failure is visible.".to_string(),
        }
    }
}

pub fn classify_error(text: &str) -> RecoveryDecision {
    let lower = text.to_ascii_lowercase();

    if lower.contains("plugin/list featured plugin fetch failed") {
        return RecoveryDecision {
            kind: RecoveryKind::None,
            auto_allowed: false,
            delay_seconds: 0,
            label: "Plugin catalog noise".to_string(),
            reason:
                "Codex falls back to an empty featured plugin list; this is not a turn failure."
                    .to_string(),
        };
    }

    if lower.contains("retrying sampling request") {
        return RecoveryDecision {
            kind: RecoveryKind::None,
            auto_allowed: false,
            delay_seconds: 0,
            label: "Retry still in progress".to_string(),
            reason: "Codex is still inside its own retry loop; wait for the final turn result before recovering.".to_string(),
        };
    }

    if lower.contains("silent turn completion")
        || lower.contains("completed without a final assistant message")
    {
        return RecoveryDecision {
            kind: RecoveryKind::RetrySoon,
            auto_allowed: true,
            delay_seconds: 0,
            label: "Silent turn completion".to_string(),
            reason: "Codex reported the turn as complete but did not produce a final assistant message; continue once to finish or report the actual state.".to_string(),
        };
    }

    if lower.contains("codex app-server reported terminal thread status")
        || lower.contains("codex app-server reported terminal turn status")
    {
        return RecoveryDecision {
            kind: RecoveryKind::RetrySoon,
            auto_allowed: true,
            delay_seconds: 0,
            label: "Codex terminal status".to_string(),
            reason: "Codex app-server 已确认线程或 turn 处于终止错误状态；可交给可见窗口恢复。"
                .to_string(),
        };
    }

    if lower.contains("this content was flagged for possible cybersecurity risk")
        || (lower.contains("possible cybersecurity risk") && lower.contains("try rephrasing"))
        || lower.contains("trusted access for cyber")
    {
        return RecoveryDecision {
            kind: RecoveryKind::SafetyRephrase,
            auto_allowed: true,
            delay_seconds: 0,
            label: "内容安全改写".to_string(),
            reason: "上一轮请求表述触发平台安全规则；需要明确本机授权、维护/防御/排障范围后继续。"
                .to_string(),
        };
    }

    if lower.contains("insufficient_balance") || lower.contains("insufficient account balance") {
        return RecoveryDecision {
            kind: RecoveryKind::ManualOnly,
            auto_allowed: true,
            delay_seconds: 5,
            label: "Insufficient balance".to_string(),
            reason: "Provider balance or route is unhealthy; auto-continue is allowed a few times so Codex can switch route or report the blocker.".to_string(),
        };
    }

    if lower.contains("invalid_token")
        || lower.contains("missing or invalid access token")
        || lower.contains("authrequired")
    {
        return RecoveryDecision {
            kind: RecoveryKind::Reauth,
            auto_allowed: false,
            delay_seconds: 0,
            label: "MCP auth expired".to_string(),
            reason: "An MCP server needs OAuth refresh before related tool calls can work."
                .to_string(),
        };
    }

    if lower.contains("401 unauthorized")
        || lower.contains("unauthorized")
        || lower.contains("401:")
    {
        return RecoveryDecision {
            kind: RecoveryKind::Reauth,
            auto_allowed: false,
            delay_seconds: 0,
            label: "Unauthorized".to_string(),
            reason:
                "The request needs a fresh login, token, or provider credential before retrying."
                    .to_string(),
        };
    }

    if lower.contains("403 forbidden") || lower.contains("forbidden") || lower.contains("403:") {
        return RecoveryDecision {
            kind: RecoveryKind::ManualOnly,
            auto_allowed: true,
            delay_seconds: 5,
            label: "Forbidden".to_string(),
            reason:
                "The provider or tool rejected access. Auto-continue is allowed a few times so Codex can switch route, retry safely, or surface the blocker."
                    .to_string(),
        };
    }

    if lower.contains("selected model is at capacity") {
        return RecoveryDecision {
            kind: RecoveryKind::SwitchModel,
            auto_allowed: true,
            delay_seconds: 5,
            label: "Model at capacity".to_string(),
            reason: "The selected model is saturated. Auto-continue can ask Codex to retry or pick an available route."
                .to_string(),
        };
    }

    if lower.contains("429 too many requests") || lower.contains("exceeded retry limit") {
        return RecoveryDecision {
            kind: RecoveryKind::RetryLater,
            auto_allowed: true,
            delay_seconds: 5,
            label: "Rate limited".to_string(),
            reason: "Codex already exhausted its internal retry loop. Retry with a short backoff."
                .to_string(),
        };
    }

    if lower.contains("所有供应商已熔断") || lower.contains("all providers") {
        return RecoveryDecision {
            kind: RecoveryKind::RetryLater,
            auto_allowed: true,
            delay_seconds: 5,
            label: "All providers unavailable".to_string(),
            reason: "The upstream provider pool is circuit-open. Retry briefly and let Codex continue or surface the blocker."
                .to_string(),
        };
    }

    if lower.contains("503 service unavailable")
        || lower.contains("service temporarily unavailable")
        || lower.contains("502 bad gateway")
        || lower.contains("500 internal")
    {
        return RecoveryDecision {
            kind: RecoveryKind::RetrySoon,
            auto_allowed: true,
            delay_seconds: 3,
            label: "Temporary upstream failure".to_string(),
            reason: "The failure looks transient and is safe to retry after a short backoff."
                .to_string(),
        };
    }

    if lower.contains("stream disconnected")
        || lower.contains("response_stream_disconnected")
        || lower.contains("error decoding response body")
    {
        return RecoveryDecision {
            kind: RecoveryKind::RetrySoon,
            auto_allowed: true,
            delay_seconds: 3,
            label: "流式传输中断".to_string(),
            reason: "Codex 流式响应或网络传输中断；确认线程停止推进后可自动继续。".to_string(),
        };
    }

    if lower.contains("write_stdin failed")
        || lower.contains("unknown process id")
        || lower.contains("stdin is closed")
        || lower.contains("unknown mcp server")
        || lower.contains("unsupported call:")
        || lower.contains("app-server timed out after")
        || lower.contains("background turn supervisor failed")
    {
        return RecoveryDecision {
            kind: RecoveryKind::ToolRetryWithDifferentPath,
            auto_allowed: true,
            delay_seconds: 0,
            label: "Tool path failed".to_string(),
            reason: "The task can often continue, but the next prompt should tell Codex not to reuse the failed process/tool.".to_string(),
        };
    }

    RecoveryDecision::none()
}

pub fn sanitized_recovery_text(text: &str) -> String {
    [
        (
            "This content was flagged for possible cybersecurity risk",
            "上一轮触发了平台内容安全拦截",
        ),
        ("possible cybersecurity risk", "平台内容安全规则"),
        ("try rephrasing your request", "改写请求"),
        ("Trusted Access for Cyber", "官方授权计划提示"),
        ("https://chatgpt.com/cyber", "官方说明链接"),
    ]
    .into_iter()
    .fold(text.to_string(), |acc, (needle, replacement)| {
        replace_ascii_case_insensitive(&acc, needle, replacement)
    })
}

fn replace_ascii_case_insensitive(text: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return text.to_string();
    }

    let lower_text = text.to_ascii_lowercase();
    let lower_needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;

    while let Some(relative) = lower_text[cursor..].find(&lower_needle) {
        let start = cursor + relative;
        let end = start + needle.len();
        out.push_str(&text[cursor..start]);
        out.push_str(replacement);
        cursor = end;
    }

    out.push_str(&text[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_user_history_errors() {
        assert_eq!(
            classify_error("Turn error: exceeded retry limit, last status: 429 Too Many Requests")
                .kind,
            RecoveryKind::RetryLater
        );
        assert_eq!(
            classify_error(
                "Turn error: unexpected status 503 Service Unavailable: 所有供应商已熔断，无可用渠道"
            )
            .delay_seconds,
            5
        );
        assert!(classify_error(
            "403 Forbidden: {\"code\":\"INSUFFICIENT_BALANCE\",\"message\":\"Insufficient account balance\"}"
        )
        .auto_allowed);
        assert_eq!(
            classify_error("Turn error: unexpected status 401 Unauthorized").kind,
            RecoveryKind::Reauth
        );
        assert_eq!(
            classify_error("Turn error: unexpected status 403 Forbidden").kind,
            RecoveryKind::ManualOnly
        );
        assert_eq!(
            classify_error("Selected model is at capacity. Please try a different model.").kind,
            RecoveryKind::SwitchModel
        );
        assert_eq!(
            classify_error("plugin/list featured plugin fetch failed: 403 Forbidden").kind,
            RecoveryKind::None
        );
        let disconnected = classify_error(
            "stream disconnected before completion: Transport error: network error: error decoding response body",
        );
        assert_eq!(disconnected.kind, RecoveryKind::RetrySoon);
        assert!(disconnected.auto_allowed);
        assert_eq!(disconnected.delay_seconds, 3);
        let disconnected_with_request_id = classify_error(
            "stream disconnected before completion: An error occurred while processing your request. You can retry your request, or contact us through our help center at help.openai.com if the error persists. Please include the request ID 00000000-0000-4000-8000-000000000000 in your message.",
        );
        assert_eq!(disconnected_with_request_id.kind, RecoveryKind::RetrySoon);
        assert!(disconnected_with_request_id.auto_allowed);
        assert_eq!(disconnected_with_request_id.delay_seconds, 3);
        assert_eq!(
            classify_error("stream disconnected - retrying sampling request (1/5 in 202ms)").kind,
            RecoveryKind::None
        );
        let silent = classify_error(
            "Silent turn completion: thread thread-a turn turn-a completed without a final assistant message",
        );
        assert_eq!(silent.kind, RecoveryKind::RetrySoon);
        assert!(silent.auto_allowed);
        assert_eq!(silent.delay_seconds, 0);
        assert_eq!(
            classify_error("background turn supervisor failed for thread-a/turn-a: app-server timed out after 1800s").kind,
            RecoveryKind::ToolRetryWithDifferentPath
        );
        let flagged = classify_error(
            "This content was flagged for possible cybersecurity risk. If this seems wrong, try rephrasing your request. To get authorized for security work, join the Trusted Access for Cyber program.",
        );
        assert_eq!(flagged.kind, RecoveryKind::SafetyRephrase);
        assert!(flagged.auto_allowed);
        assert!(!flagged.label.contains("Cyber"));
        assert!(
            !flagged
                .reason
                .to_ascii_lowercase()
                .contains("cybersecurity")
        );
    }

    #[test]
    fn sanitizes_safety_rephrase_prompt_text() {
        let text = "This content was flagged for possible cybersecurity risk. If this seems wrong, try rephrasing your request. Trusted Access for Cyber: https://chatgpt.com/cyber";
        let sanitized = sanitized_recovery_text(text);
        let lower = sanitized.to_ascii_lowercase();

        assert!(!lower.contains("possible cybersecurity risk"));
        assert!(!lower.contains("try rephrasing"));
        assert!(!lower.contains("trusted access for cyber"));
        assert!(!lower.contains("chatgpt.com/cyber"));
        assert!(sanitized.contains("平台内容安全拦截"));
    }
}
