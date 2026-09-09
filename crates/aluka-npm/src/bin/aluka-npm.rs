//! `aluka-npm` 命令行入口：aluka 生态的 npm 功能复刻。
//!
//! 职责仅限参数切分与退出码映射；全部逻辑在 `aluka_npm::commands`。

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = PathBuf::from(std::env::var("ALUKA_NPM_CWD").unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|_| ".".to_owned())
    }));
    match aluka_npm::commands::dispatch(&args, &cwd) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("aluka-npm 错误: {}", e.message);
            ExitCode::from(e.code.clamp(0, 255) as u8)
        }
    }
}
