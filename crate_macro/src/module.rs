/// 声明公开模块：`mod_pub!(a, b, c);` 展开为 `pub mod a; pub mod b; pub mod c;`。
///
/// 支持可见性前缀：`mod_pub!(crate, A);` → `pub(crate) mod A;`，
/// `mod_pub!(super, A);` → `pub(super) mod A;`，
/// `mod_pub!(pub(self), A);` → `pub(self) mod A;`。
#[macro_export]
macro_rules! mod_pub {
    ($($module_name:ident),+ $(,)?) => {
        $(pub mod $module_name;)+
    };
    (crate, $($module_name:ident),+ $(,)?) => {
        $(pub(crate) mod $module_name;)+
    };
    (super, $($module_name:ident),+ $(,)?) => {
        $(pub(super) mod $module_name;)+
    };
    (pub(self), $($module_name:ident),+ $(,)?) => {
        $(pub(self) mod $module_name;)+
    };
}

/// 声明私有模块：`mod_prv!(a, b);` 展开为 `mod a; mod b;`，仅在 crate 内可见。
#[macro_export]
macro_rules! mod_prv {
    ($($module_name:ident),+ $(,)?) => {
        $(mod $module_name;)+
    };
}
