//! 日程调度：把课表展开为唤醒时间线，供常驻进程按事件驱动休眠/唤醒。

use chrono::{Duration, NaiveDateTime};

use crate::model::Course;
use crate::upstream::SIGN_TIMESTAMP_BUFFER_MS;

/// 签到窗口最早开启：上课前 [`SIGN_WINDOW_MAX_LEAD_MINUTES`] 分钟。
/// 首签尝试随机落在窗口区间内，避免卡点抢签与无谓长时间重试。
pub const SIGN_WINDOW_MAX_LEAD_MINUTES: i64 = 5;
/// 签到窗口最晚开启：上课前 [`SIGN_WINDOW_MIN_LEAD_MINUTES`] 分钟。
pub const SIGN_WINDOW_MIN_LEAD_MINUTES: i64 = 1;
/// 签到窗口内重试间隔。
pub const SIGN_RETRY_INTERVAL: Duration = Duration::seconds(60);
/// 每日维护时刻（跨天刷新课表），凌晨 00:05。
pub const MAINTENANCE_HOUR: u32 = 0;
pub const MAINTENANCE_MINUTE: u32 = 5;

/// 生成随机的签到提前量（1..=5 分钟）。
fn random_lead_minutes() -> i64 {
    use rand::Rng;
    rand::thread_rng().gen_range(SIGN_WINDOW_MIN_LEAD_MINUTES..=SIGN_WINDOW_MAX_LEAD_MINUTES)
}

/// 单个课程签到任务的完整时间线。
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

/// 签到窗口提示（供 UI 展示当前/最近的窗口时段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignWindowHint {
    pub course_name: String,
    /// 窗口开启（上课前 30 分钟）。
    pub window_open: NaiveDateTime,
    /// 窗口关闭（下课）。
    pub class_end: NaiveDateTime,
}

/// 一天的时间线。
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    /// 按窗口开启时间排序的课程日程。
    pub courses: Vec<CourseTimeline>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// 每日维护：跨天拉当天课表。
    Maintenance,
    /// 某课程签到窗口开启。
    WindowOpen(usize),
    /// 某课程开始（最后确认签到）。
    ClassBegin(usize),
    /// 某课程结束（完结任务）。
    ClassEnd(usize),
    /// 签到窗口内的重试（签到失败后 60s 再试）。
    SignRetry(usize),
    /// 课表刷新失败后的自动重试。
    RefreshRetry,
}

/// 下一个需要唤醒的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wakeup {
    /// 事件发生时刻。
    pub at: NaiveDateTime,
    /// 事件类型。
    pub kind: EventKind,
    /// 距离现在多久。
    pub delay: Duration,
}

impl EventKind {
    /// 人类可读描述（供 UI 展示）。
    pub fn describe(&self) -> String {
        match self {
            EventKind::Maintenance => "每日维护：拉取当天课表".to_string(),
            EventKind::WindowOpen(idx) => format!("课程 {} 签到窗口开启", idx + 1),
            EventKind::ClassBegin(idx) => format!("课程 {} 开始", idx + 1),
            EventKind::ClassEnd(idx) => format!("课程 {} 结束", idx + 1),
            EventKind::SignRetry(idx) => format!("课程 {} 签到重试", idx + 1),
            EventKind::RefreshRetry => "课表刷新重试".to_string(),
        }
    }
}

impl Timeline {
    /// 从当天课表构建时间线。所有时间解析失败（格式异常）的课程会被跳过。
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

    /// 计算从 `now` 起的下一个唤醒事件。
    ///
    /// - 有未完结课程时：返回最近的事件点（窗口开 / 课开始 / 课结束）。
    /// - 全部课程完结后：睡到下一个每日维护点。
    pub fn next_wakeup(&self, now: NaiveDateTime) -> Wakeup {
        // 每日维护点：下一个 00:05
        let maintenance = next_maintenance(now);
        let mut best = Wakeup {
            at: maintenance,
            kind: EventKind::Maintenance,
            delay: maintenance.signed_duration_since(now),
        };

        for (idx, c) in self.courses.iter().enumerate() {
            for (kind, at) in [
                (EventKind::WindowOpen(idx), c.window_open),
                (EventKind::ClassBegin(idx), c.class_begin),
                (EventKind::ClassEnd(idx), c.class_end),
            ] {
                if at <= now {
                    continue;
                }
                if at < best.at {
                    best = Wakeup {
                        at,
                        kind,
                        delay: at.signed_duration_since(now),
                    };
                }
            }
        }

        best
    }

