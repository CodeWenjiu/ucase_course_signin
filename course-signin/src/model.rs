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

#[cfg(test)]
mod tests {
    use super::*;

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
