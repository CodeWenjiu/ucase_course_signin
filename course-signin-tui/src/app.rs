//! TUI 应用状态：课表数据、刷新任务与状态机。

use chrono::{Local, NaiveDateTime};
use course_signin::model::{Course, SignResult};
use course_signin::schedule::{EventKind, SIGN_RETRY_INTERVAL, Timeline, Wakeup, sign_timestamp};
use course_signin::upstream::Client;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// 课表刷新失败后的自动重试间隔（仅当天尚未成功时）。
const REFRESH_RETRY_INTERVAL: chrono::Duration = chrono::Duration::minutes(5);

/// 后台刷新任务回传的消息。
pub enum RefreshMsg {
    /// 刷新成功，携带最新课表。
    Ok(Vec<Course>),
    /// 刷新失败，携带错误描述。
    Err(String),
    /// 签到任务结果。
    Sign(SignMsg),
}

/// 一次签到尝试的结果。
pub struct SignMsg {
    /// 课程在时间线中的索引（时间线排序后）。
    pub idx: usize,
    /// 课程 id（courseSchedId），注解按它定位，避免排序错位。
    pub course_id: String,
    /// 描述信息。
    pub text: String,
    /// 是否已签到成功。
    pub signed: bool,
    /// 是否仍处于重试阶段（尚未失败放弃）。
    pub retrying: bool,
}

/// 事件驱动调度器当前所处的阶段。
pub enum SchedulerPhase {
    /// 已就绪：等待下一个唤醒点。
    Idle,
    /// 定时唤醒点已到达，正在执行任务。
    Running,
}

impl std::fmt::Display for SchedulerPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchedulerPhase::Idle => write!(f, "休眠中"),
            SchedulerPhase::Running => write!(f, "执行中"),
        }
    }
}

/// TUI 应用状态。
pub struct App {
    /// 显示用学号。
    pub username: String,
    /// 查询日期（`yyyyMMdd`）。
    pub date: String,
    /// 最新课表。
    pub courses: Vec<Course>,
    /// 状态栏文字。
    pub status_text: String,
    /// 最近一次刷新成功时间（`HH:MM:SS`）。
    pub last_updated: Option<String>,
    /// 是否自动刷新。
    pub auto_refresh: bool,
    /// 是否有刷新任务在跑（避免并发刷新）。
    pub refresh_running: bool,
    /// 是否退出。
    pub quit: bool,
    /// 最近一次刷新成功时的课程数（供状态栏展示）。
    /// 最近一次刷新成功时的课程数（供状态栏展示）。
    last_count: usize,
    timeline: Timeline,
    phase: SchedulerPhase,
    next_wakeup: Option<String>,
    /// 当前挂起的唤醒点（已计算、未触发）。
    pending: Option<Wakeup>,
    /// 签到窗口内重试唤醒时刻。
    retry_at: Option<NaiveDateTime>,
    /// 重试对应的课程索引。
    retry_course: Option<usize>,
    /// 课表刷新失败后的重试时刻（仅在当天尚未成功拉取过课表时安排）。
    refresh_retry_at: Option<NaiveDateTime>,
    /// 当天是否已成功拉取过课表。
    daily_success: bool,
    /// 每门课的签到过程注解（键为 courseSchedId）。
    sign_notes: HashMap<String, String>,
    client: Client,
    tx: mpsc::Sender<RefreshMsg>,
}

impl App {
    /// 构建应用状态。
    pub fn new(
        client: Client,
        username: impl Into<String>,
        date: impl Into<String>,
        tx: mpsc::Sender<RefreshMsg>,
    ) -> Self {
        Self {
            username: username.into(),
            date: date.into(),
            courses: Vec::new(),
            status_text: "就绪，按 r 手动刷新".to_string(),
            last_updated: None,
            auto_refresh: true,
            refresh_running: false,
            quit: false,
            last_count: 0,
            timeline: Timeline::default(),
            phase: SchedulerPhase::Idle,
            next_wakeup: None,
            pending: None,
            retry_at: None,
            retry_course: None,
            refresh_retry_at: None,
            daily_success: false,
            sign_notes: HashMap::new(),
            client,
            tx,
        }
    }

    /// 更新日期（如跨日时）。
    pub fn set_date(&mut self, date: impl Into<String>) {
        self.date = date.into();
    }

