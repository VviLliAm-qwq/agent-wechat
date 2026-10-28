use super::chat_cache::{chat_name_cache_get, chat_name_cache_put};
use super::Plan;
use super::composer::find_edit_and_send_button;
use crate::ia::actions;
use crate::ia::selectors::query_selector;
use crate::ia::types::*;
use crate::tools::chat_select::{already_open, open_chat, OpenChatResult};
use crate::tools::exec::{exec_command, ExecOptions};

pub struct SendMessagePlan;

#[cfg(test)]
mod composer_tests {
    use super::*;
    use serde_json::{json, Value};

    fn editor(y: f64) -> Value {
        json!({"role":"text","name":"","states":["EDITABLE"],
            "bounds":{"x":300.0,"y":y,"width":650.0,"height":80.0}})
    }
    fn send(name: &str) -> Value {
        json!({"role":"push-button","name":name,"states":["DISABLED"],
            "bounds":{"x":890.0,"y":700.0,"width":55.0,"height":24.0}})
    }
    fn group(children: Vec<Value>) -> Value {
        json!({"role":"filler","name":"","children":children})
    }
    fn parse(value: Value) -> A11yNode { serde_json::from_value(value).unwrap() }

    #[test]
    fn legacy_sibling_composer() {
        let tree = parse(group(vec![editor(610.0), send("Send(S)")]));
        assert!(find_edit_and_send_button(&tree).is_some());
    }

    #[test]
    fn nested_new_composer_with_search_field() {
        let composer = group(vec![group(vec![editor(610.0)]),
            group(vec![group(vec![send("Send Voice"), send("Send")])])]);
        let tree = parse(group(vec![editor(45.0), composer]));
        let (edit, button) = find_edit_and_send_button(&tree).unwrap();
        assert_eq!(edit.bounds.as_ref().unwrap().y, 610.0);
        assert_eq!(button.name, "Send");
    }

    #[test]
    fn does_not_pair_search_field_with_send() {
        let tree = parse(group(vec![group(vec![editor(45.0)]), group(vec![send("Send")])]));
        assert!(find_edit_and_send_button(&tree).is_none());
    }

    #[test]
    fn rejects_ambiguous_editors_and_voice_button() {
        let tree = parse(group(vec![group(vec![editor(610.0), editor(610.0)]), group(vec![send("Send")])]));
        assert!(find_edit_and_send_button(&tree).is_none());
        let voice = parse(group(vec![editor(610.0), send("Send Voice")]));
        assert!(find_edit_and_send_button(&voice).is_none());
    }
}

/// Notices WeChat shows in place of the message editor. When one is present the
/// chat cannot accept text, so the send can never succeed.
fn composer_block_notice(a11y: &A11yNode) -> Option<String> {
    const NOTICES: [(&str, &str); 3] = [
        ("exited group chat", "receiver exited the group chat"),
        ("not friends", "receiver is no longer a friend"),
        ("blocked", "receiver has blocked this account"),
    ];
    let haystack = serialize_names(a11y).to_lowercase();
    for (needle, reason) in NOTICES {
        if haystack.contains(needle) {
            return Some(format!("chat cannot receive messages: {reason}"));
        }
    }
    None
}

/// Concatenate every non-empty a11y name, for substring checks over the tree.
fn serialize_names(node: &A11yNode) -> String {
    let mut out = String::new();
    collect_names(node, &mut out);
    out
}

fn collect_names(node: &A11yNode, out: &mut String) {
    if !node.name.is_empty() {
        out.push_str(&node.name);
        out.push('\n');
    }
    if let Some(children) = &node.children {
        for child in children {
            collect_names(child, out);
        }
    }
}

pub struct SendMessageParams {
    pub chat_id: String,
    pub message: Option<String>,
    pub image_path: Option<String>,
    pub image_mime: Option<String>,
    pub file_path: Option<String>,
}

pub enum SendMessagePhase {
    Opening,
    Focusing,
    Inputting,
    Confirming,
    Done,
}

pub struct SendMessagePlanState {
    pub phase: SendMessagePhase,
    pub open_result: Option<OpenChatResult>,
    pub confirm_attempts: u32,
    /// Set when the plan recognises a blocker it cannot act around.
    pub failure: Option<String>,
}

#[async_trait::async_trait]
impl Plan for SendMessagePlan {
    type PlanState = SendMessagePlanState;
    type Params = SendMessageParams;

    fn id(&self) -> &str { "send_message" }

