//! 纯调度逻辑：决定"什么时候该签到 / 刷新"。
//!
//! 不依赖 tokio 与终端：所有时间由调用方传入，便于手动验证。
//! 主要职责：
//! - 计算下一个唤醒时刻（`next_deadline`）
//! - 到点时给出待执行动作（`take_due`），如窗口开启签到、失败重试、每小时例行刷新
//! - 记录每门课的签到状态（已签 / 在途 / 待重试）
//!
//! 优先级：签到 > 刷新失败重试 > 每小时例行刷新。例行刷新不会重置任何签到状态。

use std::collections::{BTreeMap, HashSet};

use chrono::{Duration, NaiveDate, NaiveDateTime};
use course_signin::model::Course;
use course_signin::schedule::{SIGN_RETRY_INTERVAL, SignWindowHint, Timeline, next_hourly};

/// 课表刷新失败后的自动重试间隔（当天尚未成功时）。
pub const REFRESH_RETRY_INTERVAL: Duration = Duration::minutes(5);

/// 到点待执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// 每小时例行刷新（不重置任何签到状态）。
    RoutineRefresh,
    /// 对某门课发起签到（索引基于当前时间线）。
    Sign { idx: usize, trigger: SignTrigger },
    /// 自动刷新课表。
    Refresh { trigger: RefreshTrigger },
}

/// 签到触发原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignTrigger {
    /// 签到窗口开启（含程序在窗口内启动/刷新的补签）。
    WindowOpen,
    /// 上课开始时的最后一次确认。
    ClassBegin,
    /// 窗口内失败重试。
    Retry,
}

impl SignTrigger {
    /// UI/状态栏文案。
    pub fn describe(self) -> &'static str {
        match self {
            SignTrigger::WindowOpen => "签到窗口开启，尝试签到",
            SignTrigger::ClassBegin => "上课开始，最后确认签到",
            SignTrigger::Retry => "签到未成功，重试",
        }
    }
}

/// 刷新触发原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTrigger {
    /// 刷新失败后的自动重试。
    Retry,
}

impl RefreshTrigger {
    pub fn describe(self) -> &'static str {
        match self {
            RefreshTrigger::Retry => "课表刷新失败，自动重试",
        }
    }
}

/// 一次签到尝试的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignOutcome {
    /// 签到成功（或上游确认已签到）。
    Success,
    /// 失败，窗口内可重试。
    Retry,
}

/// 调度器：持有课表时间线与各类定时任务的状态。
#[derive(Debug)]
pub struct Scheduler {
    timeline: Timeline,
    /// 每门课的签到重试时刻（时间线索引 → 时刻）。
    sign_retry: BTreeMap<usize, NaiveDateTime>,
    /// 已签到成功的课程 id。
    signed: HashSet<String>,
    /// 签到请求进行中的课程 id。
    in_flight: HashSet<String>,
    /// 最近一次成功刷新课表的日期（用于当日失败重试判定）。
    last_success_date: Option<NaiveDate>,
    /// 刷新失败的重试时刻。
    refresh_retry: Option<NaiveDateTime>,
    /// 下一次整点例行刷新时刻。
    next_routine_refresh_at: NaiveDateTime,
}

impl Scheduler {
    /// 以 `now` 为基准初始化。
    pub fn new(now: NaiveDateTime) -> Self {
        Self {
            timeline: Timeline::default(),
            sign_retry: BTreeMap::new(),
            signed: HashSet::new(),
            in_flight: HashSet::new(),
            last_success_date: None,
            refresh_retry: None,
            next_routine_refresh_at: next_hourly(now),
        }
    }

    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// 当天是否已成功刷新过课表（跨天自动失效）。
    pub fn daily_success(&self, now: NaiveDateTime) -> bool {
        self.last_success_date == Some(now.date())
    }

    pub fn is_signed(&self, course_id: &str) -> bool {
        self.signed.contains(course_id)
    }

    /// 签到请求已发出：在结果回来之前不再触发。
    pub fn mark_sign_started(&mut self, course_id: &str) {
        self.in_flight.insert(course_id.to_string());
    }

