// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 无 GUI CLI：`dsh-launcher plugin ...` / `dsh-launcher skill ...`
    // 与 GUI 共用同一 core（状态机/幂等/回滚语义一致），便于脚本化验收。
    let args: Vec<String> = std::env::args().skip(1).collect();
    if dsh_launcher_lib::cli::is_cli_invocation(&args) {
        std::process::exit(dsh_launcher_lib::cli::run(&args));
    }
    dsh_launcher_lib::run()
}
