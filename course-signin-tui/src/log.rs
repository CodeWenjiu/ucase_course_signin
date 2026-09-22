//! 简易日志：事件以 JSON 行追加写入用户状态目录，便于排查疑难故障。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// 日志文件路径：`$XDG_STATE_HOME/course-signin/tui.log`（缺省 `~/.local/state/...`）。
pub fn log_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("course-signin").join("tui.log")
}

/// 追加一条日志（写到文件失败时静默忽略，不影响主流程）。
pub fn log_event(msg: &str) {
    let entry = format!(
        "{{\"ts\":\"{}\",\"msg\":{}}}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        serde_json::to_string(msg).unwrap_or_else(|_| "\"???\"".to_string()),
    );
    let path = log_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("[log] 无法创建日志目录 {}: {e}", parent.display());
            return;
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(entry.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_is_under_state_dir() {
        let p = log_path();
        assert!(
            p.ends_with("course-signin/tui.log"),
            "路径: {}",
            p.display()
        );
    }
}
