# 列出所有可用的 recipe
_default:
    @just --list

# 运行程序
run:
    @cargo run -p course-signin

# 运行 CLI（参数透传，示例: just cli query --username 2025xxx）
cli *ARGS:
    @cargo run -p course-signin-cli -- {{ARGS}}

# 运行 TUI 常驻进程（示例: just tui --username 2025xxx）
tui *ARGS:
    @cargo run --release -p course-signin-tui -- {{ARGS}}

# 检查代码能否通过编译（不产出可执行文件）
check:
    @cargo check --workspace

# 格式化代码
fmt:
    @cargo fmt --all

# 运行测试
test:
    @cargo test --workspace

# 清理构建产物
clean:
    @cargo clean
