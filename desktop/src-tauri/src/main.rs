// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  std::panic::set_hook(Box::new(|info| {
    let bt = std::backtrace::Backtrace::force_capture();
    let msg = format!("🔥 PANIC: {info}\nBacktrace:\n{bt}\n");
    eprintln!("{msg}");
    let _ = std::fs::write(std::env::temp_dir().join("pony-desktop-panic.log"), &msg);
  }));
  pony_desktop_lib::run();
}
