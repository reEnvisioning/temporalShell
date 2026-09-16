use std::{env, process};

mod animation;
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
        Ok(cli::Command::Shell) => run_shell(&tool),
        Ok(cli::Command::Available) => run_available(),
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
fn run_shell(tool: &str) {
    if let Err(error) = linux::shell() {
        runtime_error(tool, &error);
    }
}

#[cfg(target_os = "linux")]
fn run_available() {
    if let Err(error) = linux::available_command() {
        eprintln!("unavailable: {error}");
        process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn run_shell(_: &str) {
    eprintln!("unavailable: temporalshell requires Linux Wayland (run)");
    process::exit(1);
}

#[cfg(not(target_os = "linux"))]
fn run_available() {
    eprintln!("unavailable: temporalshell requires Linux Wayland (available)");
    process::exit(1);
}
