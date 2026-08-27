use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};

const MAX_ENTRY_BYTES: u64 = 21;
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const AUTO_ID_LENGTH: usize = 8;
const AUTO_ID_ATTEMPTS: usize = 100;

#[derive(Debug)]
pub(crate) enum Error {
    Input(String),
    Runtime(String),
}

pub(crate) fn run(command: &crate::cli::TimerCommand) -> Result<String, Error> {
    let base = state_base().map_err(Error::Runtime)?;
    run_at(&base, command)
}

fn run_at(base: &Path, command: &crate::cli::TimerCommand) -> Result<String, Error> {
    validate_command(command)?;
    let shell = prepare_shell(base).map_err(Error::Runtime)?;
    let lock = open_lock(&shell).map_err(Error::Runtime)?;
    lock.lock()
        .map_err(|error| Error::Runtime(format!("cannot lock {}: {error}", shell.display())))?;
    let timers = shell.join("timers");
    private_directory(&timers).map_err(Error::Runtime)?;
    match command {
        crate::cli::TimerCommand::AddDuration { duration, id } => {
            let seconds = parse_duration(duration).map_err(Error::Input)?;
            let deadline = duration_deadline(OffsetDateTime::now_utc(), seconds)
                .ok_or_else(|| Error::Input("duration is out of range".into()))?;
            add(
                &timers,
                id.as_deref(),
                &canonical(deadline).map_err(Error::Input)?,
            )
            .map_err(Error::Runtime)
        }
        crate::cli::TimerCommand::AddDate { date, id } => {
            let deadline = parse_future_date(date).map_err(Error::Input)?;
            add(
                &timers,
                id.as_deref(),
                &canonical(deadline).map_err(Error::Input)?,
            )
            .map_err(Error::Runtime)
        }
        crate::cli::TimerCommand::List => list(&timers).map_err(Error::Runtime),
        crate::cli::TimerCommand::RemoveId(id) => {
            validate_id(id).map_err(Error::Input)?;
            remove_id(&timers, id).map_err(Error::Runtime)
        }
        crate::cli::TimerCommand::RemoveDate(date) => {
            validate_date(date).map_err(Error::Input)?;
            remove_date(&timers, date).map_err(Error::Runtime)
        }
        crate::cli::TimerCommand::Reset => reset(&timers).map_err(Error::Runtime),
    }
}

fn validate_command(command: &crate::cli::TimerCommand) -> Result<(), Error> {
    match command {
        crate::cli::TimerCommand::AddDuration { duration, id } => {
            let seconds = parse_duration(duration).map_err(Error::Input)?;
            duration_deadline(OffsetDateTime::now_utc(), seconds)
                .ok_or_else(|| Error::Input("duration is out of range".into()))?;
            if let Some(id) = id {
                validate_id(id).map_err(Error::Input)?;
            }
        }
        crate::cli::TimerCommand::AddDate { date, id } => {
            parse_future_date(date).map_err(Error::Input)?;
            if let Some(id) = id {
                validate_id(id).map_err(Error::Input)?;
            }
        }
        crate::cli::TimerCommand::RemoveId(id) => validate_id(id).map_err(Error::Input)?,
        crate::cli::TimerCommand::RemoveDate(date) => validate_date(date).map_err(Error::Input)?,
        crate::cli::TimerCommand::List | crate::cli::TimerCommand::Reset => {}
    }
    Ok(())
}

fn state_base() -> Result<PathBuf, String> {
    match env::var_os("XDG_STATE_HOME") {
        Some(value) => absolute_path("XDG_STATE_HOME", value),
        None => absolute_path(
            "HOME",
            env::var_os("HOME").ok_or("set XDG_STATE_HOME or HOME")?,
        )
        .map(|path| path.join(".local/state")),
    }
}

fn absolute_path(name: &str, value: std::ffi::OsString) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(format!("{name} must be a non-empty absolute path"));
    }
    Ok(path)
}