    /// 课表刷新成功。
    pub fn on_refresh_ok(&mut self, courses: &[Course], now: NaiveDateTime) {
        self.timeline = Timeline::from_courses(courses);
        self.sign_retry.clear();
        // 上游已标记为已签到的课程不再重复签
        self.signed = courses
            .iter()
            .filter(|c| c.sign_status == "1")
            .map(|c| c.id.clone())
            .collect();
        self.last_success_date = Some(now.date());
        self.refresh_retry = None;
    }

    /// 课表刷新失败（保留旧时间线）。
    pub fn on_refresh_err(&mut self, now: NaiveDateTime) {
        if !self.daily_success(now) {
            self.refresh_retry = Some(now + REFRESH_RETRY_INTERVAL);
        }
    }

    /// 一次签到尝试结束。
    pub fn on_sign_result(&mut self, course_id: &str, outcome: SignOutcome, now: NaiveDateTime) {
        self.in_flight.remove(course_id);
        match outcome {
            SignOutcome::Success => {
                self.signed.insert(course_id.to_string());
                if let Some(idx) = self.timeline.index_of(course_id) {
                    self.sign_retry.remove(&idx);
                }
            }
            SignOutcome::Retry => {
                if let Some(idx) = self.timeline.index_of(course_id) {
                    if self
                        .timeline
                        .courses
                        .get(idx)
                        .is_some_and(|c| now <= c.class_end)
                    {
                        self.sign_retry.insert(idx, now + SIGN_RETRY_INTERVAL);
                    }
                }
            }
        }
    }

    /// 当前/最近的签到窗口提示。
    pub fn sign_window_hint(&self, now: NaiveDateTime) -> Option<SignWindowHint> {
        self.timeline.sign_window_hint(now)
    }

    /// 是否有到点的动作；有则清除对应状态并返回。
    ///
    /// 优先级：签到 > 刷新 > 维护（三者时间通常不冲突）。
    pub fn take_due(&mut self, now: NaiveDateTime) -> Option<Due> {
        for (idx, c) in self.timeline.courses.iter().enumerate() {
            if now > c.class_end {
                // 已下课：清掉残留重试
                self.sign_retry.remove(&idx);
                continue;
            }
            if self.signed.contains(&c.course_id) || self.in_flight.contains(&c.course_id) {
                self.sign_retry.remove(&idx);
                continue;
            }
            if let Some(at) = self.sign_retry.get(&idx) {
                if *at <= now {
                    return Some(Due::Sign {
                        idx,
                        trigger: SignTrigger::Retry,
                    });
                }
                continue; // 重试未到点，先不签
            }
            if c.window_contains(now) {
                let trigger = if now >= c.class_begin {
                    SignTrigger::ClassBegin
                } else {
                    SignTrigger::WindowOpen
                };
                return Some(Due::Sign { idx, trigger });
            }
        }

        if self.refresh_retry.is_some_and(|at| at <= now) {
            self.refresh_retry = None;
            return Some(Due::Refresh {
                trigger: RefreshTrigger::Retry,
            });
        }

        // 每小时例行刷新：优先级最低，且不重置任何签到状态
        if now >= self.next_routine_refresh_at {
            self.next_routine_refresh_at = next_hourly(now);
            return Some(Due::RoutineRefresh);
        }

        None
    }

    /// 下一个需要唤醒的时刻（用于休眠）。
    pub fn next_deadline(&self, now: NaiveDateTime) -> NaiveDateTime {
        let mut deadline = self.next_routine_refresh_at;
        for (idx, c) in self.timeline.courses.iter().enumerate() {
            if now > c.class_end {
                continue;
            }
            if self.signed.contains(&c.course_id) || self.in_flight.contains(&c.course_id) {
                continue;
            }
            if let Some(at) = self.sign_retry.get(&idx) {
                deadline = deadline.min(*at);
            } else if now >= c.window_open {
                // 已在窗口内且未签：立刻处理
                return now;
            } else {
                deadline = deadline.min(c.window_open);
            }
        }
        if let Some(at) = self.refresh_retry {
            deadline = deadline.min(at);
        }
        deadline
    }
}
