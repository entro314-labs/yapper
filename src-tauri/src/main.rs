// The Windows release build is a GUI app: without this it opens a console window
// behind the real one.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    yapper_lib::run();
}