fn prepare_shell(base: &Path) -> Result<PathBuf, String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(base)
        .map_err(|error| format!("cannot create {}: {error}", base.display()))?;
    let metadata = fs::symlink_metadata(base)
        .map_err(|error| format!("cannot inspect {}: {error}", base.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{} must be a directory", base.display()));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o022 != 0 {
        return Err(format!(
            "{} must not be writable by other users",
            base.display()
        ));
    }
    let owner = base.join("reEnvisioning");
    let shell = owner.join("temporalShell");
    private_directory(&owner)?;
    private_directory(&shell)?;
    Ok(shell)
}

fn private_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            match builder.create(path) {
                Ok(()) => sync_parent(path)?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
            }
        }
        Ok(_) => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{} must be a directory", path.display()));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!("{} must be private", path.display()));
    }
    Ok(())
}

fn open_lock(shell: &Path) -> Result<File, String> {
    let path = shell.join(".timers.lock");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!("{} must be a regular file", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let path_metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || !metadata.is_file()
        || {
            #[cfg(unix)]
            {
                !same_file(&path_metadata, &metadata)
            }
            #[cfg(not(unix))]
            {
                false
            }
        }
    {
        return Err(format!(
            "{} must be the opened regular file",
            path.display()
        ));
    }
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("cannot secure {}: {error}", path.display()))?;
    Ok(file)
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn add(timers: &Path, requested_id: Option<&str>, date: &str) -> Result<String, String> {
    private_directory(timers)?;
    let id = match requested_id {
        Some(id) => {
            validate_id(id)?;
            if !target_available(&timers.join(id))? {
                return Err(format!("timer {id} already exists"));
            }
            id.to_owned()
        }
        None => automatic_id(timers)?,
    };
    let temporary = timers.join(format!(".{id}.tmp"));
    let target = timers.join(&id);
    remove_stale_temp(&temporary)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
    let result = file
        .write_all(date.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all());
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot write {}: {error}", target.display()));
    }
    match fs::hard_link(&temporary, &target) {
        Ok(()) => {
            let _ = fs::remove_file(&temporary);
            if let Err(error) = sync_directory(timers) {
                return Err(format!(
                    "timer {id} stored but durability sync failed: {error}"
                ));
            }
            Ok(format!("{id}\t{date}\n"))
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Err(format!("timer {id} already exists"))
            } else {
                Err(format!("cannot store {}: {error}", target.display()))
            }
        }
    }
}

fn remove_stale_temp(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata) if metadata.file_type().is_file() => fs::remove_file(path)
            .map_err(|error| format!("cannot remove stale {}: {error}", path.display())),
        Ok(_) => Err(format!(
            "{} must be a regular stale timer temporary",
            path.display()
        )),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

#[derive(Debug)]
struct Entry {
    id: String,
    date: String,
}

fn entries(timers: &Path) -> Result<Vec<Entry>, String> {
    match fs::symlink_metadata(timers) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(format!("{} must be a directory", timers.display())),
        Err(error) => return Err(format!("cannot inspect {}: {error}", timers.display())),
    }
    let mut entries = Vec::new();
    for item in fs::read_dir(timers)
        .map_err(|error| format!("cannot read {}: {error}", timers.display()))?
    {
        let item = item.map_err(|error| format!("cannot read {}: {error}", timers.display()))?;
        let name = item.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| format!("{} contains a non-UTF-8 entry", timers.display()))?;
        if name == ".timers.lock" {
            return Err(format!(
                "{} contains an unexpected lock file",
                timers.display()
            ));
        }
        if is_temp(name) {
            let metadata = fs::symlink_metadata(item.path())
                .map_err(|error| format!("cannot inspect {}: {error}", item.path().display()))?;
            if !metadata.file_type().is_file() {
                return Err(format!(
                    "{} must be a regular stale timer temporary",
                    item.path().display()
                ));
            }
            continue;
        }
        validate_id(name)?;
        let path = item.path();
        let path_metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
            return Err(format!("{} must be a regular file", path.display()));
        }
        let file = File::open(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let metadata = file
            .metadata()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if !metadata.is_file() || {
            #[cfg(unix)]
            {
                !same_file(&path_metadata, &metadata)
            }
            #[cfg(not(unix))]
            {
                false
            }
        } {
            return Err(format!("{} changed while opening", path.display()));
        }
        if metadata.len() > MAX_ENTRY_BYTES {
            return Err(format!("{} is oversized", path.display()));
        }
        let mut contents = String::new();
        file.take(MAX_ENTRY_BYTES + 1)
            .read_to_string(&mut contents)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if contents.len() as u64 > MAX_ENTRY_BYTES || !contents.ends_with('\n') {
            return Err(format!(
                "{} must contain one canonical date",
                item.path().display()
            ));
        }
        let date = contents.strip_suffix('\n').expect("checked above");
        validate_date(date)
            .map_err(|_| format!("{} must contain one canonical date", item.path().display()))?;
        entries.push(Entry {
            id: name.to_owned(),
            date: date.to_owned(),
        });
    }
    Ok(entries)
}

