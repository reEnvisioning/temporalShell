pub(crate) const HELP: &str = "Passive borders for compatible Linux Wayland compositors.\n\nUsage:\n  {tool}\n  {tool} available\n  {tool} timer add DURATION\n  {tool} timer add DURATION --id ID\n  {tool} timer add --date YYYY-MM-DDTHH:MM:SSZ\n  {tool} timer add --date DATE --id ID\n  {tool} timer list\n  {tool} timer remove ID\n  {tool} timer remove --date DATE\n  {tool} timer reset\n  {tool} help\n\navailable probes compositor capability without creating surfaces. Timer dates use UTC YYYY-MM-DDTHH:MM:SSZ; add dates must be strictly future.\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TimerCommand {
    AddDuration {
        duration: String,
        id: Option<String>,
    },
    AddDate {
        date: String,
        id: Option<String>,
    },
    List,
    RemoveId(String),
    RemoveDate(String),
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help,
    Shell,
    Available,
    Timer(TimerCommand),
}

pub(crate) fn help(tool: &str) -> String {
    HELP.replace("{tool}", tool)
}

pub(crate) fn parse(args: &[String]) -> Result<Command, &'static str> {
    match args {
        [] => Ok(Command::Shell),
        [command] if command == "help" => Ok(Command::Help),
        [command] if command == "available" => Ok(Command::Available),
        [timer, add, duration] if timer == "timer" && add == "add" => {
            Ok(Command::Timer(TimerCommand::AddDuration {
                duration: duration.clone(),
                id: None,
            }))
        }
        [timer, add, duration, flag, id] if timer == "timer" && add == "add" && flag == "--id" => {
            Ok(Command::Timer(TimerCommand::AddDuration {
                duration: duration.clone(),
                id: Some(id.clone()),
            }))
        }
        [timer, add, flag, date] if timer == "timer" && add == "add" && flag == "--date" => {
            Ok(Command::Timer(TimerCommand::AddDate {
                date: date.clone(),
                id: None,
            }))
        }
        [timer, add, flag, date, id_flag, id]
            if timer == "timer" && add == "add" && flag == "--date" && id_flag == "--id" =>
        {
            Ok(Command::Timer(TimerCommand::AddDate {
                date: date.clone(),
                id: Some(id.clone()),
            }))
        }
        [timer, list] if timer == "timer" && list == "list" => {
            Ok(Command::Timer(TimerCommand::List))
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
    fn timer_grammar_is_exact() {
        assert_eq!(parse(&[]), Ok(Command::Shell));
        assert_eq!(parse(&args(&["help"])), Ok(Command::Help));
        assert_eq!(parse(&args(&["available"])), Ok(Command::Available));
        assert_eq!(
            parse(&args(&["timer", "add", "10s", "--id", "A_1"])),
            Ok(Command::Timer(TimerCommand::AddDuration {
                duration: "10s".into(),
                id: Some("A_1".into()),
            }))
        );
        assert_eq!(
            parse(&args(&["timer", "add", "--date", "2096-02-29T12:34:56Z"])),
            Ok(Command::Timer(TimerCommand::AddDate {
                date: "2096-02-29T12:34:56Z".into(),
                id: None,
            }))
        );
        assert_eq!(
            parse(&args(&["timer", "list"])),
            Ok(Command::Timer(TimerCommand::List))
        );
        assert_eq!(
            parse(&args(&["timer", "remove", "a"])),
            Ok(Command::Timer(TimerCommand::RemoveId("a".into())))
        );
        assert_eq!(
            parse(&args(&["timer", "reset"])),
            Ok(Command::Timer(TimerCommand::Reset))
        );
        for invalid in [
            &["timer", "10s"][..],
            &["timer", "add"][..],
            &["timer", "add", "--id", "x", "10s"][..],
            &["timer", "add", "10s", "--date", "2096-02-29T12:34:56Z"][..],
            &["timer", "add", "--date", "2096-02-29T12:34:56Z", "x"][..],
            &["timer", "list", "x"][..],
            &["timer", "remove"][..],
            &["timer", "remove", "--date"][..],
            &["timer", "reset", "x"][..],
        ] {
            assert!(parse(&args(invalid)).is_err(), "accepted {invalid:?}");
        }
    }
}
