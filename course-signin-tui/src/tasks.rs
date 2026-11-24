//! 后台任务（网络侧）：课表刷新与签到，结果经 channel 回传主循环。

use course_signin::model::{Course, SignResult};
use course_signin::schedule::sign_timestamp;
use course_signin::upstream::Client;
use tokio::sync::mpsc;

use crate::app::Command;

/// 后台任务回传的结果。
pub enum TaskResult {
    /// 课表刷新结果。
    Refresh(Result<Vec<Course>, String>),
    /// 一次签到尝试的结果。
    Sign(SignTaskResult),
}

/// 签到尝试的结果（关联课程 id，索引由主循环按最新时间线解析）。
pub struct SignTaskResult {
    pub course_id: String,
    pub result: Result<SignResult, String>,
}

/// 把错误格式化为带完整 cause 链的字符串。
fn err_chain(err: impl std::error::Error + Send + Sync + 'static) -> String {
    format!("{:#}", anyhow::Error::new(err))
}

/// 登录并拉取某天课表。
async fn refresh_schedule(client: &Client, date: &str) -> Result<Vec<Course>, String> {
    let session = client.login().await.map_err(err_chain)?;
    client.get_schedule(&session, date).await.map_err(err_chain)
}

/// 登录 → 校准服务器时间 → 直接签到。
async fn sign_course(client: &Client, course_id: &str) -> Result<SignResult, String> {
    let session = client.login().await.map_err(err_chain)?;
    let server_ts = client.server_timestamp().await.map_err(err_chain)?;
    client
        .scan_sign(&session, course_id, sign_timestamp(server_ts))
        .await
        .map_err(err_chain)
}

/// 执行应用给出的后台任务（启动对应的 tokio 任务）。
pub fn execute(client: &Client, cmd: Command, tx: &mpsc::Sender<TaskResult>) {
    match cmd {
        Command::Refresh { date } => spawn_refresh(client.clone(), date, tx.clone()),
        Command::Sign { course_id } => spawn_sign(client.clone(), course_id, tx.clone()),
    }
}

/// 启动一次后台课表刷新。
fn spawn_refresh(client: Client, date: String, tx: mpsc::Sender<TaskResult>) {
    tokio::spawn(async move {
        let result = refresh_schedule(&client, &date).await;
        let _ = tx.send(TaskResult::Refresh(result)).await;
    });
}

/// 启动一次后台签到尝试。
fn spawn_sign(client: Client, course_id: String, tx: mpsc::Sender<TaskResult>) {
    tokio::spawn(async move {
        let result = sign_course(&client, &course_id).await;
        let _ = tx
            .send(TaskResult::Sign(SignTaskResult { course_id, result }))
            .await;
    });
}