fn is_temp(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(|id| validate_id(id).is_ok())
}

fn automatic_id(timers: &Path) -> Result<String, String> {
    available_id(timers, (0..AUTO_ID_ATTEMPTS).map(|_| random_id()))
}

fn available_id(
    timers: &Path,
    candidates: impl IntoIterator<Item = Result<String, String>>,
) -> Result<String, String> {
    for candidate in candidates {
        let id = candidate?;
        validate_id(&id)?;
        if target_available(&timers.join(&id))? {
            return Ok(id);
        }
    }
    Err("cannot generate an unused timer ID".into())
}

fn target_available(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(format!("{} must be a regular file", path.display()))
        }
        Ok(_) => Ok(false),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn random_id() -> Result<String, String> {
    for _ in 0..AUTO_ID_ATTEMPTS {
        let mut bytes = [0; AUTO_ID_LENGTH * 2];
        getrandom::fill(&mut bytes)
            .map_err(|error| format!("cannot generate timer ID: {error}"))?;
        if let Some(id) = encode_id(&bytes) {
            return Ok(id);
        }
    }
    Err("cannot generate timer ID".into())
}

fn encode_id(bytes: &[u8]) -> Option<String> {
    let mut id = String::with_capacity(AUTO_ID_LENGTH);
    for byte in bytes.iter().copied().filter(|byte| *byte < 252) {
        id.push(ID_ALPHABET[usize::from(byte % ID_ALPHABET.len() as u8)] as char);
        if id.len() == AUTO_ID_LENGTH {
            return Some(id);
        }
    }
    None
}

fn list(timers: &Path) -> Result<String, String> {
    let mut entries = entries(timers)?;
    entries.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(entries
        .into_iter()
        .map(|entry| format!("{}\t{}\n", entry.id, entry.date))
        .collect())
}

fn remove_id(timers: &Path, id: &str) -> Result<String, String> {
    let entries = entries(timers)?;
    if !entries.iter().any(|entry| entry.id == id) {
        return Err(format!("timer {id} does not exist"));
    }
    fs::remove_file(timers.join(id))
        .map_err(|error| format!("cannot remove timer {id}: {error}"))?;
    sync_directory(timers)?;
    Ok(String::new())
}

fn remove_date(timers: &Path, date: &str) -> Result<String, String> {
    let entries = entries(timers)?;
    let matches: Vec<_> = entries
        .into_iter()
        .filter(|entry| entry.date == date)
        .collect();
    if matches.is_empty() {
        return Err(format!("no timer has date {date}"));
    }
    for entry in matches {
        fs::remove_file(timers.join(&entry.id)).map_err(|error| {
            format!(
                "cannot remove timer {}: {error}; removal may be partial",
                entry.id
            )
        })?;
    }
    sync_directory(timers)?;
    Ok(String::new())
}

