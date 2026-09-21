//! UCAS 课程签到核心库。
//!
//! 负责与 iclass 上游 API 交互：登录、时间戳校准、课表查询、直接签到。

crate_macro::mod_pub!(config, model, upstream);
