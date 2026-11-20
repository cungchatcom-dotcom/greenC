//! Desktop GUI entry point with single-instance check and Windows Subsystem.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_code)]

fn main() {
    greenc_app::run();
}