fn reset(timers: &Path) -> Result<String, String> {
    let entries = entries(timers)?;
    let mut paths: Vec<_> = entries
        .into_iter()
        .map(|entry| timers.join(entry.id))
        .collect();
    if let Ok(directory) = fs::read_dir(timers) {
        for item in directory {
            let item =
                item.map_err(|error| format!("cannot read {}: {error}", timers.display()))?;
            if is_temp(&item.file_name().to_string_lossy()) {
                paths.push(item.path());
            }
        }
    }
    for path in paths {
        fs::remove_file(&path).map_err(|error| {
            format!(
                "cannot remove {}: {error}; reset may be partial",
                path.display()
            )
        })?;
    }
    if timers.exists() {
        sync_directory(timers)?;
    }
    Ok(String::new())
}

fn sync_parent(path: &Path) -> Result<(), String> {
    path.parent().map(sync_directory).transpose()?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}

fn validate_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    if !(1..=64).contains(&bytes.len())
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("ID must match [A-Za-z0-9][A-Za-z0-9_-]{0,63}".into());
    }
    Ok(())
}

fn parse_duration(value: &str) -> Result<i64, String> {
    if value.is_empty() {
        return Err("duration must use descending positive d/h/m/s components".into());
    }
    let mut rest = value.as_bytes();
    let mut last_unit = 4;
    let mut total = 0_u64;
    while !rest.is_empty() {
        let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
        if digits == 0 || digits == rest.len() {
            return Err("duration must use descending positive d/h/m/s components".into());
        }
        let amount: u64 = std::str::from_utf8(&rest[..digits])
            .expect("ASCII digits")
            .parse()
            .map_err(|_| "duration is out of range")?;
        if amount == 0 {
            return Err("duration components must be positive".into());
        }
        let (unit, multiplier) = match rest[digits] {
            b'd' => (3, 86_400),
            b'h' => (2, 3_600),
            b'm' => (1, 60),
            b's' => (0, 1),
            _ => return Err("duration must use descending positive d/h/m/s components".into()),
        };
        if unit >= last_unit {
            return Err("duration components must be in descending d/h/m/s order".into());
        }
        last_unit = unit;
        total = total
            .checked_add(
                amount
                    .checked_mul(multiplier)
                    .ok_or("duration is out of range")?,
            )
            .ok_or("duration is out of range")?;
        rest = &rest[digits + 1..];
    }
    i64::try_from(total).map_err(|_| "duration is out of range".into())
}

fn duration_deadline(now: OffsetDateTime, seconds: i64) -> Option<OffsetDateTime> {
    let now = if now.nanosecond() == 0 {
        now
    } else {
        now.replace_nanosecond(0)
            .ok()?
            .checked_add(Duration::seconds(1))?
    };
    now.checked_add(Duration::seconds(seconds))
}

fn parse_future_date(value: &str) -> Result<OffsetDateTime, String> {
    let deadline = parse_date(value)?;
    if deadline <= OffsetDateTime::now_utc() {
        return Err("date must be in the future".into());
    }
    Ok(deadline)
}

fn validate_date(value: &str) -> Result<(), String> {
    parse_date(value).map(|_| ())
}

fn parse_date(value: &str) -> Result<OffsetDateTime, String> {
    if value.len() != 20
        || !matches!(value.as_bytes(), [a, b, c, d, b'-', f, g, b'-', i, j, b'T', l, m, b':', o, p, b':', r, s, b'Z'] if [*a, *b, *c, *d, *f, *g, *i, *j, *l, *m, *o, *p, *r, *s].iter().all(u8::is_ascii_digit))
    {
        return Err("date must be YYYY-MM-DDTHH:MM:SSZ in UTC".into());
    }
    let deadline = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| "date must be a valid UTC RFC3339 timestamp")?;
    if canonical(deadline)? != value {
        return Err("date must be YYYY-MM-DDTHH:MM:SSZ in UTC".into());
    }
    Ok(deadline)
}

