//! chat_id → 会话显示名 的进程内缓存。
//!
//! 用途：发送消息时判断目标会话是否已经打开，从而跳过昂贵的 chat-select。
//!
//! 为什么需要：open_chat 内部跑 chat-select 要枚举全部会话（frida 内存扫描），
//! 实测 600~1700ms。而单条消息发送总耗时约 2.5 秒，这段占了绝大头。
//! 如果目标会话本来就开着，这一趟纯属浪费。
//!
//! 为什么用缓存而不是查库：查 SQLCipher 要解密 session.db，成本不低；
//! 而映射关系在运行期几乎不变，缓存足够。
//!
//! 安全性：只在「上次确实打开过同一个 chat_id」时才跳过。
//! 缓存没有对应条目，或状态不符，一律走原流程。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn store() -> &'static Mutex<HashMap<String, String>> {
    static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 记录一次成功的会话打开，供后续判断使用。
pub fn chat_name_cache_put(chat_id: &str, name: &str) {
    if chat_id.is_empty() || name.is_empty() {
        return;
    }
    if let Ok(mut map) = store().lock() {
        // 上限保护：长期运行不应无限增长
        if map.len() > 512 {
            map.clear();
        }
        map.insert(chat_id.to_string(), name.to_string());
    }
}

/// 查缓存。没有条目返回 None（调用方应走完整流程）。
pub fn chat_name_cache_get(chat_id: &str) -> Option<String> {
    store().lock().ok()?.get(chat_id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_trip() {
        chat_name_cache_put("wxid_test_a", "测试会话");
        assert_eq!(chat_name_cache_get("wxid_test_a").as_deref(), Some("测试会话"));
    }

    #[test]
    fn unknown_returns_none() {
        assert_eq!(chat_name_cache_get("wxid_never_seen"), None);
    }

    #[test]
    fn empty_inputs_ignored() {
        chat_name_cache_put("", "x");
        chat_name_cache_put("wxid_y", "");
        assert_eq!(chat_name_cache_get(""), None);
        assert_eq!(chat_name_cache_get("wxid_y"), None);
    }
}
