// 发布版使用 GUI 子系统：双击 exe 不再附带黑色控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    terry_comfy_launcher_lib::run();
}
