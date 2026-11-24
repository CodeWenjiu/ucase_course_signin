//! 课表时间线：把当天课表展开为签到窗口，供上层调度器使用。

use chrono::{Duration, NaiveDateTime, Timelike};

use crate::model::Course;
use crate::upstream::SIGN_TIMESTAMP_BUFFER_MS;

/// 签到窗口最早开启：上课前 [`SIGN_WINDOW_MAX_LEAD_MINUTES`] 分钟。
/// 首签尝试随机落在窗口区间内，避免卡点抢签与无谓长时间重试。
pub const SIGN_WINDOW_MAX_LEAD_MINUTES: i64 = 5;
/// 签到窗口最晚开启：上课前 [`SIGN_WINDOW_MIN_LEAD_MINUTES`] 分钟。
pub const SIGN_WINDOW_MIN_LEAD_MINUTES: i64 = 1;
/// 签到窗口内重试间隔。
pub const SIGN_RETRY_INTERVAL: Duration = Duration::seconds(60);

/// 生成随机的签到提前量（1..=5 分钟）。
fn random_lead_minutes() -> i64 {
    use rand::RngExt;
    rand::rng().random_range(SIGN_WINDOW_MIN_LEAD_MINUTES..=SIGN_WINDOW_MAX_LEAD_MINUTES)
}

/// 单个课程的签到时间线。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseTimeline {
    /// 课程原始 ID（courseSchedId）。
    pub course_id: String,
    pub course_name: String,
    /// 签到窗口开启时刻：上课前 1~5 分钟内随机（防卡点抢签）。
    pub window_open: NaiveDateTime,
    /// 上课开始时刻。
    pub class_begin: NaiveDateTime,
    /// 下课时刻。
    pub class_end: NaiveDateTime,
}

impl CourseTimeline {
    /// `now` 是否处于该课的签到窗口内（窗口开启 ~ 下课）。
    pub fn window_contains(&self, now: NaiveDateTime) -> bool {
        self.window_open <= now && now <= self.class_end
    }
}

/// 当前/最近签到窗口的展示信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignWindowHint {
    pub course_name: String,
    /// 窗口开启（上课前 1~5 分钟随机）。
    pub window_open: NaiveDateTime,
    /// 窗口关闭（下课）。
    pub class_end: NaiveDateTime,
}

/// 一天的时间线。
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    /// 按窗口开启时间排序的课程。
    pub courses: Vec<CourseTimeline>,
}

impl Timeline {
    /// 从当天课表构建时间线。时间格式异常的课程会被跳过。
    /// 签到窗口点随机落在上课前 1~5 分钟内。
    pub fn from_courses(courses: &[Course]) -> Self {
        let mut list: Vec<CourseTimeline> = courses
            .iter()
            .filter_map(|c| {
                let class_begin = parse_upstream_time(&c.class_begin_time)?;
                let class_end = parse_upstream_time(&c.class_end_time)?;
                Some(CourseTimeline {
                    course_id: c.id.clone(),
                    course_name: c.course_name.clone(),
                    window_open: class_begin - Duration::minutes(random_lead_minutes()),
                    class_begin,
                    class_end,
                })
            })
            .collect();
        list.sort_by_key(|c| c.window_open);
        Self { courses: list }
    }

    /// 当前/最近的签到窗口：优先当前窗口内，否则最近一个未结束的窗口。
    /// 全部课程已结束返回 `None`。
    pub fn sign_window_hint(&self, now: NaiveDateTime) -> Option<SignWindowHint> {
        self.courses
            .iter()
            .find(|c| now <= c.class_end)
            .map(|c| SignWindowHint {
                course_name: c.course_name.clone(),
                window_open: c.window_open,
                class_end: c.class_end,
            })
    }

    /// 按 `course_id` 查找课程在时间线中的索引。
    pub fn index_of(&self, course_id: &str) -> Option<usize> {
        self.courses.iter().position(|c| c.course_id == course_id)
    }
}

/// 计算下一个整点时刻（严格晚于 `now`，用于每小时例行刷新）。
pub fn next_hourly(now: NaiveDateTime) -> NaiveDateTime {
    let this_hour = now.date().and_hms_opt(now.hour(), 0, 0).expect("合法时刻");
    this_hour + Duration::hours(1)
}

/// 生成签到时间戳（服务器时间 - 缓冲）。
///
/// `server_time_ms` 来自上游 `get_timestamp.do`，减去 [`SIGN_TIMESTAMP_BUFFER_MS`]
/// 抵消两个服务器间的时钟偏差。
pub fn sign_timestamp(server_time_ms: i64) -> i64 {
    server_time_ms - SIGN_TIMESTAMP_BUFFER_MS
}

/// 解析上游时间串 `yyyy-MM-dd HH:mm:ss`。
fn parse_upstream_time(raw: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S").ok()
}
