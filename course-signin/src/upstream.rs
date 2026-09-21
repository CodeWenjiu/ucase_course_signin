//! 上游 iclass API 封装。
//!
//! 复刻参考项目（UCAS-Course-Sign-in）逆向出的链路：
//! 登录、时间戳校准、课表查询、直接签到。所有请求伪装成官方 Android App。

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::header::{CONTENT_TYPE, USER_AGENT};
use serde::Deserialize;

use crate::config::DEFAULT_BASE_URL;
use crate::model::{AuthSession, Course, SignResult};

/// 官方 Android App 登录接口 UA。
const LOGIN_UA: &str = "student_5.0.1.2_android_12_20__110000";
/// 官方 Android App 业务接口 UA。
const API_UA: &str = "student_5.0.1.2_android_12_20_100000000000000_110000";
/// 单次上游请求超时。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 时间戳接口与签到接口运行在不同服务器，时钟偏差约 3.5s。
/// 发送签到时间戳时需减去该缓冲，否则会被上游以 `ERRCODE=100` 拒绝。
pub const SIGN_TIMESTAMP_BUFFER_MS: i64 = 3000;

/// 上游路径。
const LOGIN_PATH: &str = "/app/user/login.action";
const TIMESTAMP_PATH: &str = "/app/common/get_timestamp.do";
const SCHEDULE_PATH: &str = "/app/course/get_stu_course_sched.action";
const SIGN_PATH: &str = "/app/course/stu_scan_sign.action";

/// 上游登录请求体中的验证 URL 模板（保持与官方 App 一致）。
const VERIFICATION_URL_TEMPLATE: &str =
    "http://iclass.ucas.edu.cn:88/ve/webservices/mobileCheck.shtml?method=mobileLogin&username=${0}&password=${1}&lx=${2}";

/// iclass 上游客户端。
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    /// 学号（`with_credentials` 后可用）。
    username: Option<String>,
    /// 密码（`with_credentials` 后可用）。
    password: Option<String>,
}

impl Client {
    /// 仅指定上游地址，用于无需凭据的探测（如服务器时间）。
    pub fn new(base_url: &str) -> Result<Self> {
        Self::build(base_url, None, None)
    }

    /// 携带凭据，用于登录/课表/签到。
    pub fn with_credentials(
        base_url: &str,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self> {
        Self::build(
            base_url,
            Some(username.into()),
            Some(password.into()),
        )
    }

    /// 使用默认上游地址且携带凭据。
    pub fn with_credentials_default_base(
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self> {
        Self::with_credentials(DEFAULT_BASE_URL, username, password)
    }

    fn build(base_url: &str, username: Option<String>, password: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("构建 HTTP 客户端失败")?;

        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            username,
            password,
        })
    }

    /// 发起登录，返回会话凭证。
    pub async fn login(&self) -> Result<AuthSession> {
        let username = self
            .username
            .as_deref()
            .ok_or_else(|| anyhow!("客户端未配置凭据"))?;
        let password = self
            .password
            .as_deref()
            .ok_or_else(|| anyhow!("客户端未配置凭据"))?;

        let body = [
            ("phone", username),
            ("password", password),
            ("verificationType", "1"),
            ("verificationUrl", VERIFICATION_URL_TEMPLATE),
            ("userLevel", "1"),
        ];

        let resp: LoginResponse = self
            .http
            .post(self.url(LOGIN_PATH))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(USER_AGENT, LOGIN_UA)
            .form(&body)
            .send()
            .await
            .context("登录请求失败")?
            .error_for_status()
            .context("登录接口 HTTP 异常")?
            .json()
            .await
            .context("登录接口返回非 JSON")?;

        if resp.status != "0" {
            return Err(anyhow!("登录失败：STATUS={}", resp.status));
        }

        let result = resp
            .result
            .ok_or_else(|| anyhow!("登录响应缺少 result"))?;
        let session_id = result
            .session_id
            .filter(|s| s.len() == 32)
            .ok_or_else(|| anyhow!("登录响应缺少合法 sessionId"))?;

        Ok(AuthSession {
            user_id: result.id,
            session_id,
        })
    }

    /// 获取上游服务器时间（毫秒）。
    pub async fn server_timestamp(&self) -> Result<i64> {
        // 随机 id 参数防止中间节点缓存
        let url = format!("{}?id={}", self.url(TIMESTAMP_PATH), rand_id());

        let resp: TimestampResponse = self
            .http
            .post(url)
            .header(USER_AGENT, API_UA)
            .header("Connection", "Keep-Alive")
            .send()
            .await
            .context("时间戳请求失败")?
            .error_for_status()
            .context("时间戳接口 HTTP 异常")?
            .json()
            .await
            .context("时间戳接口返回非 JSON")?;

        if resp.status != "0" {
            return Err(anyhow!("时间戳接口 STATUS={}", resp.status));
        }

        Ok(resp.timestamp)
    }

