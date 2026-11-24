//! 数据模型：上游 API 的响应结构。

use serde::{Deserialize, Serialize};

/// 一次登录得到的会话凭证。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
    /// 用户 id（上游字段 `id`），后续课表/签到请求的 `id` 参数。
    #[serde(rename = "id")]
    pub user_id: String,
    /// 32 位十六进制会话 id（上游字段 `sessionId`），请求头 `sessionId`。
    pub session_id: String,
}

/// 单节课（来自课表接口）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Course {
    /// 7 位课程排期 id（`courseSchedId`），签到参数之一。
    pub id: String,
    /// 32 位课程 uuid。
    pub uuid: String,
    pub course_name: String,
    pub teacher_name: String,
    pub week_day: String,
    /// 形如 `2026-03-25 10:25:00`。
    pub class_begin_time: String,
    pub class_end_time: String,
    /// 签到状态（`0`/`1`，含义以上游为准）。
    pub sign_status: String,
}

/// 签到结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignResult {
    pub success: bool,
    pub message: String,
    /// 上游 `STATUS` 原始值。
    pub upstream_status: String,
    pub stu_sign_id: String,
    pub stu_sign_status: String,
}

impl Course {
    /// 上课时间范围字符串（`HH:MM ~ HH:MM`）。
    pub fn class_time_range(&self) -> String {
        format!(
            "{} ~ {}",
            Self::time_only(&self.class_begin_time),
            Self::time_only(&self.class_end_time),
        )
    }

    /// 签到状态可读文字：`1`→已签到，`0`→未签到。
    pub fn sign_status_label(&self) -> String {
        match self.sign_status.as_str() {
            "1" => "已签到".to_string(),
            "0" => "未签到".to_string(),
            other => format!("未知({other})"),
        }
    }

    /// 从上游时间串 `yyyy-MM-dd HH:mm:ss` 提取 `HH:MM`。
    fn time_only(datetime: &str) -> String {
        let time = datetime.rsplit(' ').next().unwrap_or(datetime);
        time.chars().take(5).collect()
    }
}

