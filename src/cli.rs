pub(crate) const HELP: &str = r#"Usage: {tool} [COMMAND] [OPTION]...
Draw passive borders and timer highlights on compatible Linux Wayland compositors.
With no COMMAND, run the shell in the foreground.

Commands:
  available                            check whether temporalshell can draw now
  timer add DURATION [OPTION]...       add a timer relative to now
  timer add --date DATE [OPTION]...    add a timer with a UTC deadline
  timer list                           list timers
  timer prune                          remove timers with passed deadlines
  timer remove ID                      remove a timer by ID
  timer remove --date DATE             remove timers by deadline
  timer reset                          remove every timer
  trigger NAME                         run a named TOML highlight
  trigger [HIGHLIGHT OPTIONS]          run a direct highlight over configured defaults

Options:
      --date DATE
             use UTC YYYY-MM-DDTHH:MM:SSZ; must be strictly future
      --countdown
             animate from timer creation through its deadline
      --id ID
             set a timer ID
      --edge top|right|bottom|left
      --start BOUND
      --end BOUND
      --duration DURATION
      --animation NAME
      --color #RRGGBB
             direct highlight options; any subset may be supplied in any order
      --help
             display this help and exit
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TimerCommand {
    AddDuration {
        duration: String,
        id: Option<String>,
        countdown: bool,
    },
    AddDate {
        date: String,
        id: Option<String>,
        countdown: bool,
    },
    List,
    Prune,
    RemoveId(String),
    RemoveDate(String),
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TriggerCommand {
    Named(String),
    Direct {
        edge: Option<String>,
        start: Option<String>,
        end: Option<String>,
        duration: Option<String>,
        animation: Option<String>,
        color: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help,
    Shell,
    Available,
    Timer(TimerCommand),
    Trigger(TriggerCommand),
}

pub(crate) fn help(tool: &str) -> String {
    HELP.replace("{tool}", tool)
}

fn id_option(args: &[String]) -> Result<Option<String>, &'static str> {
    match args {
        [] => Ok(None),
        [flag, id] if flag == "--id" => Ok(Some(id.clone())),
        _ => Err("invalid command"),
    }
}

fn trigger_options(args: &[String]) -> Result<TriggerCommand, &'static str> {
    let (mut edge, mut start, mut end, mut duration, mut animation, mut color) =
        (None, None, None, None, None, None);
    let mut pairs = args.chunks_exact(2);
    for pair in &mut pairs {
        let target = match pair[0].as_str() {
            "--edge" => &mut edge,
            "--start" => &mut start,
            "--end" => &mut end,
            "--duration" => &mut duration,
            "--animation" => &mut animation,
            "--color" => &mut color,
            _ => return Err("unknown trigger option"),
        };
        if target.is_some() {
            return Err("duplicate trigger option");
        }
        *target = Some(pair[1].clone());
    }
    if !pairs.remainder().is_empty() {
        return Err("trigger option requires a value");
    }
    Ok(TriggerCommand::Direct {
        edge,
        start,
        end,
        duration,
        animation,
        color,
    })
}

