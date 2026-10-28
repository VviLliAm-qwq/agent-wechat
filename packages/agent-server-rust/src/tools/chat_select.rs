use super::exec::{exec_command, ExecOptions};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenChatResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Open a chat in the WeChat UI using the chat-select tool.
///
/// Args format: chat-select [--force] [--click-xy X Y] <username>
pub async fn open_chat(
    chat_id: &str,
    force: bool,
    click_xy: Option<(f64, f64)>,
) -> OpenChatResult {
    let mut args: Vec<String> = Vec::new();

    if force {
        args.push("--force".into());
    }

    if let Some((x, y)) = click_xy {
        args.push("--click-xy".into());
        args.push((x as i32).to_string());
        args.push((y as i32).to_string());
    }

    // chat_id is a positional arg — must be last
    args.push(chat_id.into());

    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let result = exec_command(
        "chat-select",
        &args_ref,
        &ExecOptions::default(),
    )
    .await;

    // Result JSON is on stdout regardless of exit code
    if let Ok(parsed) = serde_json::from_str::<OpenChatResult>(&result.stdout) {
        return parsed;
    }

    // Fallback: couldn't parse stdout
    OpenChatResult {
        ok: false,
        username: None,
        index: None,
        skipped: None,
        error: Some(if result.stderr.is_empty() {
            format!("chat-select exited with code {}", result.exit_code)
        } else {
            result.stderr
        }),
    }
}

/// 判断目标会话是否已经处于打开状态，用于跳过昂贵的 chat-select 调用。
///
/// 背景：open_chat 内部要跑 chat-select，它需要枚举全部会话（frida 内存扫描），
/// 实测耗时约 1.0~1.7 秒。这个开销在「目标会话本来就开着」时完全是浪费——
/// 实测发送一条消息总共 2.5 秒，其中 1.7 秒花在这里。
///
/// 这里改为直接读 AppState：如果当前已打开的就是目标会话，并且界面处于
/// chat_open（输入框可用），则无需切会话。
///
/// 安全性：名字必须完全匹配，且必须有 chat_open 状态。
/// 任一条件不满足就走原流程，不会误发到别的会话。
pub fn already_open(
    state: &crate::ia::types::AppState,
    identified: &crate::ia::types::IdentifiedStates,
    target_name: &str,
) -> bool {
    let is_chat_open = identified
        .main_window
        .as_ref()
        .map(|m| m.state_id.as_str() == "chat_open")
        .unwrap_or(false);
    if !is_chat_open {
        return false;
    }

    match state.main_window.opened_chat_name.as_deref() {
        Some(name) => name == target_name,
        None => false,
    }
}
