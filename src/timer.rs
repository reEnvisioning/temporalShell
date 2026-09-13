use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};

const MAX_ENTRY_BYTES: u64 = 256;
const MAX_ENTRIES: usize = 4096;
const AUTO_ID_LENGTH: usize = 8;
const AUTO_ID_ATTEMPTS: usize = 100;
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

#[derive(Debug)]
pub(crate) enum Error {
    Input(String),
    Runtime(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveHighlight {
    pub(crate) started_ns: i128,
    pub(crate) expires_ns: i128,
    pub(crate) id: String,
    pub(crate) style: Option<crate::core::TimerConfig>,
}

#[derive(Clone, Debug)]
enum TimerEntry {
    Normal {
        id: String,
        deadline: String,
    },
    Countdown {
        id: String,
        deadline: String,
        created: String,
    },
}

#[derive(Clone, Debug)]
struct TriggerEntry {
    id: String,
    created: String,
    style: crate::core::TimerConfig,
}

pub(crate) fn run(command: &crate::cli::TimerCommand) -> Result<String, Error> {
    validate_timer(command)?;
    let base = state_base().map_err(Error::Runtime)?;
    let shell = prepare_shell(&base).map_err(Error::Runtime)?;
    let (_lock, directory) = lock_directory(&shell, "timers").map_err(Error::Runtime)?;
    match command {
        crate::cli::TimerCommand::AddDuration {
            duration,
            id,
            countdown,
        } => {
            let created = OffsetDateTime::now_utc();
            let deadline =
                duration_deadline(created, parse_duration(duration).map_err(Error::Input)?)
                    .ok_or_else(|| Error::Input("duration is out of range".into()))?;
            add_timer(
                &directory,
                id.as_deref(),
                &if *countdown {
                    TimerEntry::Countdown {
                        id: String::new(),
                        deadline: canonical(deadline).map_err(Error::Input)?,
                        created: canonical_timestamp(created).map_err(Error::Input)?,
                    }
                } else {
                    TimerEntry::Normal {
                        id: String::new(),
                        deadline: canonical(deadline).map_err(Error::Input)?,
                    }
                },
            )
            .map_err(Error::Runtime)
        }
        crate::cli::TimerCommand::AddDate {
            date,
            id,
            countdown,
        } => {
            let created = OffsetDateTime::now_utc();
            let deadline =
                canonical(parse_future_date(date).map_err(Error::Input)?).map_err(Error::Input)?;
            add_timer(
                &directory,
                id.as_deref(),
                &if *countdown {
                    TimerEntry::Countdown {
                        id: String::new(),
                        deadline,
                        created: canonical_timestamp(created).map_err(Error::Input)?,
                    }
                } else {
                    TimerEntry::Normal {
                        id: String::new(),
                        deadline,
                    }
                },
            )
            .map_err(Error::Runtime)
        }
        crate::cli::TimerCommand::List => list_timers(&directory).map_err(Error::Runtime),
        crate::cli::TimerCommand::Prune => prune_timers(&directory).map_err(Error::Runtime),
        crate::cli::TimerCommand::RemoveId(id) => {
            remove_timer_id(&directory, id).map_err(Error::Runtime)
        }
        crate::cli::TimerCommand::RemoveDate(date) => {
            remove_date(&directory, date).map_err(Error::Runtime)
        }
        crate::cli::TimerCommand::Reset => reset_timers(&directory).map_err(Error::Runtime),
    }
}

pub(crate) fn run_trigger(command: &crate::cli::TriggerCommand) -> Result<String, Error> {
    let style = match command {
        crate::cli::TriggerCommand::Named(name) => {
            validate_id(name).map_err(|_| {
                Error::Input("NAME must match [A-Za-z0-9][A-Za-z0-9_-]{0,63}".into())
            })?;
            crate::core::Config::load()
                .map_err(Error::Runtime)?
                .named_style(name)
                .ok_or_else(|| Error::Input(format!("trigger {name} is not configured")))?
        }
        crate::cli::TriggerCommand::Direct {
            edge,
            start,
            end,
            thickness_px,
            duration,
            animation,
            fade,
            color,
        } => crate::core::style_from_record(&[
            edge.as_str(),
            start.as_str(),
            end.as_str(),
            thickness_px.as_str(),
            duration.as_str(),
            animation.as_str(),
            fade.as_str(),
            color.as_str(),
        ])
        .map_err(Error::Input)?,
    };
    let base = state_base().map_err(Error::Runtime)?;
    let shell = prepare_shell(&base).map_err(Error::Runtime)?;
    let (_lock, directory) = lock_directory(&shell, "triggers").map_err(Error::Runtime)?;
    add_trigger(
        &directory,
        None,
        &TriggerEntry {
            id: String::new(),
            created: canonical_timestamp(OffsetDateTime::now_utc()).map_err(Error::Input)?,
            style,
        },
    )
    .map(|_| String::new())
    .map_err(Error::Runtime)
}

pub(crate) fn snapshot_timers(
    style: crate::core::TimerConfig,
    shell_started_ns: i128,
) -> Result<Vec<ActiveHighlight>, String> {
    snapshot_timers_at(&state_base()?, style, shell_started_ns)
}

pub(crate) fn snapshot_triggers() -> Result<Vec<ActiveHighlight>, String> {
    snapshot_triggers_at(&state_base()?)
}

fn snapshot_timers_at(
    base: &Path,
    style: crate::core::TimerConfig,
    shell_started_ns: i128,
) -> Result<Vec<ActiveHighlight>, String> {
    let Some(directory) = snapshot_directory(base, "timers")? else {
        return Ok(Vec::new());
    };
    let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let mut active = timer_entries(&directory)?
        .into_iter()
        .filter_map(|entry| timer_highlight(entry, style, shell_started_ns, now))
        .collect::<Vec<_>>();
    sort_active(&mut active);
    Ok(active)
}

fn timer_highlight(
    entry: TimerEntry,
    style: crate::core::TimerConfig,
    shell_started_ns: i128,
    now: i128,
) -> Option<ActiveHighlight> {
    let (id, started, expires) = match entry {
        TimerEntry::Normal { id, deadline } => {
            let started = parse_date(&deadline).ok()?.unix_timestamp_nanos();
            if started < shell_started_ns {
                return None;
            }
            (
                id,
                started,
                started.checked_add(i128::from(style.duration_seconds) * 1_000_000_000)?,
            )
        }
        TimerEntry::Countdown {
            id,
            deadline,
            created,
        } => (
            id,
            parse_timestamp(&created).ok()?.unix_timestamp_nanos(),
            parse_date(&deadline).ok()?.unix_timestamp_nanos(),
        ),
    };
    (started <= now && now < expires).then_some(ActiveHighlight {
        started_ns: started,
        expires_ns: expires,
        id,
        style: None,
    })
}

fn snapshot_triggers_at(base: &Path) -> Result<Vec<ActiveHighlight>, String> {
    let Some(directory) = snapshot_directory(base, "triggers")? else {
        return Ok(Vec::new());
    };
    let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let mut active = trigger_entries(&directory)?
        .into_iter()
        .filter_map(|entry| {
            let started = parse_timestamp(&entry.created).ok()?.unix_timestamp_nanos();
            let expires =
                started.checked_add(i128::from(entry.style.duration_seconds) * 1_000_000_000)?;
            (started <= now && now < expires).then_some(ActiveHighlight {
                started_ns: started,
                expires_ns: expires,
                id: entry.id,
                style: Some(entry.style),
            })
        })
        .collect::<Vec<_>>();
    sort_active(&mut active);
    Ok(active)
}

fn sort_active(active: &mut [ActiveHighlight]) {
    active.sort_by(|left, right| {
        left.started_ns
            .cmp(&right.started_ns)
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn snapshot_directory(base: &Path, kind: &str) -> Result<Option<PathBuf>, String> {
    let owner = base.join("reEnvisioning");
    let shell = owner.join("temporalShell");
    let directory = shell.join(kind);
    for (path, private) in [
        (base, false),
        (&owner, true),
        (&shell, true),
        (&directory, true),
    ] {
        if readable_directory(path, private)?.is_none() {
            return Ok(None);
        }
    }
    Ok(Some(directory))
}

fn validate_timer(command: &crate::cli::TimerCommand) -> Result<(), Error> {
    match command {
        crate::cli::TimerCommand::AddDuration { duration, id, .. } => {
            duration_deadline(
                OffsetDateTime::now_utc(),
                parse_duration(duration).map_err(Error::Input)?,
            )
            .ok_or_else(|| Error::Input("duration is out of range".into()))?;
            if let Some(id) = id {
                validate_id(id).map_err(Error::Input)?;
            }
        }
        crate::cli::TimerCommand::AddDate { date, id, .. } => {
            parse_future_date(date).map_err(Error::Input)?;
            if let Some(id) = id {
                validate_id(id).map_err(Error::Input)?;
            }
        }
        crate::cli::TimerCommand::RemoveId(id) => validate_id(id).map_err(Error::Input)?,
        crate::cli::TimerCommand::RemoveDate(date) => validate_date(date).map_err(Error::Input)?,
        crate::cli::TimerCommand::List
        | crate::cli::TimerCommand::Prune
        | crate::cli::TimerCommand::Reset => {}
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
        Err(format!("{name} must be a non-empty absolute path"))
    } else {
        Ok(path)
    }
}

fn prepare_shell(base: &Path) -> Result<PathBuf, String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(base)
        .map_err(|error| format!("cannot create {}: {error}", base.display()))?;
    readable_directory(base, false)?.ok_or_else(|| format!("{} disappeared", base.display()))?;
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
    readable_directory(path, true)?.ok_or_else(|| format!("{} disappeared", path.display()))
}

fn readable_directory(path: &Path, private: bool) -> Result<Option<()>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{} must be a directory", path.display()));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & if private { 0o077 } else { 0o022 } != 0 {
        return Err(format!("{} has unsafe permissions", path.display()));
    }
    Ok(Some(()))
}

fn lock_directory(shell: &Path, name: &str) -> Result<(File, PathBuf), String> {
    let lock = open_lock(&shell.join(format!(".{name}.lock")))?;
    lock.lock()
        .map_err(|error| format!("cannot lock {}: {error}", shell.display()))?;
    let directory = shell.join(name);
    private_directory(&directory)?;
    Ok((lock, directory))
}

fn open_lock(path: &Path) -> Result<File, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!("{} must be a regular file", path.display()))
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
        .open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let path_metadata = fs::symlink_metadata(path)
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

fn add_timer(
    directory: &Path,
    requested_id: Option<&str>,
    entry: &TimerEntry,
) -> Result<String, String> {
    timer_entries(directory)?;
    ensure_publication_room(directory)?;
    let id = add(directory, requested_id, &serialize_timer(entry))?;
    let deadline = match entry {
        TimerEntry::Normal { deadline, .. } | TimerEntry::Countdown { deadline, .. } => deadline,
    };
    Ok(format!("{id}\t{deadline}\n"))
}

fn add_trigger(
    directory: &Path,
    requested_id: Option<&str>,
    entry: &TriggerEntry,
) -> Result<String, String> {
    cleanup_expired_triggers(directory)?;
    trigger_entries(directory)?;
    ensure_publication_room(directory)?;
    let id = add(directory, requested_id, &serialize_trigger(entry))?;
    Ok(format!("{id}\n"))
}

fn cleanup_expired_triggers(directory: &Path) -> Result<(), String> {
    let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let mut expired = Vec::new();
    for entry in trigger_entries(directory)? {
        let started = parse_timestamp(&entry.created)
            .map_err(|_| format!("{} has invalid state", directory.join(&entry.id).display()))?
            .unix_timestamp_nanos();
        let expires = started
            .checked_add(i128::from(entry.style.duration_seconds) * 1_000_000_000)
            .ok_or_else(|| format!("{} has invalid state", directory.join(&entry.id).display()))?;
        if expires <= now {
            expired.push(entry.id);
        }
    }
    if !expired.is_empty() {
        for id in expired {
            fs::remove_file(directory.join(id))
                .map_err(|error| format!("cannot remove expired trigger: {error}"))?;
        }
        sync_directory(directory)?;
    }
    Ok(())
}

fn ensure_publication_room(directory: &Path) -> Result<(), String> {
    let mut count = 0;
    for item in fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
    {
        item.map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        count += 1;
        if count >= MAX_ENTRIES - 1 {
            return Err(format!(
                "{}/ has reached its safe {MAX_ENTRIES}-entry publication limit",
                directory.display()
            ));
        }
    }
    Ok(())
}

fn add(directory: &Path, requested_id: Option<&str>, contents: &str) -> Result<String, String> {
    let id = match requested_id {
        Some(id) if target_available(&directory.join(id))? => id.to_owned(),
        Some(id) => return Err(format!("entry {id} already exists")),
        None => automatic_id(directory)?,
    };
    let temporary = directory.join(format!(".{id}.tmp"));
    let target = directory.join(&id);
    remove_stale_temp(&temporary)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot write {}: {error}", target.display()));
    }
    match fs::hard_link(&temporary, &target) {
        Ok(()) => {
            let _ = fs::remove_file(&temporary);
            sync_directory(directory)?;
            Ok(id)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Err(format!("entry {id} already exists"))
            } else {
                Err(format!("cannot store {}: {error}", target.display()))
            }
        }
    }
}

