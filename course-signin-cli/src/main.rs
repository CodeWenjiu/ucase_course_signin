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
