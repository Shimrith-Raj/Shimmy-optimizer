// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  // The installer calls `--autostart on|off` to add or remove the sign-in task; that exits without opening Shimmy.
  if let Some(code) = shimmy_lib::installer_step() {
    std::process::exit(code);
  }
  // `--report` prints a one-page summary of this PC and exits, without opening Shimmy.
  if let Some(code) = shimmy_lib::report_step() {
    std::process::exit(code);
  }
  shimmy_lib::run();
}