fn remove_stale_temp(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => {
            validate_temp(path)?;
            fs::remove_file(path)
                .map_err(|error| format!("cannot remove stale {}: {error}", path.display()))
        }
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
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

fn automatic_id(directory: &Path) -> Result<String, String> {
    for _ in 0..AUTO_ID_ATTEMPTS {
        let id = random_id()?;
        if target_available(&directory.join(&id))? {
            return Ok(id);
        }
    }
    Err("cannot generate an unused ID".into())
}

fn random_id() -> Result<String, String> {
    for _ in 0..AUTO_ID_ATTEMPTS {
        let mut bytes = [0; AUTO_ID_LENGTH * 2];
        getrandom::fill(&mut bytes).map_err(|error| format!("cannot generate ID: {error}"))?;
        let id: String = bytes
            .iter()
            .filter(|byte| **byte < 252)
            .map(|byte| ID_ALPHABET[usize::from(*byte % ID_ALPHABET.len() as u8)] as char)
            .take(AUTO_ID_LENGTH)
            .collect();
        if id.len() == AUTO_ID_LENGTH {
            return Ok(id);
        }
    }
    Err("cannot generate ID".into())
}

fn timer_entries(directory: &Path) -> Result<Vec<TimerEntry>, String> {
    records(directory, |id, text| {
        let fields: Vec<_> = line(text)?.split('\t').collect();
        match fields.as_slice() {
            [deadline] => {
                validate_date(deadline)?;
                Ok(TimerEntry::Normal {
                    id: id.into(),
                    deadline: (*deadline).into(),
                })
            }
            [deadline, created] => {
                validate_date(deadline)?;
                validate_timestamp(created)?;
                Ok(TimerEntry::Countdown {
                    id: id.into(),
                    deadline: (*deadline).into(),
                    created: (*created).into(),
                })
            }
            _ => Err("fields".into()),
        }
    })
}

fn trigger_entries(directory: &Path) -> Result<Vec<TriggerEntry>, String> {
    records(directory, |id, text| {
        let fields: Vec<_> = line(text)?.split('\t').collect();
        let (created, style) = fields.split_first().ok_or("fields")?;
        validate_timestamp(created)?;
        Ok(TriggerEntry {
            id: id.into(),
            created: (*created).into(),
            style: crate::core::style_from_record(style)?,
        })
    })
}

fn records<T>(
    directory: &Path,
    parse: impl Fn(&str, &str) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let mut records = Vec::new();
    for (index, item) in fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
        .enumerate()
    {
        if index >= MAX_ENTRIES {
            return Err(format!(
                "{} contains more than {MAX_ENTRIES} entries",
                directory.display()
            ));
        }
        let item = item.map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        let id = item
            .file_name()
            .into_string()
            .map_err(|_| format!("{} contains a non-UTF-8 entry", directory.display()))?;
        let path = item.path();
        if is_temp(&id) {
            validate_temp(&path)?;
            continue;
        }
        validate_id(&id)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!("{} must be a regular file", path.display()));
        }
        #[cfg(unix)]
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!("{} must be private", path.display()));
        }
        let file = File::open(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let opened = file
            .metadata()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if !opened.is_file() || {
            #[cfg(unix)]
            {
                !same_file(&metadata, &opened)
            }
            #[cfg(not(unix))]
            {
                false
            }
        } {
            return Err(format!("{} changed while opening", path.display()));
        }
        let mut text = String::new();
        file.take(MAX_ENTRY_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if text.len() as u64 > MAX_ENTRY_BYTES {
            return Err(format!("{} is oversized", path.display()));
        }
        records
            .push(parse(&id, &text).map_err(|_| format!("{} has invalid state", path.display()))?);
    }
    Ok(records)
}