    /// 记录当前时间线。
    pub fn set_timeline(&mut self, timeline: Timeline) {
        self.timeline = timeline;
    }

    /// 获取当前时间线。
    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// 记录下一个唤醒点。
    pub fn set_next_wakeup(&mut self, at: Option<String>) {
        self.next_wakeup = at;
    }

    /// 计算并保存下一个唤醒点（合并时间线事件、签到重试与刷新重试）；`now` 为本地当前时刻。
    pub fn compute_next_wakeup(&mut self, now: NaiveDateTime) {
        if self.pending.is_none() {
            let timeline_wake = self.timeline.next_wakeup(now);
            let candidate = match (self.retry_at, self.retry_course) {
                (Some(at), Some(idx)) if at < timeline_wake.at => Wakeup {
                    at,
                    kind: EventKind::SignRetry(idx),
                    delay: at.signed_duration_since(now),
                },
                _ => timeline_wake,
            };
            self.pending = match self.refresh_retry_at {
                Some(at) if at < candidate.at => Some(Wakeup {
                    at,
                    kind: EventKind::RefreshRetry,
                    delay: at.signed_duration_since(now),
                }),
                _ => Some(candidate),
            };
        }
        if let Some(w) = &self.pending {
            self.next_wakeup = Some(format!(
                "{} · {}",
                w.at.format("%H:%M:%S"),
                w.kind.describe()
            ));
        }
    }

    /// 唤醒点是否已到时刻（已触发则清除挂起状态）。
    pub fn wakeup_due(&mut self, now: NaiveDateTime) -> Option<Wakeup> {
        let due = self.pending.filter(|w| now >= w.at);
        if due.is_some() {
            self.pending = None;
        }
        due
    }

    /// 当前/最近签到窗口的展示文字（无课时返回 None）。
    pub fn sign_window_text_at(&self, now: NaiveDateTime) -> Option<String> {
        self.timeline.sign_window_hint(now).map(|hint| {
            format!(
                "〔{}〕{} ~ {}",
                hint.course_name,
                hint.window_open.format("%H:%M"),
                hint.class_end.format("%H:%M"),
            )
        })
    }

    /// [`Self::sign_window_text_at`] 的实时版本（当前本地时间）。
    pub fn sign_window_text(&self) -> Option<String> {
        self.sign_window_text_at(Local::now().naive_local())
    }

    /// 当前时间线的下一次唤醒时长（用于 tokio sleep）。
    pub fn sleep_until_next(&self, now: NaiveDateTime) -> std::time::Duration {
        let at = self
            .pending
            .map(|w| w.at)
            .unwrap_or_else(|| self.timeline.next_wakeup(now).at);
        let ms = at.signed_duration_since(now).num_milliseconds();
        if ms > 0 {
            std::time::Duration::from_millis(ms as u64)
        } else {
            std::time::Duration::from_secs(60)
        }
    }

    /// 课程签到过程注解（供表格状态列展示）。
    pub fn sign_note(&self, course_id: &str) -> Option<&str> {
        self.sign_notes.get(course_id).map(String::as_str)
    }

    /// 启动一次签到尝试（后台任务），携带原因文案。
    pub fn spawn_sign_attempt_with(&mut self, idx: usize, reason: String) {
        let Some(course) = self.timeline.courses.get(idx) else {
            return;
        };
        let course_id = course.course_id.clone();
        let client = self.client.clone();
        let tx = self.tx.clone();

        self.phase = SchedulerPhase::Running;
        self.status_text = reason;
        self.sign_notes
            .insert(course.course_id.clone(), "尝试签到中 ...".to_string());
        crate::log::log_event(&format!(
            "sign attempt course={} {}",
            course.course_id, course.course_name
        ));

        tokio::spawn(async move {
            let result = attempt_sign_once(&client, &course_id).await;
            let _ = tx
                .send(RefreshMsg::Sign(SignMsg {
                    idx,
                    course_id,
                    text: result.message.clone(),
                    signed: result.signed,
                    retrying: result.retrying,
                }))
                .await;
        });
    }

    /// 启动一次签到尝试（后台任务）。
    pub fn spawn_sign_attempt(&mut self, idx: usize) {
        let name = self
            .timeline
            .courses
            .get(idx)
            .map(|c| c.course_name.as_str())
            .unwrap_or("未知课程");
        self.spawn_sign_attempt_with(idx, format!("〔{name}〕正在发起签到 ..."));
    }