fn canonical(deadline: OffsetDateTime) -> Result<String, String> {
    if !(0..=9999).contains(&deadline.year()) || deadline.nanosecond() != 0 {
        return Err("deadline is outside the supported second-precision UTC range".into());
    }
    deadline
        .format(&Rfc3339)
        .map_err(|_| "deadline is outside the supported UTC format".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };

    static TEST_NUMBER: AtomicUsize = AtomicUsize::new(0);

    fn base() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "temporalshell-timer-{}-{}",
            std::process::id(),
            TEST_NUMBER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn command(command: crate::cli::TimerCommand) -> crate::cli::TimerCommand {
        command
    }

    #[test]
    fn ids_and_dates_are_strict() {
        for id in ["a", "Z", "0", "a_", "A-1", &"a".repeat(64)] {
            assert!(validate_id(id).is_ok());
        }
        for id in ["", "-a", "_a", "a!", &"a".repeat(65)] {
            assert!(validate_id(id).is_err());
        }
        assert!(validate_date("2000-01-01T00:00:00Z").is_ok());
        assert!(validate_date("2096-02-29T12:34:56Z").is_ok());
        assert!(validate_date("2096-02-29T12:34:56+00:00").is_err());
        assert!(parse_future_date("2000-01-01T00:00:00Z").is_err());
        assert_eq!(
            encode_id(&[0, 25, 26, 35, 251, 252, 1, 2, 3]),
            Some("az099bcd".into())
        );
    }

    #[test]
    fn add_list_remove_and_reset_lifecycle() {
        let base = base();
        let date = "2096-02-29T12:34:56Z";
        assert_eq!(
            run_at(
                &base,
                &command(crate::cli::TimerCommand::AddDate {
                    date: date.into(),
                    id: Some("z".into())
                })
            )
            .unwrap(),
            format!("z\t{date}\n")
        );
        assert_eq!(
            run_at(
                &base,
                &command(crate::cli::TimerCommand::AddDate {
                    date: "2095-02-28T12:34:56Z".into(),
                    id: Some("a".into())
                })
            )
            .unwrap(),
            "a\t2095-02-28T12:34:56Z\n"
        );
        assert_eq!(
            run_at(&base, &command(crate::cli::TimerCommand::List)).unwrap(),
            "a\t2095-02-28T12:34:56Z\nz\t2096-02-29T12:34:56Z\n"
        );
        assert_eq!(
            run_at(
                &base,
                &command(crate::cli::TimerCommand::RemoveDate(
                    "2095-02-28T12:34:56Z".into()
                ))
            )
            .unwrap(),
            ""
        );
        assert!(run_at(
            &base,
            &command(crate::cli::TimerCommand::RemoveId("missing".into()))
        )
        .is_err());
        assert_eq!(
            run_at(&base, &command(crate::cli::TimerCommand::Reset)).unwrap(),
            ""
        );
        assert_eq!(
            run_at(&base, &command(crate::cli::TimerCommand::List)).unwrap(),
            ""
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn automatic_ids_are_random_and_retry_collisions() {
        let base = base();
        run_at(
            &base,
            &crate::cli::TimerCommand::AddDate {
                date: "2096-02-29T12:34:56Z".into(),
                id: Some("7".into()),
            },
        )
        .unwrap();
        let timers = base.join("reEnvisioning/temporalShell/timers");
        fs::write(timers.join("aaaaaaaa"), "2096-02-29T12:34:56Z\n").unwrap();
        assert_eq!(
            available_id(&timers, [Ok("aaaaaaaa".into()), Ok("bbbbbbbb".into())]).unwrap(),
            "bbbbbbbb"
        );
        let output = run_at(
            &base,
            &crate::cli::TimerCommand::AddDate {
                date: "2097-02-28T12:34:56Z".into(),
                id: None,
            },
        )
        .unwrap();
        let id = output.split_once('\t').unwrap().0;
        assert_eq!(id.len(), AUTO_ID_LENGTH);
        assert!(id.bytes().all(|byte| ID_ALPHABET.contains(&byte)));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn malformed_state_blocks_mutation_and_reset_removes_known_temps() {
        let base = base();
        let owner = base.join("reEnvisioning");
        let shell = owner.join("temporalShell");
        let timers = shell.join("timers");
        private_directory(&owner).unwrap();
        private_directory(&shell).unwrap();
        private_directory(&timers).unwrap();
        fs::write(timers.join("bad!"), "2096-02-29T12:34:56Z\n").unwrap();
        assert!(run_at(&base, &crate::cli::TimerCommand::List).is_err());
        assert!(run_at(&base, &crate::cli::TimerCommand::Reset).is_err());
        fs::remove_file(timers.join("bad!")).unwrap();
        fs::write(timers.join("1"), "2000-01-01T00:00:00Z\nextra").unwrap();
        assert!(run_at(&base, &crate::cli::TimerCommand::List).is_err());
        fs::remove_file(timers.join("1")).unwrap();
        fs::write(timers.join("1"), "2000-01-01T00:00:00Z\n").unwrap();
        assert_eq!(
            run_at(&base, &crate::cli::TimerCommand::List).unwrap(),
            "1\t2000-01-01T00:00:00Z\n"
        );
        assert_eq!(
            run_at(
                &base,
                &crate::cli::TimerCommand::RemoveDate("2000-01-01T00:00:00Z".into())
            )
            .unwrap(),
            ""
        );
        fs::write(timers.join(".1.tmp"), "partial").unwrap();
        assert_eq!(run_at(&base, &crate::cli::TimerCommand::Reset).unwrap(), "");
        assert!(!timers.join(".1.tmp").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn identity_check_distinguishes_replacements() {
        let base = base();
        let first = base.join("first");
        let linked = base.join("linked");
        let other = base.join("other");
        fs::write(&first, "one").unwrap();
        fs::hard_link(&first, &linked).unwrap();
        fs::write(&other, "two").unwrap();
        assert!(same_file(
            &fs::symlink_metadata(&first).unwrap(),
            &fs::metadata(&linked).unwrap()
        ));
        assert!(!same_file(
            &fs::symlink_metadata(&first).unwrap(),
            &fs::metadata(&other).unwrap()
        ));
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn writable_state_base_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let base = base();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(run_at(&base, &crate::cli::TimerCommand::List).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_state_is_rejected() {
        use std::os::unix::fs::symlink;

        let base = base();
        let owner = base.join("reEnvisioning");
        let shell = owner.join("temporalShell");
        let timers = shell.join("timers");
        private_directory(&owner).unwrap();
        private_directory(&shell).unwrap();
        private_directory(&timers).unwrap();
        let outside = base.join("outside");
        fs::write(&outside, "2096-02-29T12:34:56Z\n").unwrap();
        symlink(&outside, timers.join("safe")).unwrap();
        assert!(run_at(&base, &crate::cli::TimerCommand::List).is_err());
        assert_eq!(
            fs::read_to_string(outside).unwrap(),
            "2096-02-29T12:34:56Z\n"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_lock_is_rejected_without_chmodding_its_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let base = base();
        let owner = base.join("reEnvisioning");
        let shell = owner.join("temporalShell");
        private_directory(&owner).unwrap();
        private_directory(&shell).unwrap();
        let outside = base.join("outside");
        fs::write(&outside, "outside").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&outside, shell.join(".timers.lock")).unwrap();
        assert!(run_at(&base, &crate::cli::TimerCommand::List).is_err());
        assert_eq!(
            fs::metadata(outside).unwrap().permissions().mode() & 0o777,
            0o644
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn lock_serializes_auto_ids() {
        let base = base();
        let left = base.clone();
        let right = base.clone();
        let one = thread::spawn(move || {
            run_at(
                &left,
                &crate::cli::TimerCommand::AddDate {
                    date: "2096-02-29T12:34:56Z".into(),
                    id: None,
                },
            )
            .unwrap()
        });
        let two = thread::spawn(move || {
            run_at(
                &right,
                &crate::cli::TimerCommand::AddDate {
                    date: "2097-02-28T12:34:56Z".into(),
                    id: None,
                },
            )
            .unwrap()
        });
        let ids = [one.join().unwrap(), two.join().unwrap()];
        assert_ne!(ids[0], ids[1]);
        for id in ids.map(|output| output.split_once('\t').unwrap().0.to_owned()) {
            assert_eq!(id.len(), AUTO_ID_LENGTH);
            assert!(id.bytes().all(|byte| ID_ALPHABET.contains(&byte)));
        }
        fs::remove_dir_all(base).unwrap();
    }
}