fn line(text: &str) -> Result<&str, String> {
    let text = text.strip_suffix('\n').ok_or("newline")?;
    if text.contains('\n') {
        Err("line".into())
    } else {
        Ok(text)
    }
}

fn validate_temp(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "{} must be a regular stale temporary",
            path.display()
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!("{} must be private", path.display()));
    }
    Ok(())
}

fn is_temp(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(|id| validate_id(id).is_ok())
}

fn serialize_timer(entry: &TimerEntry) -> String {
    match entry {
        TimerEntry::Normal { deadline, .. } => format!("{deadline}\n"),
        TimerEntry::Countdown {
            deadline, created, ..
        } => format!("{deadline}\t{created}\n"),
    }
}

fn serialize_trigger(entry: &TriggerEntry) -> String {
    format!(
        "{}\t{}\n",
        entry.created,
        crate::core::style_record(entry.style)
    )
}

fn list_timers(directory: &Path) -> Result<String, String> {
    let mut entries = timer_entries(directory)?;
    entries.sort_by(|left, right| {
        timer_deadline(left)
            .cmp(timer_deadline(right))
            .then_with(|| timer_id(left).cmp(timer_id(right)))
    });
    Ok(entries
        .into_iter()
        .map(|entry| format!("{}\t{}\n", timer_id(&entry), timer_deadline(&entry)))
        .collect())
}
fn timer_id(entry: &TimerEntry) -> &str {
    match entry {
        TimerEntry::Normal { id, .. } | TimerEntry::Countdown { id, .. } => id,
    }
}
fn timer_deadline(entry: &TimerEntry) -> &str {
    match entry {
        TimerEntry::Normal { deadline, .. } | TimerEntry::Countdown { deadline, .. } => deadline,
    }
}

