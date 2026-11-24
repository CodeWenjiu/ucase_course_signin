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

crate_macro::mod_pub!(app, log, scheduler, tasks, ui);

/// 键盘事件线程轮询间隔。
const KEYBOARD_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// 关闭自动调度时的休眠间隔（仅用于定期检查开关状态）。
const PAUSED_SLEEP: Duration = Duration::from_secs(60);

#[derive(Debug, Parser)]
#[command(
    name = "course-signin-tui",
    version,
    about = "UCAS 课程签到 TUI（常驻进程）"
)]
struct Args {
    /// 学号
    #[arg(long)]
    username: Option<String>,

    /// 密码（缺省为 UCAS 默认密码）
    #[arg(long, default_value = DEFAULT_PASSWORD)]
    password: String,
}

/// 单线程 runtime：程序是长睡眠 + 低频异步请求型负载，
/// 多线程 worker（默认 = CPU 核数）只会白白占用线程栈内存。
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // 任何 panic 前先恢复终端（raw mode / alternate screen / 光标）
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    let args = Args::parse();

    let username = args
        .username
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("缺少学号：请用 --username 指定"))?;

    let client = Client::with_credentials_default_base(&username, &args.password)?;
    let date = Local::now().format("%Y%m%d").to_string();

    let (tx, rx) = mpsc::channel(4);
    let mut app = App::new(username, date);
    // 启动即刷新一次，构建当天时间线
    if let Some(cmd) = app.refresh("启动刷新课表") {
        tasks::execute(&client, cmd, &tx);
    }

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
    let result = run(&mut terminal, &mut app, &client, &tx, rx).await;
    ratatui::restore();
    result
}

/// 键盘事件转发线程 → mpsc channel，避免阻塞事件循环。
/// 转发全部事件（按键 + Resize），由主循环分发。
fn spawn_input_thread() -> mpsc::UnboundedReceiver<crossterm::event::Event> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        loop {
            if event::poll(KEYBOARD_POLL_INTERVAL).unwrap_or(false) {
                if let Ok(ev) = event::read() {
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// 主循环：处理到点动作 → 睡到下一个唤醒点（期间被键盘/消息/信号唤醒）→ 重绘。
async fn run(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    client: &Client,
    tx: &mpsc::Sender<tasks::TaskResult>,
    mut rx: mpsc::Receiver<tasks::TaskResult>,
) -> Result<()> {
    let mut events = spawn_input_thread();

    loop {
        let now = Local::now().naive_local();

        // 自动调度：连续处理所有到点动作（签到/刷新/维护）
        if app.auto_refresh {
            while let Some(due) = app.take_due(now) {
                if let Some(cmd) = app.handle_due(due) {
                    tasks::execute(client, cmd, tx);
                }
            }
        }

        let sleep = if app.auto_refresh {
            app.sleep_duration(Local::now().naive_local())
        } else {
            PAUSED_SLEEP
        };

        tokio::select! {
            _ = tokio::time::sleep(sleep) => {
                // 唤醒：重新处理到点动作
            }
            Some(ev) = events.recv() => {
                match ev {
                    Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('r') => {
                            if let Some(cmd) = app.manual_refresh() {
                                tasks::execute(client, cmd, tx);
                            }
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
                    },
                    // 终端尺寸变化：下面的 autoresize + draw 会按新尺寸全量重绘
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
            Some(result) = rx.recv() => {
                app.apply(result, Local::now().naive_local());
            }
            // Ctrl+C 优雅退出（走 terminal 恢复流程）
            _ = tokio::signal::ctrl_c() => {
                return Ok(());
            }
        }

        // 以最新终端尺寸重绘（ioctl 查询，无阻塞；尺寸变化时全量重绘）
        terminal.autoresize()?;
        terminal.draw(|frame| ui::draw(frame, app))?;
    }
}