    fn initial_plan_state(&self) -> SendMessagePlanState {
        SendMessagePlanState {
            phase: SendMessagePhase::Opening,
            open_result: None,
            confirm_attempts: 0,
            failure: None,
        }
    }

    fn is_goal_reached(&self, _state: &AppState, plan_state: &SendMessagePlanState) -> bool {
        matches!(plan_state.phase, SendMessagePhase::Done)
    }

    fn failure_reason(&self, plan_state: &SendMessagePlanState) -> Option<String> {
        plan_state.failure.clone()
    }

    async fn select_action(
        &self,
        state: &AppState,
        params: &SendMessageParams,
        identified: &IdentifiedStates,
        plan_state: &mut SendMessagePlanState,
        a11y: &A11yNode,
        _session_id: &str,
    ) -> Option<SelectedAction> {
        let main_state_id = identified.main_window.as_ref().map(|m| m.state_id.as_str());

        // Dismiss popups
        if state.popup.is_some() && identified.popup.is_some() {
            return Some(SelectedAction {
                action: actions::dismiss_popup(),
                frame: identified.main_window.as_ref().and_then(|m| m.frame.clone()),
            });
        }

        loop {
            match &plan_state.phase {
                SendMessagePhase::Opening => {
                    if main_state_id != Some("chat") && main_state_id != Some("chat_open") {
                        return None;
                    }

                    let chat_list_item = query_selector(a11y, r#"list[name="Chats"] > list-item"#);
                    let click_xy = chat_list_item.and_then(|item| {
                        item.bounds.as_ref().map(|b| (
                            (b.x + b.width / 2.0).round(),
                            (b.y + b.height / 2.0).round(),
                        ))
                    });

                    // 省时间的关键路径：如果目标会话本来就开着（且输入框可用），
                    // 就不必跑 chat-select。那一趟要枚举全部会话（frida 内存扫描），
                    // 实测 1.0~1.7 秒，占了单条发送 2.5 秒的大半。
                    //
                    // 只有当 AppState 里的 opened_chat_name 与目标完全一致时才跳过，
                    // 否则老老实实切会话——发错人的代价远高于慢 1 秒。
                    // 省时间的关键路径：如果目标会话本来就开着（且输入框可用），
                    // 就不必跑 chat-select。那一趟要枚举全部会话（frida 内存扫描），
                    // 实测 600~1700 ms，占了单条发送 2.5 秒的大半。
                    //
                    // 判断依据：进程内缓存了上次成功打开的 chat_id → 会话显示名。
                    // 只在上次确实打开过同一 chat_id 时才跳过，否则走完整流程。
                    // 发错人的代价远高于慢 1 秒，所以这里宁保守勿激进。
                    let target_name = chat_name_cache_get(&params.chat_id);
                    let already = target_name
                        .as_deref()
                        .map(|name| already_open(state, identified, name))
                        .unwrap_or(false);

                    // 临时诊断：确认这个快速路径为什么没命中
                    tracing::info!(
                        "[send-diag] cached_name={:?} opened_name={:?} state={:?} already={}",
                        target_name,
                        state.main_window.opened_chat_name,
                        identified.main_window.as_ref().map(|m| m.state_id.clone()),
                        already
                    );

                    // 无论走哪条路径，只要知道当前打开的会话名，就更新缓存。
                    // 这样第一次发送后缓存就有值了，后续才能命中快速路径。
                    if let Some(name) = state.main_window.opened_chat_name.as_deref() {
                        chat_name_cache_put(&params.chat_id, name);
                    }

                    if already {
                        tracing::info!(
                            "[send] target chat already open ({:?}), skipping chat-select",
                            target_name
                        );
                        plan_state.open_result = Some(OpenChatResult {
                            ok: true,
                            username: Some(params.chat_id.clone()),
                            index: None,
                            skipped: Some(true),
                            error: None,
                        });
                        plan_state.phase = SendMessagePhase::Focusing;
                        continue;
                    }

                    let force = main_state_id == Some("chat");
                    let result = open_chat(&params.chat_id, force, click_xy).await;

                    if !result.ok {
                        return None;
                    }

                    let skipped = result.skipped.unwrap_or(false);
                    // 记录 chat_id → 显示名，供下次判断「是否已打开」使用
                    if let Some(name) = state.main_window.opened_chat_name.as_deref() {
                        chat_name_cache_put(&params.chat_id, name);
                    }
                    plan_state.open_result = Some(result);
                    plan_state.phase = SendMessagePhase::Focusing;

                    if !skipped {
                        return Some(SelectedAction {
                            action: actions::wait_short(),
                            frame: identified.main_window.as_ref().and_then(|m| m.frame.clone()),
                        });
                    }
                    continue;
                }

                SendMessagePhase::Focusing => {
                    if main_state_id != Some("chat_open") {
                        return None;
                    }

                    // A chat whose composer is replaced by a notice cannot take
                    // text at all. Name the real blocker instead of looping into
                    // a generic "No action selected".
                    if let Some(notice) = composer_block_notice(a11y) {
                        plan_state.failure = Some(notice);
                        return None;
                    }

                    let found = find_edit_and_send_button(a11y);
                    let (edit_node, _) = match found {
                        Some(f) => f,
                        None => return None,
                    };

                    plan_state.phase = SendMessagePhase::Inputting;

                    let is_focused = edit_node
                        .states
                        .as_ref()
                        .map(|s| s.iter().any(|st| st == "FOCUSED"))
                        .unwrap_or(false);

                    if is_focused {
                        continue;
                    }

                    if let Some(bounds) = &edit_node.bounds {
                        return Some(SelectedAction {
                            action: actions::click_bounds(bounds),
                            frame: identified.main_window.as_ref().and_then(|m| m.frame.clone()),
                        });
                    }
                    return None;
                }

                SendMessagePhase::Inputting => {
                    if let Some(notice) = composer_block_notice(a11y) {
                        plan_state.failure = Some(notice);
                        return None;
                    }
                    let found = find_edit_and_send_button(a11y);
                    if found.is_none() {
                        return None;
                    }

                    plan_state.phase = SendMessagePhase::Confirming;

                    // File
                    if let Some(fp) = &params.file_path {
                        exec_command("paste-file", &[fp], &ExecOptions::default()).await;
                        return Some(SelectedAction {
                            action: actions::sequence(vec![
                                Action::Wait { ms: 100 },
                                Action::Key { combo: "Return".to_string() },
                            ]),
                            frame: identified.main_window.as_ref().and_then(|m| m.frame.clone()),
                        });
                    }

                    // Image
                    if let Some(ip) = &params.image_path {
                        let mut args: Vec<&str> = vec![ip];
                        if let Some(mime) = &params.image_mime {
                            args.push(mime);
                        }
                        exec_command("paste-image", &args, &ExecOptions::default()).await;
                        return Some(SelectedAction {
                            action: actions::sequence(vec![
                                Action::Wait { ms: 100 },
                                Action::Key { combo: "Return".to_string() },
                            ]),
                            frame: identified.main_window.as_ref().and_then(|m| m.frame.clone()),
                        });
                    }

                    // Text
                    // Paste via the clipboard instead of typing character by
                    // character: the `input` tool sets the X clipboard and sends
                    // one Ctrl+V, which is dramatically faster for long messages
                    // and identical to what a user does when pasting.
                    if let Some(msg) = &params.message {
                        // Clear any leftover draft first, then paste the text.
                        exec_command("key", &["ctrl+a"], &ExecOptions::default()).await;
                        exec_command("input", &[msg.as_str()], &ExecOptions::default()).await;
                        return Some(SelectedAction {
                            action: actions::sequence(vec![
                                Action::Wait { ms: 100 },
                                Action::Key { combo: "Return".to_string() },
                            ]),
                            frame: identified.main_window.as_ref().and_then(|m| m.frame.clone()),
                        });
                    }

                    return None;
                }

                SendMessagePhase::Confirming => {
                    let found = find_edit_and_send_button(a11y);
                    let (_, send_btn) = match found {
                        Some(f) => f,
                        None => return None,
                    };

                    let is_disabled = send_btn
                        .states
                        .as_ref()
                        .map(|s| s.iter().any(|st| st == "DISABLED"))
                        .unwrap_or(false);

                    if is_disabled {
                        plan_state.phase = SendMessagePhase::Done;
                        return Some(SelectedAction {
                            action: actions::wait_short(),
                            frame: identified.main_window.as_ref().and_then(|m| m.frame.clone()),
                        });
                    }

                    plan_state.confirm_attempts += 1;
                    if plan_state.confirm_attempts >= 5 {
                        return None;
                    }

                    return Some(SelectedAction {
                        action: actions::wait_short(),
                        frame: identified.main_window.as_ref().and_then(|m| m.frame.clone()),
                    });
                }

                SendMessagePhase::Done => return None,
            }
        }
    }
}