    /// 查询某天课表。`date` 形如 `yyyyMMdd`。
    pub async fn get_schedule(&self, session: &AuthSession, date: &str) -> Result<Vec<Course>> {
        let url = format!(
            "{}?id={}&dateStr={}",
            self.url(SCHEDULE_PATH),
            urlencode(&session.user_id),
            urlencode(date),
        );

        let resp: ScheduleResponse = self
            .http
            .get(url)
            .header("sessionId", &session.session_id)
            .header(USER_AGENT, API_UA)
            .send()
            .await
            .context("课表请求失败")?
            .error_for_status()
            .context("课表接口 HTTP 异常")?
            .json()
            .await
            .context("课表接口返回非 JSON")?;

        if resp.status != "0" {
            return Err(anyhow!("课表查询失败：STATUS={}", resp.status));
        }

        Ok(resp.result.unwrap_or_default())
    }

    /// 直接签到。
    ///
    /// `timestamp_ms` 为校准后的签到时间戳（服务器时间减去
    /// [`SIGN_TIMESTAMP_BUFFER_MS`] 缓冲）。
    pub async fn scan_sign(
        &self,
        session: &AuthSession,
        course_sched_id: &str,
        timestamp_ms: i64,
    ) -> Result<SignResult> {
        let url = format!(
            "{}?courseSchedId={}&timestamp={}&id={}",
            self.url(SIGN_PATH),
            urlencode(course_sched_id),
            timestamp_ms,
            urlencode(&session.user_id),
        );

        let resp: SignResponse = self
            .http
            .get(url)
            .header("sessionId", &session.session_id)
            .header(USER_AGENT, API_UA)
            .send()
            .await
            .context("签到请求失败")?
            .error_for_status()
            .context("签到接口 HTTP 异常")?
            .json()
            .await
            .context("签到接口返回非 JSON")?;

        let upstream_status = resp.status;
        let result = resp.result.unwrap_or_default();
        let stu_sign_id = result.stu_sign_id.unwrap_or_default();
        let stu_sign_status = result.stu_sign_status.unwrap_or_default();
        let message = resp
            .message
            .or(resp.errmsg)
            .or(resp.msg)
            .or_else(|| result.message)
            .unwrap_or_else(|| "签到失败，请稍后重试".to_string());

        let success = upstream_status == "0" && stu_sign_status == "1";

        Ok(SignResult {
            success,
            message,
            upstream_status,
            stu_sign_id,
            stu_sign_status,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

/// 生成 0..1_000_000 的随机数。
fn rand_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos as u64) % 1_000_000
}

/// URL 编码（空格 → %20）。
fn urlencode(s: &str) -> String {
    s.replace('%', "%25").replace(' ', "%20")
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    #[serde(rename = "STATUS")]
    status: String,
    result: Option<LoginResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginResult {
    id: String,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TimestampResponse {
    #[serde(rename = "STATUS")]
    status: String,
    timestamp: i64,
}

#[derive(Debug, Deserialize)]
struct ScheduleResponse {
    #[serde(rename = "STATUS")]
    status: String,
    result: Option<Vec<Course>>,
}

#[derive(Debug, Deserialize)]
struct SignResponse {
    #[serde(rename = "STATUS")]
    status: String,
    message: Option<String>,
    msg: Option<String>,
    #[serde(rename = "ERRMSG")]
    errmsg: Option<String>,
    result: Option<SignResultBody>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignResultBody {
    stu_sign_id: Option<String>,
    stu_sign_status: Option<String>,
    message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encodes_query_values() {
        assert_eq!(urlencode("2026-03-25"), "2026-03-25");
        assert_eq!(urlencode("a b"), "a%20b");
    }

    #[test]
    fn parses_timestamp_response() {
        let json = r#"{"STATUS":"0","timestamp":1770000000000}"#;
        let resp: TimestampResponse = serde_json::from_str(json).expect("解析失败");
        assert_eq!(resp.status, "0");
        assert_eq!(resp.timestamp, 1770000000000);
    }

    #[test]
    fn parses_login_response() {
        let json = r#"{"STATUS":"0","result":{"id":"20250001","sessionId":"AABBCCDDEEFF00112233445566778899"}}"#;
        let resp: LoginResponse = serde_json::from_str(json).expect("解析失败");
        assert_eq!(resp.status, "0");
        let result = resp.result.expect("缺少 result");
        assert_eq!(result.id, "20250001");
        assert_eq!(result.session_id.as_deref(), Some("AABBCCDDEEFF00112233445566778899"));
    }

    #[test]
    fn parses_sign_success_response() {
        let json = r#"{"STATUS":"0","message":"签到成功","result":{"stuSignId":"123456","stuSignStatus":"1"}}"#;
        let resp: SignResponse = serde_json::from_str(json).expect("解析失败");
        assert_eq!(resp.status, "0");
        let result = resp.result.expect("缺少 result");
        assert_eq!(result.stu_sign_id.as_deref(), Some("123456"));
        assert_eq!(result.stu_sign_status.as_deref(), Some("1"));
    }
}
