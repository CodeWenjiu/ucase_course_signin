//! 终端渲染：标题栏、课表表格、状态栏。

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, Cell, Paragraph, Row, Table, Wrap},
};

use crate::app::App;

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
    let next_wakeup = app.next_wakeup().unwrap_or("计算中 ...");

    let title = Paragraph::new("UCAS 课程签到")
        .bold()
        .block(Block::bordered().title("Dashboard"))
        .style(Style::default().fg(Color::Cyan));

    let info_lines = vec![
        Line::from(format!(
            "学号: {} | 日期: {} | 自动调度: {} | 最近更新: {}",
            app.username, app.date, auto, updated
        )),
        Line::from(format!("下次唤醒: {next_wakeup}")),
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
            // 签到过程注解优先于上游原始状态
            let (status_label, status_color) = match app.sign_note(i) {
                Some(note) if note.contains('✓') => (note.to_string(), Color::Green),
                Some(note) if note.contains("重试") || note.contains("尝试") => {
                    (note.to_string(), Color::Yellow)
                }
                Some(note) => (note.to_string(), Color::Red),
                None => (c.sign_status_label(), {
                    match c.sign_status.as_str() {
                        "1" => Color::Green,
                        "0" => Color::Red,
                        _ => Color::Yellow,
                    }
                }),
            };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, RefreshMsg};
    use course_signin::model::Course;
    use course_signin::upstream::Client;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn sample_course() -> Course {
        Course {
            id: "1222807".into(),
            uuid: "CADD27F17ACC44EDAF00000000000001".into(),
            course_name: "面向对象程序设计—C++".into(),
            teacher_name: "刘立祥".into(),
            week_day: "周一".into(),
            // 2099 年：保证测试跑在任何真实时刻都未完结，签到窗口提示稳定
            class_begin_time: "2099-01-01 18:30:00".into(),
            class_end_time: "2099-01-01 21:00:00".into(),
            sign_status: "1".into(),
        }
    }

    fn render(app: &App) -> String {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).expect("构建测试终端");
        terminal.draw(|frame| draw(frame, app)).expect("渲染失败");
        let buffer = terminal.backend().buffer().clone();
        // CJK 字符占 2 个 cell，宽字符后跟空格占位 cell，拼行时去掉便于断言
        buffer_lines(&buffer).join("\n").replace(' ', "")
    }

    fn buffer_lines(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area();
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn make_app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let client = Client::with_credentials_default_base("u", "p").expect("构建客户端失败");
        App::new(client, "20250001", "20260921", tx)
    }

    #[test]
    fn renders_courses_table() {
        let mut app = make_app();
        app.apply(RefreshMsg::Ok(vec![sample_course()]));

        let text = render(&app);
        assert!(text.contains("面向对象程序设计—C++"), "课程名应渲染");
        assert!(text.contains("18:30~21:00"));
        assert!(text.contains("已签到"));
        assert!(text.contains("签到窗口"), "顶栏应显示签到窗口");
        assert!(text.contains("~21:00"), "应显示窗口截止时段");
    }

    #[test]
    fn footer_merges_status_and_keys() {
        let mut app = make_app();
        app.apply(RefreshMsg::Ok(vec![sample_course()]));

        let text = render(&app);
        assert!(text.contains("刷新成功：1门课"), "状态文字应渲染");
        assert!(text.contains("q:退出"), "键位提示应渲染");
        assert!(text.contains("a:自动刷新开关"), "键位提示应渲染");
    }

    #[test]
    fn empty_courses_shows_hint() {
        let app = make_app();

        let text = render(&app);
        assert!(text.contains("暂无课程") || text.contains("正在加载课表"));
        assert!(text.contains("当天无课程"));
    }
}
