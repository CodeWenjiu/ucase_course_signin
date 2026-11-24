//! 终端渲染：标题栏、课表表格、状态栏。

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, Cell, Paragraph, Row, Table, Wrap},
};

use crate::app::{App, SignNote};

/// 顶栏高度 / 状态栏高度。
const HEADER_HEIGHT: u16 = 4;
const FOOTER_HEIGHT: u16 = 4;

/// 渲染整个界面。
pub fn draw(frame: &mut Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(HEADER_HEIGHT),
        Constraint::Min(0),
        Constraint::Length(FOOTER_HEIGHT),
    ])
    .areas(frame.area());

    draw_header(frame, header, app);
    draw_body(frame, body, app);
    draw_footer(frame, footer, app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let auto = if app.auto_refresh { "开" } else { "关" };
    let updated = app.last_updated.as_deref().unwrap_or("--:--:--");

    let title = Paragraph::new("UCAS 课程签到")
        .bold()
        .block(Block::bordered().title("Dashboard"))
        .style(Style::default().fg(Color::Cyan));

    let info_lines = vec![
        Line::from(format!(
            "学号: {} | 日期: {} | 自动调度: {} | 最近更新: {}",
            app.username, app.date, auto, updated
        )),
        Line::from(format!("下次唤醒: {}", app.next_wakeup_text())),
        match app.sign_window_text() {
            Some(w) => Line::from(format!("签到窗口: {w}")),
            None => Line::from("签到窗口: --（当天无课程）").fg(Color::DarkGray),
        },
    ];
    let info = Paragraph::new(info_lines).wrap(Wrap { trim: false });

    let [lhs, rhs] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).areas(area);
    frame.render_widget(title, lhs);
    frame.render_widget(info, rhs);
}

fn draw_body(frame: &mut Frame, area: Rect, app: &App) {
    if app.courses.is_empty() {
        let hint = if app.refresh_running {
            "正在加载课表 ..."
        } else {
            "暂无课程（按 r 刷新）"
        };
        let paragraph = Paragraph::new(hint)
            .centered()
            .block(Block::bordered().title(format!("课表（{}）", app.date)));
        frame.render_widget(paragraph, area);
        return;
    }

    let header = Row::new(vec!["#", "课程", "上课时间", "教师", "签到状态", "ID"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = app
        .courses
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (status_label, status_color) = status_cell(app.sign_note(&c.id), c);
            Row::new(vec![
                Cell::from((i + 1).to_string()),
                Cell::from(c.course_name.as_str()),
                Cell::from(c.class_time_range()),
                Cell::from(c.teacher_name.as_str()),
                Cell::from(status_label).fg(status_color),
                Cell::from(c.id.as_str()),
            ])
        })
        .collect::<Vec<_>>();

    let widths = [
        Constraint::Length(5),
        Constraint::Fill(3),
        Constraint::Length(16),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title(format!("课表（{}）", app.date)))
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    frame.render_widget(table, area);
}

/// 签到状态列：本地签到注记优先于上游原始状态。
fn status_cell(note: Option<&SignNote>, course: &course_signin::model::Course) -> (String, Color) {
    match note {
        Some(SignNote::Success(text)) => (text.clone(), Color::Green),
        Some(SignNote::Pending) => ("签到中 ...".to_string(), Color::Yellow),
        Some(SignNote::Retrying(text)) => (format!("重试中：{text}"), Color::Yellow),
        Some(SignNote::Failed(text)) => (format!("✗ {text}"), Color::Red),
        None => (
            course.sign_status_label(),
            match course.sign_status.as_str() {
                "1" => Color::Green,
                "0" => Color::Red,
                _ => Color::Yellow,
            },
        ),
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let status_style = if app.refresh_running {
        Color::Yellow
    } else {
        Color::White
    };

    let footer = Paragraph::new(vec![
        Line::from(app.status_text.clone()).fg(status_style),
        Line::from("q: 退出 | r: 手动刷新 | a: 自动刷新开关")
            .alignment(Alignment::Right)
            .fg(Color::DarkGray),
    ])
    .block(Block::bordered().title("状态"));

    frame.render_widget(footer, area);
}

