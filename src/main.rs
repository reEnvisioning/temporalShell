use std::{env, process};

mod cli;
mod core;
#[cfg(target_os = "linux")]
mod linux;
mod timer;

fn main() {
    let tool = env::args()
        .next()
        .and_then(|path| path.rsplit('/').next().map(str::to_owned))
        .unwrap_or_else(|| "temporalshell".into());
    let args: Vec<String> = env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(cli::Command::Help) => print!("{}", cli::help(&tool)),
        Ok(cli::Command::Timer(command)) => run_state(timer::run(&command), &tool),
        Ok(cli::Command::Trigger(command)) => run_state(timer::run_trigger(&command), &tool),
        Ok(command) => run_platform(&command, &tool),
        Err(error) => usage_error(&tool, error),
    }
}

fn run_state(result: Result<String, timer::Error>, tool: &str) {
    match result {
        Ok(output) => print!("{output}"),
        Err(timer::Error::Input(error)) => usage_error(tool, &error),
        Err(timer::Error::Runtime(error)) => runtime_error(tool, &error),
    }
}

fn usage_error(tool: &str, error: &str) -> ! {
    eprintln!("{tool}: {error}\nusage: {tool} ...; run '{tool} --help'");
    process::exit(2);
}

fn runtime_error(tool: &str, error: &str) -> ! {
    eprintln!("{tool}: {error}");
    process::exit(1);
}

#[cfg(target_os = "linux")]
fn run_platform(command: &cli::Command, tool: &str) {
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
fn run_platform(command: &cli::Command, _: &str) {
    let action = if matches!(command, cli::Command::Available) {
        "available"
    } else {
        "run"
    };
    eprintln!("unavailable: temporalshell requires Linux Wayland ({action})");
    process::exit(1);
}
