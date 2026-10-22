// A GUI app on Windows must not open a console window next to itself. This
// attribute can only live at the crate root, so it is the one `cfg(windows)`
// outside a platform module. Debug builds keep the console for their output.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;
use std::rc::Rc;

use clap::Parser;

use hatchzip_gui::args::{Args, Launch};
use hatchzip_gui::{backend, main_window, task_window};

fn main() -> ExitCode {
    let launch = Args::parse().launch();
    let drops = Rc::new(backend::Drops::default());
    let result = backend::select(drops.clone()).and_then(|()| match launch {
        Launch::Main(archive) => main_window::run(archive, drops),
        Launch::Task(kind, archive) => task_window::run(kind, archive),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hatchzip-gui: {e:#}");
            ExitCode::FAILURE
        }
    }
}
