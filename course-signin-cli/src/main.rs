//! UCAS 课程签到 CLI。

use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};
use course_signin::config::{DEFAULT_BASE_URL, DEFAULT_PASSWORD};
use course_signin::model::Course;
use course_signin::upstream::{Client, SIGN_TIMESTAMP_BUFFER_MS};
use tabled::{Table, Tabled};

#[derive(Debug, Parser)]
#[command(
    name = "course-signin-cli",
    version,
    about = "UCAS 课程签到：读取课表并在上课时间自动签到",
    propagate_version = true
)]
struct Cli {
    /// 学号（或环境变量 UCAS_USERNAME）
    #[arg(long, env = "UCAS_USERNAME", global = true)]
    username: Option<String>,

    /// 密码（或环境变量 UCAS_PASSWORD；缺省为默认密码）
    #[arg(long, env = "UCAS_PASSWORD", global = true, default_value = DEFAULT_PASSWORD)]
    password: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 查询某天课表（默认今天）
    Query {
        /// 查询日期，格式 yyyy-MM-dd 或 yyyyMMdd，默认今天
        #[arg(long)]
        date: Option<String>,

        /// 以 JSON 形式输出
        #[arg(long)]
        json: bool,
    },

    /// 获取上游服务器时间戳（毫秒，用于联调探测，无需登录）
    Timestamp,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("错误: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Timestamp => {
            let client = Client::new(DEFAULT_BASE_URL)?;
            let ts = client.server_timestamp().await?;
            let now = chrono::Utc::now().timestamp_millis();
            println!("服务器时间戳: {ts} ms");
            println!("本地时间戳:   {now} ms");
            println!("偏差:         {:+} ms", ts - now);
            println!(
                "签到用时间戳（减 {}ms 缓冲）: {}",
                SIGN_TIMESTAMP_BUFFER_MS,
                ts - SIGN_TIMESTAMP_BUFFER_MS
            );
        }

        Commands::Query { ref date, json } => {
            let (username, password) = require_credentials(&cli)?;
            let client = Client::with_credentials_default_base(username, password)?;
            let date = normalize_date(date.as_deref())?;

            eprintln!("正在登录 ...");
            let session = client.login().await?;
            eprintln!(
                "登录成功（sessionId 前 8 位: {}）",
                &session.session_id[..8]
            );

            eprintln!("正在查询 {date} 课表 ...");
            let courses = client.get_schedule(&session, &date).await?;

            if json {
                println!("{}", serde_json::to_string_pretty(&courses)?);
            } else {
                print_courses(&courses, &date);
            }
        }
    }

    Ok(())
}

/// 从参数或环境变量（clap env 特性已注入）取凭据。
/// 密码缺省（未提供或为空串）时回退到 UCAS 默认密码。
fn require_credentials(cli: &Cli) -> Result<(String, String)> {
    let username = cli
        .username
        .clone()
        .ok_or_else(|| anyhow!("缺少学号：请用 --username 或环境变量 UCAS_USERNAME"))?;

    if username.is_empty() {
        return Err(anyhow!("学号不能为空"));
    }

    let password = if cli.password.is_empty() {
        DEFAULT_PASSWORD.to_string()
    } else {
        cli.password.clone()
    };

    Ok((username, password))
}

/// 把 `yyyy-MM-dd` / `yyyyMMdd` 归一化为 `yyyyMMdd`，缺省为今天。
fn normalize_date(input: Option<&str>) -> Result<String> {
    match input {
        None => Ok(Local::now().format("%Y%m%d").to_string()),
        Some(raw) => {
            let compact = raw.replace('-', "");
            let parsed = NaiveDate::parse_from_str(&compact, "%Y%m%d")
                .context("日期格式错误，请使用 yyyy-MM-dd 或 yyyyMMdd")?;
            Ok(parsed.format("%Y%m%d").to_string())
        }
    }
}

/// 人类可读地打印课表（tabled 表格）。
fn print_courses(courses: &[Course], date: &str) {
    println!("=== {date} 课程表（{} 门课）===", courses.len());
    println!("{}", Table::new(build_rows(courses)));
}

