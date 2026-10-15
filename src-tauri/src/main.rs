// 防止移动窗口在 release 编译时弹出控制台
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    csv_utf8_converter_lib::run()
}