    /// 处理一个到点的唤醒事件。
    pub fn handle_wakeup(&mut self, wakeup: Wakeup) {
        match wakeup.kind {
            EventKind::Maintenance => {
                self.set_date(Local::now().format("%Y%m%d").to_string());
                // 新的一天：重置“当日已成功”标记，失败会自动重试
                self.daily_success = false;
                self.refresh_retry_at = None;
                self.spawn_refresh_with("每日维护：更新日期，拉取当天课表".to_string());
                crate::log::log_event(&format!("maintenance date={}", self.date));
            }
            EventKind::WindowOpen(idx) => {
                let name = self
                    .timeline
                    .courses
                    .get(idx)
                    .map(|c| c.course_name.as_str())
                    .unwrap_or("未知课程");
                self.spawn_sign_attempt_with(
                    idx,
                    format!("〔{name}〕签到窗口开启（课前 1~5 分钟随机），尝试签到中 ..."),
                );
            }
            EventKind::ClassBegin(idx) => {
                let name = self
                    .timeline
                    .courses
                    .get(idx)
                    .map(|c| c.course_name.as_str())
                    .unwrap_or("未知课程");
                self.spawn_sign_attempt_with(idx, format!("〔{name}〕上课开始，最后确认签到 ..."));
            }
            EventKind::ClassEnd(idx) => {
                let name = self
                    .timeline
                    .courses
                    .get(idx)
                    .map(|c| c.course_name.as_str())
                    .unwrap_or("未知课程");
                if let Some(course) = self.timeline.courses.get(idx) {
                    self.sign_notes.remove(&course.course_id);
                }
                self.retry_at = None;
                self.retry_course = None;
                self.status_text = format!("〔{name}〕下课，该课任务完结");
            }
            EventKind::SignRetry(idx) => {
                self.retry_course = None;
                self.retry_at = None;
                let course = self
                    .timeline
                    .courses
                    .get(idx)
                    .map(|c| c.course_name.as_str())
                    .unwrap_or("未知课程");
                self.status_text = format!("〔{course}〕签到未成功，60 秒后重试 ...");
                self.spawn_sign_attempt(idx);
            }
            EventKind::RefreshRetry => {
                self.refresh_retry_at = None;
                self.spawn_refresh_with("课表刷新失败自动重试 ...".to_string());
            }
        }
    }

    /// 下一个唤醒点描述（供 UI 展示）。
    pub fn next_wakeup(&self) -> Option<&str> {
        self.next_wakeup.as_deref()
    }

    /// 当前调度阶段。
    pub fn phase(&self) -> &SchedulerPhase {
        &self.phase
    }

    /// 启动一次后台刷新（已在运行时忽略）。
    pub fn spawn_refresh(&mut self) {
        self.spawn_refresh_with("正在刷新课表 ...".to_string());
    }

