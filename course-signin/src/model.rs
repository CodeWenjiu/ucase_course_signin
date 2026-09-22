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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Course {
        Course {
            id: "1222807".into(),
            uuid: "CADD27F17ACC44EDAF00000000000001".into(),
            course_name: "面向对象程序设计—C++".into(),
            teacher_name: "刘立祥".into(),
            week_day: "周一".into(),
            class_begin_time: "2026-09-21 18:30:00".into(),
            class_end_time: "2026-09-21 21:00:00".into(),
            sign_status: "1".into(),
        }
    }

    #[test]
    fn class_time_range_drops_date_and_seconds() {
        let c = sample();
        assert_eq!(c.class_time_range(), "18:30 ~ 21:00");
    }

    #[test]
    fn sign_status_label_maps_values() {
        let mut c = sample();
        assert_eq!(c.sign_status_label(), "已签到");
        c.sign_status = "0".into();
        assert_eq!(c.sign_status_label(), "未签到");
        c.sign_status = "2".into();
        assert_eq!(c.sign_status_label(), "未知(2)");
        c.sign_status = String::new();
        assert_eq!(c.sign_status_label(), "未知()");
    }

    #[test]
    fn class_time_range_handles_abnormal_input() {
        let mut c = sample();
        c.class_begin_time = "无空格日期".into();
        c.class_end_time = String::new();
        assert_eq!(c.class_time_range(), "无空格日期 ~ ");
    }

    #[test]
    fn parses_course_list_from_upstream() {
        // 取自参考项目 README 的响应示例
        let json = r#"{
            "date": "20260325",
            "total": 2,
            "courses": [{
                "id": "1145140",
                "uuid": "CADD27F17ACC44EDAF00000000000001",
                "courseName": "计算机网络",
                "teacherName": "张三",
                "weekDay": "周三",
                "classBeginTime": "2026-03-25 10:25:00",
                "classEndTime": "2026-03-25 12:00:00",
                "signStatus": "1"
            }]
        }"#;

        #[derive(Deserialize)]
        struct QueryResp {
            courses: Vec<Course>,
        }

        let resp: QueryResp = serde_json::from_str(json).expect("解析失败");
        assert_eq!(resp.courses.len(), 1);
        let c = &resp.courses[0];
        assert_eq!(c.id, "1145140");
        assert_eq!(c.uuid, "CADD27F17ACC44EDAF00000000000001");
        assert_eq!(c.course_name, "计算机网络");
        assert_eq!(c.class_begin_time, "2026-03-25 10:25:00");
        assert_eq!(c.sign_status, "1");
    }

    #[test]
    fn parses_login_session() {
        let json = r#"{"STATUS":"0","result":{"id":"20250001","sessionId":"AABBCCDDEEFF00112233445566778899"}}"#;

        #[derive(Deserialize)]
        struct LoginResp {
            #[serde(rename = "STATUS")]
            status: String,
            result: Option<AuthSession>,
        }

        let resp: LoginResp = serde_json::from_str(json).expect("解析失败");
        assert_eq!(resp.status, "0");
        let session = resp.result.expect("缺少 result");
        assert_eq!(session.user_id, "20250001");
        assert_eq!(session.session_id, "AABBCCDDEEFF00112233445566778899");
    }

    #[test]
    fn parses_sign_response() {
        let json = r#"{"STATUS":"0","result":{"stuSignId":"123456","stuSignStatus":"1"}}"#;

        #[derive(Deserialize)]
        struct SignResp {
            #[serde(rename = "STATUS")]
            status: String,
            result: serde_json::Value,
        }

        let resp: SignResp = serde_json::from_str(json).expect("解析失败");
        assert_eq!(resp.status, "0");
        assert_eq!(resp.result["stuSignId"], "123456");
    }
}
