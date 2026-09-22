//! UCAS 课程签到 TUI：常驻进程，按日程事件驱动休眠/唤醒。

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::Local;
use clap::Parser;
use course_signin::config::DEFAULT_PASSWORD;
use course_signin::upstream::Client;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::app::App;

crate_macro::mod_pub!(app, ui);

/// 键盘事件线程轮询间隔。
const KEYBOARD_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Parser)]
#[command(
    name = "course-signin-tui",
    version,
    about = "UCAS 课程签到 TUI（常驻进程）"
)]
struct Args {
    /// 学号（或环境变量 UCAS_USERNAME）
    #[arg(long, env = "UCAS_USERNAME")]
    username: Option<String>,

    /// 密码（或环境变量 UCAS_PASSWORD；缺省为默认密码）
    #[arg(long, env = "UCAS_PASSWORD", default_value = DEFAULT_PASSWORD)]
    password: String,
}

/// 单线程 runtime：程序是长睡眠 + 低频异步请求型负载，
/// 多线程 worker（默认 = CPU 核数）只会白白占用线程栈内存。
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();

    let username = args
        .username
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("缺少学号：请用 --username 或环境变量 UCAS_USERNAME"))?;

    let client = Client::with_credentials_default_base(&username, &args.password)?;
    let date = Local::now().format("%Y%m%d").to_string();

    let (tx, rx) = mpsc::channel(4);
    let mut app = App::new(client, username, date, tx);
    // 启动即刷新一次，构建当天时间线
    app.spawn_refresh();

    let mut terminal = ratatui::init();
    // 启动时全量清屏一次，避免残留启动前的终端内容（之后走 diff 增量渲染）。
    // 不用 Terminal::clear()：它会先查询并等待终端光标位置应答，可能阻塞。
    crossterm::execute!(
        std::io::stdout(),
        crossterm::cursor::MoveTo(0, 0),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
    )
    .context("启动清屏失败")?;
    // 先渲染一帧（如“正在刷新课表”），避免启动时黑屏等待
    terminal.draw(|frame| ui::draw(frame, &app))?;
    let result = run(&mut terminal, &mut app, rx).await;
    ratatui::restore();
    result
}

/// 键盘事件转发线程 → mpsc channel，避免阻塞事件循环。
fn spawn_keyboard_thread() -> mpsc::UnboundedReceiver<crossterm::event::KeyEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        loop {
            if event::poll(KEYBOARD_POLL_INTERVAL).unwrap_or(false) {
                if let Ok(Event::Key(key)) = event::read() {
                    if key.kind == KeyEventKind::Press && tx.send(key).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// 主循环：事件驱动的休眠（定时唤醒点 / 键盘 / 后台消息三路竞争）+ 渲染。
async fn run(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    mut rx: mpsc::Receiver<app::RefreshMsg>,
) -> Result<()> {
    let mut keys = spawn_keyboard_thread();

    loop {
        let now = Local::now().naive_local();

        // 事件驱动：计算/更新下一个唤醒点，若有到点事件则执行
        if app.auto_refresh {
            app.compute_next_wakeup(now);
            if let Some(wakeup) = app.wakeup_due(now) {
                app.handle_wakeup(wakeup);
            }
        } else {
            app.invalidate_wakeup();
        }

        // 睡到下一个唤醒点；期间键盘与后台消息可随时打断
        let sleep_time = app.sleep_until_next(Local::now().naive_local());

        tokio::select! {
            _ = tokio::time::sleep(sleep_time) => {
                // 唤醒：重新计算（包括跨天维护事件）
            }
            Some(key) = keys.recv() => {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('r') => {
                        app.invalidate_wakeup();
                        app.spawn_refresh();
                    }
                    KeyCode::Char('a') => {
                        app.auto_refresh = !app.auto_refresh;
                        app.status_text = if app.auto_refresh {
                            "自动调度已开启".to_string()
                        } else {
                            "自动调度已暂停（按 a 恢复）".to_string()
                        };
                    }
                    _ => {}
                }
            }
            Some(msg) = rx.recv() => {
                // 后台任务结果（刷新/签到）到达：立刻落地并重绘
                app.apply(msg);
            }
        }

        // 渲染
        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.quit {
            return Ok(());
        }
    }
}