    /// 带着原因启动后台刷新（原因会显示在状态栏）。
    /// 刷新前强制同步日期为当前本地日期，避免跨天后查询旧日期。
    pub fn spawn_refresh_with(&mut self, reason: String) {
        if self.refresh_running {
            return;
        }
        self.date = Local::now().format("%Y%m%d").to_string();
        self.refresh_running = true;
        self.phase = SchedulerPhase::Running;
        self.status_text = reason;

        let client = self.client.clone();
        let date = self.date.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let result: Result<Vec<Course>, String> = async {
                let session = client.login().await.map_err(|e| format!("{e:#}"))?;
                client
                    .get_schedule(&session, &date)
                    .await
                    .map_err(|e| format!("{e:#}"))
            }
            .await;

            let _ = tx
                .send(match result {
                    Ok(courses) => RefreshMsg::Ok(courses),
                    Err(msg) => RefreshMsg::Err(msg),
                })
                .await;
        });
    }

    /// 手动刷新后重置挂起唤醒点（事件驱动循环会重新计算）。
    pub fn invalidate_wakeup(&mut self) {
        self.pending = None;
    }

    /// 应用后台任务回传的结果。
    pub fn apply(&mut self, msg: RefreshMsg) {
        self.refresh_running = false;
        match msg {
            RefreshMsg::Ok(courses) => {
                let total = courses.len();
                self.last_count = total;
                self.courses = courses;
                self.timeline = Timeline::from_courses(&self.courses);
                self.pending = None; // 课程变化 → 重新计算唤醒点
                self.daily_success = true;
                self.refresh_retry_at = None;
                self.last_updated = Some(Local::now().format("%H:%M:%S").to_string());
                self.status_text = format!("刷新成功：{total} 门课（{date}）", date = self.date);
                crate::log::log_event(&format!("refresh ok date={} total={}", self.date, total));
            }
            RefreshMsg::Err(msg) => {
                self.status_text = format!("刷新失败：{msg}（沿用上次课表）");
                crate::log::log_event(&format!("refresh err date={}: {msg}", self.date));
                // 当天尚未成功时，安排 5 分钟后的自动重试（覆盖凌晨接口未就绪/瞬时故障）
                if !self.daily_success {
                    self.refresh_retry_at =
                        Some(Local::now().naive_local() + REFRESH_RETRY_INTERVAL);
                    self.status_text.push_str("，5 分钟后自动重试");
                }
            }
            RefreshMsg::Sign(msg) => self.apply_sign(msg),
        }
    }

    /// 落地一次签到尝试的结果。
    fn apply_sign(&mut self, msg: SignMsg) {
        let course_name = self
            .timeline
            .courses
            .get(msg.idx)
            .map(|c| c.course_name.clone())
            .unwrap_or_else(|| format!("课程 {}", msg.idx + 1));

        let now = Local::now().naive_local();
        let still_open = self
            .timeline
            .courses
            .get(msg.idx)
            .is_some_and(|c| now <= c.class_end);

        if msg.signed {
            self.sign_notes
                .insert(msg.course_id.clone(), format!("✓ {}", msg.text));
            self.clear_retry();
            self.status_text = format!("〔{course_name}〕{}", msg.text);
            crate::log::log_event(&format!("sign ok course={}", msg.course_id));
        } else if still_open && msg.retrying {
            self.sign_notes
                .insert(msg.course_id.clone(), format!("重试中：{}", msg.text));
            self.retry_at = Some(now + SIGN_RETRY_INTERVAL);
            self.retry_course = Some(msg.idx);
            self.status_text = format!("〔{course_name}〕签到未成功（{}），60s 后重试", msg.text);
            crate::log::log_event(&format!("sign retry course={}", msg.course_id));
        } else {
            self.sign_notes
                .insert(msg.course_id.clone(), format!("✗ {}", msg.text));
            self.clear_retry();
            self.status_text = format!("〔{course_name}〕签到失败：{}", msg.text);
            crate::log::log_event(&format!("sign fail course={}", msg.course_id));
        }
    }

    fn clear_retry(&mut self) {
        self.retry_at = None;
        self.retry_course = None;
    }
}

/// 一次完整的签到尝试：登录 → 服务器时间校准 → 直接签到。
async fn attempt_sign_once(client: &Client, course_id: &str) -> SignAttemptResult {
    let result: Result<SignResult, String> = async {
        let session = client.login().await.map_err(|e| format!("{e:#}"))?;
        let server_ts = client
            .server_timestamp()
            .await
            .map_err(|e| format!("{e:#}"))?;
        let ts = sign_timestamp(server_ts);
        client
            .scan_sign(&session, course_id, ts)
            .await
            .map_err(|e| format!("{e:#}"))
    }
    .await;

    match result {
        Ok(s) if s.success => SignAttemptResult {
            message: format!("签到成功（记录 {}", s.stu_sign_id),
            signed: true,
            retrying: false,
        },
        Ok(s) => SignAttemptResult {
            message: s.message,
            signed: false,
            retrying: true,
        },
        Err(e) => SignAttemptResult {
            message: e,
            signed: false,
            retrying: true,
        },
    }
}

struct SignAttemptResult {
    message: String,
    signed: bool,
    retrying: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use course_signin::model::Course;

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

    fn make_app() -> (App, mpsc::Sender<RefreshMsg>) {
        let (tx, _rx) = mpsc::channel(4);
        // 无需真实 HTTP 客户端：测试只走 apply 路径
        let client = Client::with_credentials_default_base("u", "p").expect("构建客户端失败");
        let app = App::new(client, "20250001", "20260921", tx.clone());
        (app, tx)
    }

