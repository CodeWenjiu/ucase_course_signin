//! 运行配置：凭据与上游地址。

use anyhow::{Context, Result};

/// 上游 iclass 服务默认地址。
pub const DEFAULT_BASE_URL: &str = "https://iclass.ucas.edu.cn:8181";

/// UCAS 默认初始密码（未修改密码的账号通用）。
pub const DEFAULT_PASSWORD: &str = "Ucas@2025";

/// 程序配置。
#[derive(Debug, Clone)]
pub struct Config {
    /// 学号。
    pub username: String,
    /// 密码。
    pub password: String,
    /// 上游服务地址（默认 `DEFAULT_BASE_URL`）。
    pub base_url: String,
}

impl Config {
    /// 从学号/密码/上游地址构建配置。
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Result<Self> {
        let username = username.into();
        let password = password.into();
        let base_url = base_url.into();

        anyhow::ensure!(
            !username.is_empty() && !password.is_empty(),
            "学号与密码不能为空"
        );

        Ok(Self {
            username,
            password,
            base_url,
        })
    }

    /// 使用默认上游地址构建配置。
    pub fn new_with_default_base(
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self> {
        Self::new(username, password, DEFAULT_BASE_URL).context("配置无效")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_ucas_base_url() {
        let config = Config::new("20250001", "secret", DEFAULT_BASE_URL).expect("配置应有效");
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.username, "20250001");
    }

    #[test]
    fn rejects_empty_credentials() {
        assert!(Config::new("", "pwd", DEFAULT_BASE_URL).is_err());
        assert!(Config::new("20250001", "", DEFAULT_BASE_URL).is_err());
    }
}