fn prune_timers(directory: &Path) -> Result<String, String> {
    prune_timers_at(directory, OffsetDateTime::now_utc())
}

fn prune_timers_at(directory: &Path, now: OffsetDateTime) -> Result<String, String> {
    let entries = timer_entries(directory)?;
    let mut removed = false;
    for entry in entries
        .iter()
        .filter(|entry| parse_date(timer_deadline(entry)).is_ok_and(|deadline| deadline <= now))
    {
        fs::remove_file(directory.join(timer_id(entry))).map_err(|error| {
            format!(
                "cannot prune timer {}: {error}; prune may be partial",
                timer_id(entry)
            )
        })?;
        removed = true;
    }
    sync_directory(directory).map_err(|error| {
        if removed {
            format!("{error}; prune may be partial")
        } else {
            error
        }
    })?;
    Ok(String::new())
}

fn remove_timer_id(directory: &Path, id: &str) -> Result<String, String> {
    if !timer_entries(directory)?
        .iter()
        .any(|entry| timer_id(entry) == id)
    {
        return Err(format!("timer {id} does not exist"));
    }
    fs::remove_file(directory.join(id))
        .map_err(|error| format!("cannot remove timer {id}: {error}"))?;
    sync_directory(directory)?;
    Ok(String::new())
}