    #[test]
    fn apply_success_updates_courses() {
        let (mut app, _) = make_app();
        app.apply(RefreshMsg::Ok(vec![sample()]));
        assert_eq!(app.courses.len(), 1);
        assert!(!app.refresh_running);
        assert!(app.last_updated.is_some());
        assert!(app.status_text.contains("1 门课"));
    }

    #[test]
    fn apply_error_keeps_courses_and_reports() {
        let (mut app, _) = make_app();
        app.apply(RefreshMsg::Ok(vec![sample()]));
        app.apply(RefreshMsg::Err("登录失败".into()));
        assert_eq!(app.courses.len(), 1, "失败不应清空已有课表");
        assert!(app.status_text.contains("登录失败"));
        assert!(!app.refresh_running);
    }

    #[test]
    fn apply_ok_builds_timeline() {
        let (mut app, _tx) = make_app();
        app.apply(RefreshMsg::Ok(vec![sample()]));
        assert_eq!(app.courses.len(), 1);
        assert_eq!(app.timeline().courses.len(), 1, "课表应构建时间线");
    }

    #[tokio::test]
    async fn wakeup_computes_and_fires_at_window_open() {
        let (mut app, _) = make_app();
        app.apply(RefreshMsg::Ok(vec![sample()])); // 18:30 的课，窗口 18:00

        // 17:00 时下一个唤醒点是窗口开（18:25~18:29 随机）
        let now = chrono::NaiveDateTime::parse_from_str("2026-09-21 17:00:00", "%Y-%m-%d %H:%M:%S")
            .expect("合法时间");
        app.compute_next_wakeup(now);
        assert!(
            app.next_wakeup().expect("应有唤醒点").contains("18:2"),
            "窗口应落在 18:25~18:29"
        );
        assert!(app.wakeup_due(now).is_none(), "未到点不应触发");

        // 到 18:35（窗口已开后）触发窗口开启事件
        let due = app
            .wakeup_due(now + chrono::Duration::minutes(95))
            .expect("到点应触发");
        app.handle_wakeup(due);
        assert!(app.status_text.contains("签到窗口开启"));
        assert!(app.pending.is_none(), "触发后挂起点应清空");
    }

    #[tokio::test]
    async fn maintenance_updates_date_and_starts_refresh() {
        let (mut app, _) = make_app();
        assert_eq!(app.date, "20260921");
        let wake = course_signin::schedule::Wakeup {
            at: chrono::NaiveDateTime::parse_from_str("2026-09-22 00:05:00", "%Y-%m-%d %H:%M:%S")
                .expect("合法时间"),
            kind: course_signin::schedule::EventKind::Maintenance,
            delay: chrono::Duration::zero(),
        };
        app.handle_wakeup(wake);
        assert_eq!(
            app.date,
            chrono::Local::now().format("%Y%m%d").to_string(),
            "日期应更新为当天"
        );
    }

    #[test]
    fn refresh_error_schedules_retry_when_no_success_yet() {
        let (mut app, _) = make_app();
        app.apply(RefreshMsg::Err("上游超时".into()));
        assert!(
            app.refresh_retry_at.is_some(),
            "当日尚未成功时失败应安排重试"
        );
        assert!(app.status_text.contains("自动重试"));
    }

    #[test]
    fn refresh_error_keeps_courses_and_no_retry_after_success() {
        let (mut app, _) = make_app();
        app.apply(RefreshMsg::Ok(vec![sample()]));
        app.apply(RefreshMsg::Err("又一次失败".into()));
        assert_eq!(app.courses.len(), 1, "失败保留已有课表");
        assert!(app.refresh_retry_at.is_none(), "已成功过则不再安排重试");
        assert!(app.status_text.contains("沿用上次课表"));
    }

    #[test]
    fn refresh_success_clears_retry_and_sets_daily_flag() {
        let (mut app, _) = make_app();
        app.apply(RefreshMsg::Err("超时".into()));
        assert!(app.refresh_retry_at.is_some());

        app.apply(RefreshMsg::Ok(vec![sample()]));
        assert!(app.refresh_retry_at.is_none());
        assert!(app.daily_success);
        assert!(app.status_text.contains("刷新成功"));
    }
}
