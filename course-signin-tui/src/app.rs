//! 应用状态与业务编排：纯状态机，不依赖 tokio/网络。
//!
//! 调度到点的动作由 [`App::take_due`] 取出，经 [`App::handle_due`] 翻译为
//! 待执行的 [`Command`]（刷新 / 签到）；网络任务由 `main` 交给 `tasks` 执行。

use std::collections::HashMap;

use chrono::{Local, NaiveDateTime};
use course_signin::model::Course;

use crate::scheduler::{Due, Scheduler, SignOutcome, SignTrigger};
use crate::tasks::SignTaskResult;

/// 需要由上层执行的后台任务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// 刷新指定日期的课表。
    Refresh { date: String },
    /// 对指定课程发起签到。
    Sign { course_id: String },
}

/// 某门课的签到过程注记（覆盖表格中的签到状态列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignNote {
    /// 请求进行中。
    Pending,
    /// 签到成功。
    Success(String),
    /// 失败但会重试。
    Retrying(String),
    /// 失败（不再重试）。
    Failed(String),
}

/// 应用状态。
pub struct App {
    pub username: String,
    /// 当前课表日期（`yyyyMMdd`）。
    pub date: String,
    /// 状态栏文字。
    pub status_text: String,
    /// 最近一次成功刷新的时刻（`HH:MM:SS`）。
    pub last_updated: Option<String>,
    /// 自动调度开关（'a' 键切换）。
    pub auto_refresh: bool,
    /// 是否有课表刷新任务在跑。
    pub refresh_running: bool,
    /// 当前展示的课表（上游原始顺序）。
    pub courses: Vec<Course>,

    sign_notes: HashMap<String, SignNote>,
    scheduler: Scheduler,
}

impl App {
    /// 构建应用状态（以当前本地时间初始化调度器）。
    pub fn new(username: impl Into<String>, date: impl Into<String>) -> Self {
        Self::with_now(username, date, Local::now().naive_local())
    }

    /// 以指定时刻初始化调度器（测试用，避免依赖真实时钟）。
    pub fn with_now(
        username: impl Into<String>,
        date: impl Into<String>,
        now: NaiveDateTime,
    ) -> Self {
        Self {
            username: username.into(),
            date: date.into(),
            status_text: "就绪，按 r 手动刷新".to_string(),
            last_updated: None,
            auto_refresh: true,
            refresh_running: false,
            courses: Vec::new(),
            sign_notes: HashMap::new(),
            scheduler: Scheduler::new(now),
        }
    }

    /// 当前/最近签到窗口的展示文字（无课时返回 None）。
    pub fn sign_window_text(&self) -> Option<String> {
        let now = Local::now().naive_local();
        self.scheduler.sign_window_hint(now).map(|hint| {
            format!(
                "〔{}〕{} ~ {}",
                hint.course_name,
                hint.window_open.format("%H:%M"),
                hint.class_end.format("%H:%M"),
            )
        })
    }

    /// 下一次唤醒时刻（供顶栏展示）。
    pub fn next_wakeup_text(&self) -> String {
        let now = Local::now().naive_local();
        self.scheduler
            .next_deadline(now)
            .format("%H:%M:%S")
            .to_string()
    }

    /// 课程签到过程注记。
    pub fn sign_note(&self, course_id: &str) -> Option<&SignNote> {
        self.sign_notes.get(course_id)
    }

    /// 取出一个到点的调度动作。
    pub fn take_due(&mut self, now: NaiveDateTime) -> Option<Due> {
        self.scheduler.take_due(now)
    }

    /// 距离下一个调度动作的休眠时长（至少 200ms，避免忙循环）。
    pub fn sleep_duration(&self, now: NaiveDateTime) -> std::time::Duration {
        let ms = self
            .scheduler
            .next_deadline(now)
            .signed_duration_since(now)
            .num_milliseconds();
        if ms > 0 {
            std::time::Duration::from_millis(ms as u64)
        } else {
            std::time::Duration::from_millis(200)
        }
    }

    /// 执行一个到点的调度动作，返回需要交给后台执行的任务（如有）。
    pub fn handle_due(&mut self, due: Due) -> Option<Command> {
        match due {
            // 例行刷新只拉课表，不重置任何签到状态（日期在刷新前自动同步）
            Due::RoutineRefresh => {
                crate::log::log_event("routine refresh");
                self.refresh("每小时例行刷新")
            }
            Due::Sign { idx, trigger } => self.start_sign(idx, trigger),
            Due::Refresh { trigger } => self.refresh(trigger.describe()),
        }
    }

    /// 手动刷新（'r' 键）。
    pub fn manual_refresh(&mut self) -> Option<Command> {
        self.refresh("手动刷新")
    }