fn remove_date(directory: &Path, date: &str) -> Result<String, String> {
    let mut matches: Vec<_> = timer_entries(directory)?
        .into_iter()
        .filter(|entry| timer_deadline(entry) == date)
        .collect();
    if matches.is_empty() {
        return Err(format!("no timer has date {date}"));
    }
    matches.sort_by(|left, right| timer_id(left).cmp(timer_id(right)));
    for entry in &matches {
        fs::remove_file(directory.join(timer_id(entry))).map_err(|error| {
            format!(
                "cannot remove timer {}: {error}; removal may be partial",
                timer_id(entry)
            )
        })?;
    }
    sync_directory(directory)?;
    Ok(String::new())
}

fn reset_timers(directory: &Path) -> Result<String, String> {
    let mut paths: Vec<_> = timer_entries(directory)?
        .into_iter()
        .map(|entry| directory.join(timer_id(&entry)))
        .collect();
    for item in fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
    {
        let item = item.map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        if is_temp(&item.file_name().to_string_lossy()) {
            paths.push(item.path());
        }
    }
    for path in paths {
        fs::remove_file(&path).map_err(|error| {
            format!(
                "cannot reset {}: {error}; reset may be partial",
                path.display()
            )
        })?;
    }
    sync_directory(directory)?;
    Ok(String::new())
}