    /// 当前 `now` 是否处于某门课的签到窗口内。
    pub fn active_sign_window(&self, now: NaiveDateTime) -> Option<usize> {
        self.courses
            .iter()
            .position(|c| c.window_open <= now && now <= c.class_end)
    }

    /// 当前/最近的签到窗口提示：优先窗口内，否则最近的未来窗口。
    /// 全部课程已完结返回 `None`。
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
}

/// 计算下一个每日维护时刻（当天若已过 00:05 则为次日 00:05）。
fn next_maintenance(now: NaiveDateTime) -> NaiveDateTime {
    let today = now.date();
    let today_maintenance = today
        .and_hms_opt(MAINTENANCE_HOUR, MAINTENANCE_MINUTE, 0)
        .expect("合法时刻");
    if now < today_maintenance {
        today_maintenance
    } else {
        today_maintenance + Duration::days(1)
    }
}

/// 解析上游时间串 `yyyy-MM-dd HH:mm:ss`。
fn parse_upstream_time(raw: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S").ok()
}

/// 生成当前签到时间戳（服务器时间 - 缓冲）。
///
/// `server_time_ms` 来自上游 `get_timestamp.do`，减去 [`SIGN_TIMESTAMP_BUFFER_MS`]
/// 抵消两个服务器间的时钟偏差。
pub fn sign_timestamp(server_time_ms: i64) -> i64 {
    server_time_ms - SIGN_TIMESTAMP_BUFFER_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").expect("合法时间")
    }

    fn course(id: &str, begin: &str, end: &str) -> Course {
        Course {
            id: id.into(),
            uuid: "UUID".into(),
            course_name: format!("课程{id}"),
            teacher_name: "老师".into(),
            week_day: "周一".into(),
            class_begin_time: begin.into(),
            class_end_time: end.into(),
            sign_status: "0".into(),
        }
    }

    #[test]
    fn builds_timeline_with_window_open() {
        let timeline =
            Timeline::from_courses(&[course("1", "2026-09-21 18:30:00", "2026-09-21 21:00:00")]);
        assert_eq!(timeline.courses.len(), 1);
        let c = &timeline.courses[0];
        let begin = dt("2026-09-21 18:30:00");
        // 窗口在课前 1~5 分钟随机
        assert!(c.window_open >= begin - Duration::minutes(SIGN_WINDOW_MAX_LEAD_MINUTES));
        assert!(c.window_open <= begin - Duration::minutes(SIGN_WINDOW_MIN_LEAD_MINUTES));
        assert_eq!(c.class_begin, begin);
        assert_eq!(c.class_end, dt("2026-09-21 21:00:00"));
    }

    #[test]
    fn skips_courses_with_bad_time() {
        let mut bad = course("1", "2026-09-21 18:30:00", "2026-09-21 21:00:00");
        bad.class_begin_time = "格式错误".into();
        let timeline = Timeline::from_courses(&[bad]);
        assert!(timeline.courses.is_empty());
    }

    #[test]
    fn next_wakeup_finds_earliest_event() {
        let timeline = Timeline::from_courses(&[
            course("1", "2026-09-21 18:30:00", "2026-09-21 21:00:00"),
            course("2", "2026-09-21 10:00:00", "2026-09-21 12:00:00"),
        ]);
        // 09:00 时，最近应是课程 2 的窗口开（10:00 前 1~5 分钟随机）
        let wake = timeline.next_wakeup(dt("2026-09-21 09:00:00"));
        let begin2 = dt("2026-09-21 10:00:00");
        assert!(
            wake.at >= begin2 - Duration::minutes(SIGN_WINDOW_MAX_LEAD_MINUTES)
                && wake.at <= begin2 - Duration::minutes(SIGN_WINDOW_MIN_LEAD_MINUTES),
            "窗口应在课前 1~5 分钟，实际 {}",
            wake.at
        );
        assert!(wake.delay >= Duration::minutes(55) && wake.delay <= Duration::minutes(59));
    }

    #[test]
    fn next_wakeup_skips_past_events() {
        let timeline =
            Timeline::from_courses(&[course("1", "2026-09-21 18:30:00", "2026-09-21 21:00:00")]);
        // 19:00 时课程已开始：最近的应是下课 21:00
        let wake = timeline.next_wakeup(dt("2026-09-21 19:00:00"));
        assert_eq!(wake.at, dt("2026-09-21 21:00:00"));
    }

    #[test]
    fn next_wakeup_sleeps_until_maintenance_when_all_done() {
        let timeline =
            Timeline::from_courses(&[course("1", "2026-09-21 18:30:00", "2026-09-21 21:00:00")]);
        // 22:00 全部完结：睡到次日 00:05
        let wake = timeline.next_wakeup(dt("2026-09-21 22:00:00"));
        assert_eq!(wake.at, dt("2026-09-22 00:05:00"));
    }

    #[test]
    fn next_maintenance_handles_same_day_and_next_day() {
        assert_eq!(
            next_maintenance(dt("2026-09-21 00:01:00")),
            dt("2026-09-21 00:05:00")
        );
        assert_eq!(
            next_maintenance(dt("2026-09-21 12:00:00")),
            dt("2026-09-22 00:05:00")
        );
    }

    #[test]
    fn active_sign_window_detects_only_current_course() {
        let timeline = Timeline::from_courses(&[
            course("1", "2026-09-21 18:30:00", "2026-09-21 21:00:00"),
            course("2", "2026-09-21 10:00:00", "2026-09-21 12:00:00"),
        ]);
        // 排序后早课（10:00）在索引 0，晚课（18:30）在索引 1。
        // 18:40 处于晚课的签到窗口内；12:30（两窗口都不覆盖）应返回 None
        assert_eq!(
            timeline.active_sign_window(dt("2026-09-21 18:40:00")),
            Some(1)
        );
        assert_eq!(timeline.active_sign_window(dt("2026-09-21 12:30:00")), None);
    }

    #[test]
    fn sign_window_hint_reports_current_or_next() {
        let timeline = Timeline::from_courses(&[
            course("1", "2026-09-21 18:30:00", "2026-09-21 21:00:00"),
            course("2", "2026-09-21 10:00:00", "2026-09-21 12:00:00"),
        ]);

        // 窗口内：报告当前课程窗口
        let hint = timeline
            .sign_window_hint(dt("2026-09-21 18:40:00"))
            .expect("应命中窗口");
        assert_eq!(hint.course_name, "课程1");
        // 窗口开在 18:25~18:29 随机（课前 1~5 分钟）
        assert!(hint.window_open >= dt("2026-09-21 18:25:00"));
        assert!(hint.window_open <= dt("2026-09-21 18:29:00"));
        assert_eq!(hint.class_end, dt("2026-09-21 21:00:00"));

        // 两节课之间：报告下一门课的窗口
        let hint = timeline
            .sign_window_hint(dt("2026-09-21 12:30:00"))
            .expect("应报告下一窗口");
        let begin1 = dt("2026-09-21 18:30:00");
        assert!(hint.window_open >= begin1 - Duration::minutes(SIGN_WINDOW_MAX_LEAD_MINUTES));
        assert!(hint.window_open <= begin1 - Duration::minutes(SIGN_WINDOW_MIN_LEAD_MINUTES));

        // 全部完结：None
        assert!(
            timeline
                .sign_window_hint(dt("2026-09-21 22:00:00"))
                .is_none()
        );
    }

    #[test]
    fn sign_timestamp_applies_buffer() {
        assert_eq!(
            sign_timestamp(1_000_000_000),
            1_000_000_000 - SIGN_TIMESTAMP_BUFFER_MS
        );
    }

    #[test]
    fn parses_upstream_time_format() {
        assert_eq!(
            parse_upstream_time("2026-03-25 10:25:00"),
            Some(dt("2026-03-25 10:25:00"))
        );
        assert_eq!(parse_upstream_time("2026/03/25 10:25:00"), None);
    }
}
