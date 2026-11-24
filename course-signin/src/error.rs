//! `course-signin` 统一错误类型。

use thiserror::Error;

/// 库统一错误类型。
///
/// 每个变体对应一类可区分的失败场景，`#[source]` 保留底层 reqwest 错误链。
#[derive(Debug, Error)]
pub enum Error {
    /// 学号或密码为空。
    #[error("学号与密码不能为空")]
    EmptyCredentials,
    /// 客户端未配置凭据（调用登录/签到前须 `with_credentials`）。
    #[error("客户端未配置凭据")]
    MissingCredentials,
    /// 构建 HTTP 客户端失败。
    #[error("构建 HTTP 客户端失败")]
    HttpClient(#[source] reqwest::Error),
    /// 上游请求发送失败（连接/超时等）。
    #[error("{interface}请求失败")]
    Request {
        /// 上游接口名（如“登录”）。
        interface: &'static str,
        /// 底层传输错误。
        #[source]
        source: reqwest::Error,
    },
    /// 上游返回非成功 HTTP 状态。
    #[error("{interface}接口 HTTP 异常")]
    HttpStatus {
        /// 上游接口名。
        interface: &'static str,
        /// 底层状态错误。
        #[source]
        source: reqwest::Error,
    },
    /// 响应体不是合法 JSON。
    #[error("{interface}接口返回非 JSON")]
    Decode {
        /// 上游接口名。
        interface: &'static str,
        /// 底层解码错误。
        #[source]
        source: reqwest::Error,
    },
    /// 上游返回非预期的 STATUS 字段值。
    #[error("{interface}接口返回异常状态 STATUS={status}")]
    UnexpectedStatus {
        /// 上游接口名。
        interface: &'static str,
        /// 实际收到的 STATUS。
        status: String,
    },
    /// 登录响应缺少 result 字段。
    #[error("登录响应缺少 result")]
    MissingLoginResult,
    /// 登录响应缺少合法 sessionId。
    #[error("登录响应缺少合法 sessionId")]
    InvalidSessionId,
}

/// 库内 `Result` 别名。
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_human_readable_messages() {
        assert_eq!(Error::EmptyCredentials.to_string(), "学号与密码不能为空");
        assert_eq!(Error::MissingCredentials.to_string(), "客户端未配置凭据");
        assert_eq!(
            Error::UnexpectedStatus {
                interface: "登录",
                status: "1".to_string(),
            }
            .to_string(),
            "登录接口返回异常状态 STATUS=1"
        );
        assert_eq!(
            Error::Decode {
                interface: "课表",
                source: fake_reqwest_error(),
            }
            .to_string(),
            "课表接口返回非 JSON"
        );
    }

    #[test]
    fn exposes_source_chain() {
        let err = Error::Request {
            interface: "登录",
            source: fake_reqwest_error(),
        };
        let source = std::error::Error::source(&err).expect("应暴露底层 source");
        assert!(!source.to_string().is_empty());
    }

    /// 构造一个真实的 reqwest::Error（URL 解析失败）。
    fn fake_reqwest_error() -> reqwest::Error {
        reqwest::Client::new()
            .get("::invalid::url")
            .build()
            .expect_err("非法 URL 应构建失败")
    }
}