    /// 开始一次刷新（已有刷新在跑时忽略）。刷新前同步为当前日期。
    pub fn refresh(&mut self, reason: &str) -> Option<Command> {
        if self.refresh_running {
            return None;
        }
        self.date = Local::now().format("%Y%m%d").to_string();
        self.refresh_running = true;
        self.status_text = format!("{reason} ...");
        crate::log::log_event(&format!("refresh start date={}", self.date));
        Some(Command::Refresh {
            date: self.date.clone(),
        })
    }

    /// 对时间线上的第 `idx` 门课发起签到。
    fn start_sign(&mut self, idx: usize, trigger: SignTrigger) -> Option<Command> {
        let course = self.scheduler.timeline().courses.get(idx)?;
        let course_id = course.course_id.clone();
        let name = course.course_name.clone();

        self.scheduler.mark_sign_started(&course_id);
        self.sign_notes.insert(course_id.clone(), SignNote::Pending);
        self.status_text = format!("〔{name}〕{} ...", trigger.describe());
        crate::log::log_event(&format!("sign attempt course={course_id} {name}"));

        Some(Command::Sign { course_id })
    }

    /// 应用后台任务回传的结果。
    pub fn apply(&mut self, result: crate::tasks::TaskResult, now: NaiveDateTime) {
        match result {
            crate::tasks::TaskResult::Refresh(Ok(courses)) => self.apply_refresh_ok(courses, now),
            crate::tasks::TaskResult::Refresh(Err(msg)) => self.apply_refresh_err(&msg, now),
            crate::tasks::TaskResult::Sign(msg) => self.apply_sign(msg, now),
        }
    }

    fn apply_refresh_ok(&mut self, courses: Vec<Course>, now: NaiveDateTime) {
        self.refresh_running = false;
        self.scheduler.on_refresh_ok(&courses, now);

        let total = courses.len();
        self.courses = courses;
        self.last_updated = Some(now.format("%H:%M:%S").to_string());
        self.status_text = if total == 0 {
            format!("刷新成功：当天无课程（{}）", self.date)
        } else {
            format!("刷新成功：{total} 门课（{}）", self.date)
        };
        crate::log::log_event(&format!("refresh ok date={} total={total}", self.date));
    }

    fn apply_refresh_err(&mut self, msg: &str, now: NaiveDateTime) {
        self.refresh_running = false;
        self.scheduler.on_refresh_err(now);
        self.status_text = format!("刷新失败：{msg}（沿用上次课表）");
        crate::log::log_event(&format!("refresh err date={}: {msg}", self.date));
        if !self.scheduler.daily_success(now) {
            self.status_text.push_str("，5 分钟后自动重试");
        }
    }

    fn apply_sign(&mut self, msg: SignTaskResult, now: NaiveDateTime) {
        let course_id = msg.course_id;
        let name = self
            .scheduler
            .timeline()
            .index_of(&course_id)
            .and_then(|idx| self.scheduler.timeline().courses.get(idx))
            .map(|c| c.course_name.clone())
            .unwrap_or_else(|| "未知课程".to_string());

        // 上游文案含“已签到”时视为成功（重复签到场景）
        let outcome = match &msg.result {
            Ok(result) if result.success || result.message.contains("已签到") => {
                SignOutcome::Success
            }
            _ => SignOutcome::Retry,
        };
        self.scheduler.on_sign_result(&course_id, outcome, now);

        match (outcome, msg.result) {
            (SignOutcome::Success, Ok(result)) => {
                let text = if result.success {
                    format!("签到成功（记录 {}）", result.stu_sign_id)
                } else {
                    result.message.clone()
                };
                self.sign_notes
                    .insert(course_id.clone(), SignNote::Success(text.clone()));
                self.status_text = format!("〔{name}〕{text}");
                crate::log::log_event(&format!("sign ok course={course_id}"));
            }
            (SignOutcome::Retry, Ok(result)) => {
                self.sign_notes.insert(
                    course_id.clone(),
                    SignNote::Retrying(result.message.clone()),
                );
                self.status_text =
                    format!("〔{name}〕签到未成功（{}），60 秒后重试", result.message);
                crate::log::log_event(&format!("sign retry course={course_id}"));
            }
            (SignOutcome::Retry, Err(err)) => {
                self.sign_notes
                    .insert(course_id.clone(), SignNote::Retrying(err.clone()));
                self.status_text = format!("〔{name}〕签到失败（{err}），60 秒后重试");
                crate::log::log_event(&format!("sign retry course={course_id}: {err}"));
            }
            (SignOutcome::Success, Err(_)) => unreachable!("Err 不会是 Success"),
        }
    }
}
