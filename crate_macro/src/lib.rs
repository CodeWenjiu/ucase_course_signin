//! 模块声明宏（Module Declaration Constitution）。
//!
//! 全 workspace 的 crate 模块声明必须通过 `mod_pub!` / `mod_prv!` 完成，
//! 禁止手写裸 `mod X;`。本文件是唯一允许裸 `mod module;` 的地方
//! （bootstrap 问题：宏定义在 `module` 模块内部）。

mod module;
