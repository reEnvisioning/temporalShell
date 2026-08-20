use std::{env, process};

mod cli;
mod core;
#[cfg(target_os = "linux")]
mod linux;

fn main() {
    let tool = env::args()
        .next()
        .and_then(|path| path.rsplit('/').next().map(str::to_owned))
        .unwrap_or_else(|| "temporalShell".into());
    let args: Vec<String> = env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(cli::Command::Help) => print!("{}", cli::help(&tool)),
        Ok(command) => run_platform(command, &tool),
        Err(error) => {
            eprintln!("{tool}: {error}\nrun '{tool} help'");
            process::exit(2);
        }
    }
}

#[cfg(target_os = "linux")]
fn run_platform(command: cli::Command, tool: &str) {
    match linux::run(command) {
        Ok(()) => {}
        Err(linux::RunError::Shell(reason)) | Err(linux::RunError::Unavailable(reason)) => {
            let prefix = if matches!(command, cli::Command::Available) {
                "unavailable"
            } else {
                tool
            };
            eprintln!("{prefix}: {reason}");
            process::exit(1);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run_platform(command: cli::Command, _: &str) {
    let action = if matches!(command, cli::Command::Available) {
        "available"
    } else {
        "run"
    };
    eprintln!("unavailable: temporalShell requires Linux Wayland ({action})");
    process::exit(1);
}
