pub(crate) const HELP: &str = "Passive borders for compatible Linux Wayland compositors.\n\nUsage:\n  {tool}\n  {tool} available\n  {tool} help\n\navailable probes compositor capability without creating surfaces.\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help,
    Shell,
    Available,
}

pub(crate) fn help(tool: &str) -> String {
    HELP.replace("{tool}", tool)
}

pub(crate) fn parse(args: &[String]) -> Result<Command, &'static str> {
    match args {
        [] => Ok(Command::Shell),
        [command] if command == "help" => Ok(Command::Help),
        [command] if command == "available" => Ok(Command::Available),
        _ => Err("invalid command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn help_and_commands_are_strict() {
        assert_eq!(parse(&[]), Ok(Command::Shell));
        assert_eq!(parse(&["help".into()]), Ok(Command::Help));
        assert_eq!(parse(&["available".into()]), Ok(Command::Available));
        assert!(parse(&["available".into(), "x".into()]).is_err());
        assert!(parse(&["--help".into()]).is_err());
    }
}