pub(crate) fn parse_duration(value: &str) -> Result<i64, String> {
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
    let date = parse_date(value)?;
    if date <= OffsetDateTime::now_utc() {
        Err("date must be in the future".into())
    } else {
        Ok(date)
    }
}
fn validate_date(value: &str) -> Result<(), String> {
    parse_date(value).map(|_| ())
}
fn parse_date(value: &str) -> Result<OffsetDateTime, String> {
    if value.len() != 20
        || !matches!(value.as_bytes(), [a,b,c,d,b'-',f,g,b'-',i,j,b'T',l,m,b':',o,p,b':',r,s,b'Z'] if [*a,*b,*c,*d,*f,*g,*i,*j,*l,*m,*o,*p,*r,*s].iter().all(u8::is_ascii_digit))
    {
        return Err("date must be YYYY-MM-DDTHH:MM:SSZ in UTC".into());
    }
    let date = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| "date must be a valid UTC RFC3339 timestamp")?;
    if canonical(date)? != value {
        Err("date must be YYYY-MM-DDTHH:MM:SSZ in UTC".into())
    } else {
        Ok(date)
    }
}
fn canonical(date: OffsetDateTime) -> Result<String, String> {
    if !(0..=9999).contains(&date.year()) || date.nanosecond() != 0 {
        return Err("deadline is outside the supported second-precision UTC range".into());
    }
    date.format(&Rfc3339)
        .map_err(|_| "deadline is outside the supported UTC format".into())
}
fn canonical_timestamp(date: OffsetDateTime) -> Result<String, String> {
    date.format(&Rfc3339)
        .map_err(|_| "time is outside the supported UTC format".into())
}
fn validate_timestamp(value: &str) -> Result<(), String> {
    parse_timestamp(value).map(|_| ())
}
fn parse_timestamp(value: &str) -> Result<OffsetDateTime, String> {
    value
        .ends_with('Z')
        .then_some(())
        .ok_or_else(|| "time must be a UTC RFC3339 timestamp".to_owned())
        .and_then(|()| {
            OffsetDateTime::parse(value, &Rfc3339)
                .map_err(|_| "time must be a UTC RFC3339 timestamp".into())
        })
}
fn validate_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    if !(1..=64).contains(&bytes.len())
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Err("ID must match [A-Za-z0-9][A-Za-z0-9_-]{0,63}".into())
    } else {
        Ok(())
    }
}
fn sync_parent(path: &Path) -> Result<(), String> {
    path.parent().map(sync_directory).transpose()?;
    Ok(())
}
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NUMBER: AtomicUsize = AtomicUsize::new(0);
    fn base() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "temporalshell-state-{}",
            NUMBER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn write_record(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn records_preserve_legacy_normal_and_countdown() {
        let directory = base();
        fs::write(directory.join("normal"), "2096-02-29T12:34:56Z\n").unwrap();
        fs::write(
            directory.join("countdown"),
            "2096-02-29T12:34:56Z\t2096-02-29T12:00:00Z\n",
        )
        .unwrap();
        #[cfg(unix)]
        for path in [directory.join("normal"), directory.join("countdown")] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(
            list_timers(&directory).unwrap(),
            "countdown\t2096-02-29T12:34:56Z\nnormal\t2096-02-29T12:34:56Z\n"
        );
        assert_eq!(remove_timer_id(&directory, "normal").unwrap(), "");
        assert_eq!(remove_date(&directory, "2096-02-29T12:34:56Z").unwrap(), "");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn normal_waits_for_deadline_while_countdown_uses_its_whole_span() {
        let style = crate::core::TimerConfig {
            duration_seconds: 10,
            ..crate::core::Config::default().timer
        };
        let deadline = "2096-02-29T12:34:56Z";
        let deadline_ns = parse_date(deadline).unwrap().unix_timestamp_nanos();
        assert!(timer_highlight(
            TimerEntry::Normal {
                id: "n".into(),
                deadline: deadline.into()
            },
            style,
            deadline_ns - 1,
            deadline_ns - 1
        )
        .is_none());
        assert!(timer_highlight(
            TimerEntry::Normal {
                id: "n".into(),
                deadline: deadline.into()
            },
            style,
            deadline_ns - 1,
            deadline_ns
        )
        .is_some());
        assert!(timer_highlight(
            TimerEntry::Normal {
                id: "n".into(),
                deadline: deadline.into()
            },
            style,
            deadline_ns,
            deadline_ns
        )
        .is_some());
        assert!(timer_highlight(
            TimerEntry::Normal {
                id: "n".into(),
                deadline: deadline.into()
            },
            style,
            deadline_ns + 1,
            deadline_ns + 1
        )
        .is_none());
        assert!(timer_highlight(
            TimerEntry::Normal {
                id: "n".into(),
                deadline: deadline.into()
            },
            style,
            deadline_ns,
            deadline_ns + 10_000_000_000
        )
        .is_none());
        let created = "2096-02-29T12:34:46Z";
        let created_ns = parse_date(created).unwrap().unix_timestamp_nanos();
        let countdown = timer_highlight(
            TimerEntry::Countdown {
                id: "c".into(),
                deadline: deadline.into(),
                created: created.into(),
            },
            style,
            deadline_ns - 1,
            deadline_ns - 1,
        )
        .unwrap();
        assert_eq!(countdown.started_ns, created_ns);
        assert!(timer_highlight(
            TimerEntry::Countdown {
                id: "c".into(),
                deadline: deadline.into(),
                created: created.into()
            },
            style,
            deadline_ns + 1,
            deadline_ns
        )
        .is_none());
    }

    #[test]
    fn prune_removes_expired_normal_and_countdown_timers() {
        let directory = base();
        write_record(&directory.join("normal-old"), "2096-02-29T12:34:56Z\n");
        write_record(
            &directory.join("countdown-old"),
            "2096-02-29T12:34:56Z\t2096-02-29T12:00:00Z\n",
        );
        write_record(&directory.join("normal-new"), "2096-03-01T12:34:56Z\n");
        write_record(
            &directory.join("countdown-new"),
            "2096-03-01T12:34:56Z\t2096-02-29T12:00:00Z\n",
        );
        let now = parse_date("2096-02-29T12:34:56Z").unwrap();
        assert_eq!(prune_timers_at(&directory, now).unwrap(), "");
        assert!(!directory.join("normal-old").exists());
        assert!(!directory.join("countdown-old").exists());
        assert!(directory.join("normal-new").exists());
        assert!(directory.join("countdown-new").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_state_blocks_prune_before_deletion() {
        let directory = base();
        write_record(&directory.join("expired"), "2000-01-01T00:00:00Z\n");
        write_record(&directory.join("invalid"), "not-a-date\n");
        assert!(prune_timers_at(&directory, OffsetDateTime::now_utc()).is_err());
        assert!(directory.join("expired").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn publication_cap_rejects_a_4097th_timer_without_cleanup() {
        let directory = base();
        for index in 0..MAX_ENTRIES {
            write_record(
                &directory.join(format!("timer{index}")),
                "2000-01-01T00:00:00Z\n",
            );
        }
        assert!(add_timer(
            &directory,
            Some("new"),
            &TimerEntry::Normal {
                id: String::new(),
                deadline: "2096-02-29T12:34:56Z".into(),
            },
        )
        .is_err());
        assert_eq!(timer_entries(&directory).unwrap().len(), MAX_ENTRIES);
        assert!(!directory.join("new").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn trigger_publication_cleans_only_expired_records_before_capping() {
        let directory = base();
        let style = crate::core::Config::default().timer;
        let active = TriggerEntry {
            id: String::new(),
            created: canonical_timestamp(OffsetDateTime::now_utc()).unwrap(),
            style,
        };
        for id in ["expired-one", "expired-two"] {
            write_record(
                &directory.join(id),
                &format!(
                    "2000-01-01T00:00:00Z\t{}\n",
                    crate::core::style_record(style)
                ),
            );
        }
        for index in 0..MAX_ENTRIES - 2 {
            write_record(
                &directory.join(format!("active{index}")),
                &serialize_trigger(&active),
            );
        }
        add_trigger(&directory, Some("new"), &active).unwrap();
        assert!(!directory.join("expired-one").exists());
        assert!(!directory.join("expired-two").exists());
        assert!(directory.join("active0").exists());
        assert!(directory.join("new").exists());
        assert_eq!(trigger_entries(&directory).unwrap().len(), MAX_ENTRIES - 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn timer_and_trigger_snapshots_expire_independently() {
        let base = base();
        let shell = prepare_shell(&base).unwrap();
        let (_, timers) = lock_directory(&shell, "timers").unwrap();
        let (_, triggers) = lock_directory(&shell, "triggers").unwrap();
        fs::write(timers.join("old"), "2000-01-01T00:00:00Z\n").unwrap();
        fs::write(
            triggers.join("old"),
            format!(
                "2000-01-01T00:00:00Z\t{}\n",
                crate::core::style_record(crate::core::Config::default().timer)
            ),
        )
        .unwrap();
        #[cfg(unix)]
        for path in [timers.join("old"), triggers.join("old")] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(snapshot_timers_at(
            &base,
            crate::core::Config::default().timer,
            OffsetDateTime::now_utc().unix_timestamp_nanos(),
        )
        .unwrap()
        .is_empty());
        assert!(snapshot_triggers_at(&base).unwrap().is_empty());
        assert!(timers.join("old").exists());
        assert!(triggers.join("old").exists());
        fs::remove_dir_all(base).unwrap();
    }
}