/// 课表行列结构（tabled 表格）。
#[derive(Tabled)]
struct CourseRow {
    #[tabled(rename = "#")]
    index: usize,
    #[tabled(rename = "课程")]
    course_name: String,
    #[tabled(rename = "上课时间")]
    class_time: String,
    #[tabled(rename = "教师")]
    teacher_name: String,
    #[tabled(rename = "签到状态")]
    sign_status: String,
    #[tabled(rename = "ID")]
    id: String,
}

/// 把课程列表转换为表格行，签到状态转为可读文字。
/// 只查当天课表，故省略周几；上课时间只保留 `HH:MM`。
fn build_rows(courses: &[Course]) -> Vec<CourseRow> {
    courses
        .iter()
        .enumerate()
        .map(|(i, c)| CourseRow {
            index: i + 1,
            course_name: c.course_name.clone(),
            class_time: c.class_time_range(),
            teacher_name: c.teacher_name.clone(),
            sign_status: c.sign_status_label(),
            id: c.id.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_date_formats() {
        assert_eq!(normalize_date(None).is_ok(), true);
        assert_eq!(normalize_date(Some("2026-03-25")).unwrap(), "20260325");
        assert_eq!(normalize_date(Some("20260325")).unwrap(), "20260325");
        assert!(normalize_date(Some("2026/03/25")).is_err());
    }

    #[test]
    fn requires_username_but_defaults_password() {
        let missing = Cli {
            username: None,
            password: DEFAULT_PASSWORD.to_string(),
            command: Commands::Query {
                date: None,
                json: false,
            },
        };
        assert!(require_credentials(&missing).is_err());

        let present = Cli {
            username: Some("20250001".into()),
            password: DEFAULT_PASSWORD.to_string(),
            command: Commands::Query {
                date: None,
                json: false,
            },
        };
        let (user, pwd) = require_credentials(&present).expect("应成功");
        assert_eq!(user, "20250001");
        assert_eq!(pwd, DEFAULT_PASSWORD);
    }

    #[test]
    fn empty_password_falls_back_to_default() {
        let cli = Cli {
            username: Some("20250001".into()),
            password: String::new(),
            command: Commands::Query {
                date: None,
                json: false,
            },
        };
        let (_, pwd) = require_credentials(&cli).expect("应成功");
        assert_eq!(pwd, DEFAULT_PASSWORD);
    }

    #[test]
    fn course_table_contains_status_labels() {
        let courses = vec![Course {
            id: "1222807".into(),
            uuid: "CADD27F17ACC44EDAF00000000000001".into(),
            course_name: "面向对象程序设计—C++".into(),
            teacher_name: "刘立祥".into(),
            week_day: "周一".into(),
            class_begin_time: "2026-09-21 18:30:00".into(),
            class_end_time: "2026-09-21 21:00:00".into(),
            sign_status: "1".into(),
        }];

        let table = Table::new(build_rows(&courses)).to_string();
        assert!(table.contains("面向对象程序设计—C++"));
        assert!(table.contains("已签到"));
        assert!(table.contains("18:30 ~ 21:00"));
        // 当天课表：不显示周几与日期
        assert!(!table.contains("周一"));
        assert!(!table.contains("2026-09-21"));
        assert!(table.contains("1222807"));
    }

    #[test]
    fn class_time_range_drops_date_and_seconds() {
        let courses = vec![Course {
            id: "1222807".into(),
            uuid: "CADD27F17ACC44EDAF00000000000001".into(),
            course_name: "面向对象程序设计—C++".into(),
            teacher_name: "刘立祥".into(),
            week_day: "周一".into(),
            class_begin_time: "2026-09-21 18:30:00".into(),
            class_end_time: "2026-09-21 21:00:00".into(),
            sign_status: "0".into(),
        }];

        let table = Table::new(build_rows(&courses)).to_string();
        assert!(table.contains("18:30 ~ 21:00"));
        assert!(table.contains("未签到"));
    }
}