pub(crate) fn parse(args: &[String]) -> Result<Command, &'static str> {
    match args {
        [] => Ok(Command::Shell),
        [command] if command == "--help" => Ok(Command::Help),
        [command] if command == "available" => Ok(Command::Available),
        [timer, add, duration, countdown, flags @ ..]
            if timer == "timer"
                && add == "add"
                && duration != "--date"
                && countdown == "--countdown" =>
        {
            Ok(Command::Timer(TimerCommand::AddDuration {
                duration: duration.clone(),
                id: id_option(flags)?,
                countdown: true,
            }))
        }
        [timer, add, duration, flags @ ..]
            if timer == "timer" && add == "add" && duration != "--date" =>
        {
            Ok(Command::Timer(TimerCommand::AddDuration {
                duration: duration.clone(),
                id: id_option(flags)?,
                countdown: false,
            }))
        }
        [timer, add, date_flag, date, countdown, flags @ ..]
            if timer == "timer"
                && add == "add"
                && date_flag == "--date"
                && countdown == "--countdown" =>
        {
            Ok(Command::Timer(TimerCommand::AddDate {
                date: date.clone(),
                id: id_option(flags)?,
                countdown: true,
            }))
        }
        [timer, add, date_flag, date, flags @ ..]
            if timer == "timer" && add == "add" && date_flag == "--date" =>
        {
            Ok(Command::Timer(TimerCommand::AddDate {
                date: date.clone(),
                id: id_option(flags)?,
                countdown: false,
            }))
        }
        [timer, list] if timer == "timer" && list == "list" => {
            Ok(Command::Timer(TimerCommand::List))
        }
        [timer, prune] if timer == "timer" && prune == "prune" => {
            Ok(Command::Timer(TimerCommand::Prune))
        }
        [timer, remove, id] if timer == "timer" && remove == "remove" && id != "--date" => {
            Ok(Command::Timer(TimerCommand::RemoveId(id.clone())))
        }
        [timer, remove, flag, date]
            if timer == "timer" && remove == "remove" && flag == "--date" =>
        {
            Ok(Command::Timer(TimerCommand::RemoveDate(date.clone())))
        }
        [timer, reset] if timer == "timer" && reset == "reset" => {
            Ok(Command::Timer(TimerCommand::Reset))
        }
        [trigger, name] if trigger == "trigger" && !name.starts_with('-') => {
            Ok(Command::Trigger(TriggerCommand::Named(name.clone())))
        }
        [trigger, options @ ..] if trigger == "trigger" => {
            Ok(Command::Trigger(trigger_options(options)?))
        }
        _ => Err("invalid command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn grammar_is_exact() {
        assert_eq!(parse(&args(&["--help"])), Ok(Command::Help));
        let output = help("temporalshell");
        for expected in [
            "Usage: temporalshell [COMMAND] [OPTION]...",
            "Commands:",
            "Options:",
            "--help",
        ] {
            assert!(output.contains(expected));
        }
        assert!(!output.contains("temporalshell help"));
        assert!(matches!(
            parse(&args(&["timer", "add", "10s"])),
            Ok(Command::Timer(TimerCommand::AddDuration {
                countdown: false,
                ..
            }))
        ));
        assert!(matches!(
            parse(&args(&["timer", "add", "10s", "--countdown", "--id", "a"])),
            Ok(Command::Timer(TimerCommand::AddDuration {
                countdown: true,
                ..
            }))
        ));
        assert!(matches!(
            parse(&args(&[
                "timer",
                "add",
                "--date",
                "2096-02-29T12:34:56Z",
                "--countdown"
            ])),
            Ok(Command::Timer(TimerCommand::AddDate {
                countdown: true,
                ..
            }))
        ));
        assert!(matches!(
            parse(&args(&["timer", "prune"])),
            Ok(Command::Timer(TimerCommand::Prune))
        ));
        assert!(matches!(
            parse(&args(&["trigger", "warm"])),
            Ok(Command::Trigger(TriggerCommand::Named(_)))
        ));
        assert!(matches!(
            parse(&args(&["trigger"])),
            Ok(Command::Trigger(TriggerCommand::Direct { .. }))
        ));
        assert!(matches!(
            parse(&args(&[
                "trigger",
                "--duration",
                "2s",
                "--color",
                "#ffffff"
            ])),
            Ok(Command::Trigger(TriggerCommand::Direct { .. }))
        ));
        assert!(matches!(
            parse(&args(&["trigger", "--start", "0%", "--edge", "top"])),
            Ok(Command::Trigger(TriggerCommand::Direct { .. }))
        ));
        assert!(parse(&args(&["trigger", "--fade", "false"])).is_err());
        for invalid in [
            &["help"][..],
            &["--help", "extra"][..],
            &["timer", "normal", "x"][..],
            &["timer", "prune", "now"][..],
            &["timer", "add", "10s", "--highlight", "x"][..],
            &["trigger", "warm", "--id", "x"][..],
            &["trigger", "--edge"][..],
            &["trigger", "--thickness-px", "1"][..],
            &["trigger", "--color", "#fff", "--color", "#ffffff"][..],
        ] {
            assert!(parse(&args(invalid)).is_err(), "accepted {invalid:?}");
        }
    }
}
