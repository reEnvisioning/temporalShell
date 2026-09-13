use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

const MAX_CONFIG_VALUE: u32 = 256;
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_HIGHLIGHTS: usize = 64;
const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const SHADOW_WIDTH: u32 = 3;
const SHADOW: [u32; SHADOW_WIDTH as usize] = [0x4d, 0x33, 0x1a];
const FADE_NS: i128 = 250_000_000;
#[cfg(test)]
const SECOND_NS: i128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coordinate {
    Percent(u32),
    Pixels(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Animation {
    Expand,
    FlowUp,
    FlowDown,
    Static,
    Blink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimerConfig {
    pub(crate) edge: Edge,
    pub(crate) start: Coordinate,
    pub(crate) end: Coordinate,
    pub(crate) thickness_px: u32,
    pub(crate) duration_seconds: i64,
    pub(crate) animation: Animation,
    pub(crate) fade: bool,
    pub(crate) color: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) border_thickness_px: u32,
    pub(crate) corner_radius_px: u32,
    pub(crate) shadow_strength_percent: u32,
    pub(crate) shadow_color: u32,
    pub(crate) border_color: u32,
    pub(crate) timer: TimerConfig,
    highlights: Vec<(String, TimerConfig)>,
}

const DEFAULT_CONFIG: &str = include_str!("../default.toml");

impl Default for Config {
    fn default() -> Self {
        parse_config_over(DEFAULT_CONFIG, empty_config())
            .expect("tracked default.toml must be a complete valid config")
    }
}

impl Config {
    pub(crate) fn load() -> Result<Self, String> {
        load_at(&config_path()?)
    }

    pub(crate) fn load_or_create() -> Result<Self, String> {
        load_or_create_at(&config_path()?)
    }

    pub(crate) fn named_style(&self, name: &str) -> Option<TimerConfig> {
        self.highlights
            .iter()
            .find(|(item, _)| item == name)
            .map(|(_, style)| *style)
    }
}

fn empty_config() -> Config {
    Config {
        border_thickness_px: 0,
        corner_radius_px: 0,
        shadow_strength_percent: 0,
        shadow_color: 0,
        border_color: 0,
        timer: TimerConfig {
            edge: Edge::Top,
            start: Coordinate::Percent(0),
            end: Coordinate::Percent(0),
            thickness_px: 0,
            duration_seconds: 0,
            animation: Animation::Static,
            fade: false,
            color: 0,
        },
        highlights: Vec::new(),
    }
}

fn load_at(path: &Path) -> Result<Config, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    if !file
        .metadata()
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?
        .is_file()
    {
        return Err(format!("{} must be a regular file", path.display()));
    }
    let mut contents = String::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut contents)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if contents.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!("{} must be no larger than 64 KiB", path.display()));
    }
    parse_config(&contents).map_err(|error| format!("{}: {error}", path.display()))
}

fn load_or_create_at(path: &Path) -> Result<Config, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_default_at(path)?,
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    load_at(path)
}

fn create_default_at(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("config path has no parent")?;
    private_directory(parent)?;
    for _attempt in 0..100 {
        let temporary = parent.join(format!(".config.toml.{:x}.tmp", random_u64()?));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create {}: {error}", temporary.display())),
        };
        let result = file
            .write_all(DEFAULT_CONFIG.as_bytes())
            .and_then(|()| file.sync_all())
            .and_then(|()| fs::hard_link(&temporary, path))
            .and_then(|()| sync_directory(parent));
        let cleanup = fs::remove_file(&temporary);
        return match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(cleanup)) => Err(format!(
                "created {} but cannot remove temporary {}: {cleanup}",
                path.display(),
                temporary.display()
            )),
            (Err(error), Ok(())) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            (Err(error), Err(cleanup)) if error.kind() == io::ErrorKind::AlreadyExists => {
                Err(format!(
                    "another process created {} but cannot remove temporary {}: {cleanup}",
                    path.display(),
                    temporary.display()
                ))
            }
            (Err(error), Ok(())) => Err(format!("cannot create {}: {error}", path.display())),
            (Err(error), Err(cleanup)) => Err(format!(
                "cannot create {}: {error}; cannot remove temporary {}: {cleanup}",
                path.display(),
                temporary.display()
            )),
        };
    }
    Err(format!(
        "cannot create a unique temporary config in {}",
        parent.display()
    ))
}

fn random_u64() -> Result<u64, String> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("cannot generate config temporary name: {error}"))?;
    Ok(u64::from_ne_bytes(bytes))
}

fn private_directory(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if parent != path {
            private_directory(parent)?;
        }
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => return Err(format!("{} must be a directory", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => sync_directory(path.parent().ok_or("config directory has no parent")?)
            .map_err(|error| format!("cannot sync {}: {error}", path.display()))?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
    };
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(format!("{} must be a directory", path.display())),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn sync_directory(path: &Path) -> Result<(), io::Error> {
    File::open(path)?.sync_all()
}

fn config_path() -> Result<PathBuf, String> {
    let base = match env::var_os("XDG_CONFIG_HOME") {
        Some(value) => absolute_path("XDG_CONFIG_HOME", value)?,
        None => absolute_path(
            "HOME",
            env::var_os("HOME").ok_or("set XDG_CONFIG_HOME or HOME")?,
        )?
        .join(".config"),
    };
    Ok(base.join("reEnvisioning/temporalShell/config.toml"))
}

fn absolute_path(name: &str, value: std::ffi::OsString) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(format!("{name} must be a non-empty absolute path"));
    }
    Ok(path)
}

fn parse_config(text: &str) -> Result<Config, String> {
    parse_config_over(text, Config::default())
}

fn parse_config_over(text: &str, mut config: Config) -> Result<Config, String> {
    enum Table {
        Root,
        Timer,
        Highlight(String, TimerConfig, Vec<String>),
    }
    let mut table = Table::Root;
    let mut seen_root = Vec::new();
    let mut seen_timer = Vec::new();
    let mut alias = false;
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = strip_comment(raw, line_number)?.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if let Table::Highlight(name, style, seen) = &table {
                complete_style(style, seen, line_number)?;
                config.highlights.push((name.clone(), *style));
            }
            table = if line == "[timer]" && seen_timer.is_empty() && config.highlights.is_empty() {
                Table::Timer
            } else if let Some(name) = line
                .strip_prefix("[highlight.")
                .and_then(|value| value.strip_suffix(']'))
            {
                if validate_highlight_name(name).is_err()
                    || name == "default"
                    || config.highlights.len() >= MAX_HIGHLIGHTS
                    || config.named_style(name).is_some()
                    || matches!(&table, Table::Highlight(old, _, _) if old == name)
                {
                    return Err(format!("line {line_number}: unknown or duplicate table"));
                }
                Table::Highlight(name.to_owned(), empty_config().timer, Vec::new())
            } else {
                return Err(format!("line {line_number}: unknown or duplicate table"));
            };
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {line_number}: expected key = value"))?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() || value.contains('=') {
            return Err(format!("line {line_number}: expected key = value"));
        }
        match &mut table {
            Table::Root => {
                if seen_root.iter().any(|item| item == key) {
                    return Err(format!("line {line_number}: duplicate key {key}"));
                }
                seen_root.push(key.to_owned());
                match key {
                    "border_thickness_px" => {
                        config.border_thickness_px = positive_integer(value, key, line_number)?
                    }
                    "event_line_thickness_px" => {
                        alias = true;
                        config.timer.thickness_px = positive_integer(value, key, line_number)?;
                    }
                    "corner_radius_px" => {
                        config.corner_radius_px = integer(value, key, line_number)?;
                        if config.corner_radius_px > MAX_CONFIG_VALUE {
                            return Err(format!(
                                "line {line_number}: {key} must be between 0 and {MAX_CONFIG_VALUE}"
                            ));
                        }
                    }
                    "shadow_strength_percent" => {
                        config.shadow_strength_percent = integer(value, key, line_number)?;
                        if config.shadow_strength_percent > 100 {
                            return Err(format!(
                                "line {line_number}: {key} must be between 0 and 100"
                            ));
                        }
                    }
                    "shadow_color" => config.shadow_color = color(value, key, line_number)?,
                    "border_color" => config.border_color = color(value, key, line_number)?,
                    _ => return Err(format!("line {line_number}: unknown root key {key}")),
                }
            }
            Table::Timer => {
                if seen_timer.iter().any(|item| item == key) {
                    return Err(format!("line {line_number}: duplicate key {key}"));
                }
                if alias && key == "thickness_px" {
                    return Err(
                        "event_line_thickness_px and timer.thickness_px cannot both be set".into(),
                    );
                }
                seen_timer.push(key.to_owned());
                apply_style_key(&mut config.timer, key, value, line_number)?;
            }
            Table::Highlight(_, style, seen) => {
                if seen.iter().any(|item| item == key) {
                    return Err(format!("line {line_number}: duplicate key {key}"));
                }
                seen.push(key.to_owned());
                apply_style_key(style, key, value, line_number)?;
            }
        }
    }
    if let Table::Highlight(name, style, seen) = table {
        complete_style(&style, &seen, text.lines().count() + 1)?;
        config.highlights.push((name, style));
    }
    validate_style(config.timer, config.border_thickness_px)?;
    for (_, style) in &config.highlights {
        validate_style(*style, config.border_thickness_px)?;
    }
    Ok(config)
}

pub(crate) fn style_record(style: TimerConfig) -> String {
    let edge = match style.edge {
        Edge::Top => "top",
        Edge::Right => "right",
        Edge::Bottom => "bottom",
        Edge::Left => "left",
    };
    let coordinate = |value| match value {
        Coordinate::Percent(value) => format!("{value}%"),
        Coordinate::Pixels(value) => format!("{value}px"),
    };
    let animation = match style.animation {
        Animation::Expand => "expand",
        Animation::FlowUp => "flow-up",
        Animation::FlowDown => "flow-down",
        Animation::Static => "static",
        Animation::Blink => "blink",
    };
    format!(
        "{edge}\t{}\t{}\t{}\t{}s\t{animation}\t{}\t#{:06x}",
        coordinate(style.start),
        coordinate(style.end),
        style.thickness_px,
        style.duration_seconds,
        style.fade,
        style.color
    )
}

pub(crate) fn style_from_record(fields: &[&str]) -> Result<TimerConfig, String> {
    let [edge, start, end, thickness_px, duration, animation, fade, color_text] = fields else {
        return Err("trigger style fields".into());
    };
    let edge = match *edge {
        "top" => Edge::Top,
        "right" => Edge::Right,
        "bottom" => Edge::Bottom,
        "left" => Edge::Left,
        _ => return Err("trigger style edge".into()),
    };
    let start = coordinate(&format!("\"{start}\""), "start", 0)?;
    let end = coordinate(&format!("\"{end}\""), "end", 0)?;
    let thickness_px = positive_integer(thickness_px, "thickness_px", 0)?;
    let duration_seconds = crate::timer::parse_duration(duration)?;
    if duration_seconds > 86_400 {
        return Err("trigger style duration".into());
    }
    let animation = match *animation {
        "expand" => Animation::Expand,
        "flow-up" => Animation::FlowUp,
        "flow-down" => Animation::FlowDown,
        "static" => Animation::Static,
        "blink" => Animation::Blink,
        _ => return Err("trigger style animation".into()),
    };
    let fade = match *fade {
        "true" => true,
        "false" => false,
        _ => return Err("trigger style fade".into()),
    };
    let color = color(&format!("\"{color_text}\""), "color", 0)?;
    let style = TimerConfig {
        edge,
        start,
        end,
        thickness_px,
        duration_seconds,
        animation,
        fade,
        color,
    };
    validate_style(style, MAX_CONFIG_VALUE)?;
    Ok(style)
}

fn validate_highlight_name(name: &str) -> Result<(), ()> {
    let bytes = name.as_bytes();
    if !(1..=64).contains(&bytes.len())
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Err(())
    } else {
        Ok(())
    }
}

fn complete_style(style: &TimerConfig, seen: &[String], line: usize) -> Result<(), String> {
    for key in [
        "edge",
        "start",
        "end",
        "thickness_px",
        "duration",
        "animation",
        "fade",
        "color",
    ] {
        if !seen.iter().any(|item| item == key) {
            return Err(format!("line {line}: highlight table must contain {key}"));
        }
    }
    let _ = style;
    Ok(())
}

fn apply_style_key(
    style: &mut TimerConfig,
    key: &str,
    value: &str,
    line: usize,
) -> Result<(), String> {
    match key {
        "edge" => {
            style.edge = match quoted(value, key, line)? {
                "top" => Edge::Top,
                "right" => Edge::Right,
                "bottom" => Edge::Bottom,
                "left" => Edge::Left,
                _ => return Err(format!("line {line}: invalid timer edge")),
            }
        }
        "start" => style.start = coordinate(value, key, line)?,
        "end" => style.end = coordinate(value, key, line)?,
        "thickness_px" => style.thickness_px = positive_integer(value, key, line)?,
        "duration" => {
            let seconds = crate::timer::parse_duration(quoted(value, key, line)?)?;
            if seconds > 86_400 {
                return Err(format!("line {line}: timer duration must not exceed 1d"));
            }
            style.duration_seconds = seconds;
        }
        "animation" => {
            style.animation = match quoted(value, key, line)? {
                "expand" => Animation::Expand,
                "flow-up" => Animation::FlowUp,
                "flow-down" => Animation::FlowDown,
                "static" => Animation::Static,
                "blink" => Animation::Blink,
                _ => return Err(format!("line {line}: invalid timer animation")),
            }
        }
        "fade" => {
            style.fade = match value {
                "true" => true,
                "false" => false,
                _ => return Err(format!("line {line}: timer fade must be true or false")),
            }
        }
        "color" => style.color = color(value, key, line)?,
        _ => return Err(format!("line {line}: unknown timer key {key}")),
    };
    Ok(())
}

fn validate_style(style: TimerConfig, thickness: u32) -> Result<(), String> {
    if style.thickness_px > thickness {
        return Err("timer.thickness_px must be between 1 and border_thickness_px".into());
    }
    if matches!(style.animation, Animation::FlowUp | Animation::FlowDown)
        && matches!(style.edge, Edge::Top | Edge::Bottom)
    {
        return Err("flow-up and flow-down require timer edge left or right".into());
    }
    Ok(())
}

fn strip_comment(line: &str, number: usize) -> Result<&str, String> {
    let mut quoted = false;
    for (index, byte) in line.bytes().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b'#' if !quoted => return Ok(&line[..index]),
            _ => {}
        }
    }
    if quoted {
        Err(format!("line {number}: unterminated quoted string"))
    } else {
        Ok(line)
    }
}

fn quoted<'a>(value: &'a str, key: &str, line: usize) -> Result<&'a str, String> {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .filter(|value| !value.contains(['"', '\\', '\n', '\r']))
        .ok_or_else(|| format!("line {line}: {key} must be a simple quoted string"))
}

fn integer(value: &str, key: &str, line: usize) -> Result<u32, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("line {line}: {key} must be an integer"));
    }
    value
        .parse()
        .map_err(|_| format!("line {line}: {key} is out of range"))
}

fn positive_integer(value: &str, key: &str, line: usize) -> Result<u32, String> {
    let number = integer(value, key, line)?;
    if !(1..=MAX_CONFIG_VALUE).contains(&number) {
        return Err(format!(
            "line {line}: {key} must be between 1 and {MAX_CONFIG_VALUE}"
        ));
    }
    Ok(number)
}

fn color(value: &str, key: &str, line: usize) -> Result<u32, String> {
    let value = quoted(value, key, line)?;
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!("line {line}: {key} must be quoted #RRGGBB"));
    }
    u32::from_str_radix(&value[1..], 16).map_err(|_| format!("line {line}: invalid {key}"))
}

fn coordinate(value: &str, key: &str, line: usize) -> Result<Coordinate, String> {
    let value = quoted(value, key, line)?;
    if let Some(digits) = value.strip_suffix('%') {
        let number = integer(digits, key, line)?;
        if number <= 100 {
            return Ok(Coordinate::Percent(number));
        }
    } else if let Some(digits) = value.strip_suffix("px") {
        let number = integer(digits, key, line)?;
        return Ok(Coordinate::Pixels(number));
    }
    Err(format!(
        "line {line}: {key} must be exactly N% (0..100) or nonnegative Npx"
    ))
}

pub(crate) fn buffer_dimensions(
    width: u32,
    height: u32,
    scale: i32,
) -> Result<(i32, i32, i32, usize), String> {
    let scale =
        u32::try_from(scale).map_err(|_| "compositor supplied a non-positive buffer scale")?;
    let width = width.checked_mul(scale).ok_or("buffer width overflow")?;
    let height = height.checked_mul(scale).ok_or("buffer height overflow")?;
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err("invalid buffer dimensions".into());
    }
    let stride = width.checked_mul(4).ok_or("buffer stride overflow")?;
    let bytes = usize::try_from(height)
        .ok()
        .and_then(|height| usize::try_from(stride).ok()?.checked_mul(height))
        .ok_or("buffer size overflow")?;
    if bytes > MAX_BUFFER_BYTES {
        return Err("buffer exceeds the 64 MiB safety limit".into());
    }
    Ok((width as i32, height as i32, stride as i32, bytes))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimerFrame {
    pub(crate) started_ns: i128,
    pub(crate) deadline_ns: i128,
    pub(crate) now_ns: i128,
}

#[cfg(test)]
pub(crate) fn paint(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    scale: u32,
    config: &Config,
    timer: Option<TimerFrame>,
) -> Result<(), String> {
    paint_frames(
        canvas,
        width,
        height,
        scale,
        config,
        timer.map(|frame| (config.timer, frame)).into_iter(),
    )
}

pub(crate) fn paint_highlights(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    scale: u32,
    config: &Config,
    highlights: &[crate::timer::ActiveHighlight],
    now_ns: i128,
) -> Result<(), String> {
    paint_frames(
        canvas,
        width,
        height,
        scale,
        config,
        highlights
            .iter()
            .filter(|highlight| now_ns < highlight.expires_ns)
            .map(|highlight| {
                let timer = highlight.style.unwrap_or(config.timer);
                (
                    timer,
                    TimerFrame {
                        started_ns: highlight.started_ns,
                        deadline_ns: highlight.expires_ns,
                        now_ns,
                    },
                )
            }),
    )
}

fn paint_frames(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    scale: u32,
    config: &Config,
    frames: impl Iterator<Item = (TimerConfig, TimerFrame)>,
) -> Result<(), String> {
    let logical_width = width / scale;
    let logical_height = height / scale;
    let frames: Vec<_> = frames
        .filter_map(|(timer, frame)| {
            timer_bounds(timer, logical_width, logical_height)
                .ok()
                .map(|bounds| (timer, bounds, frame))
        })
        .collect();
    for (pixel, chunk) in canvas
        .chunks_exact_mut(4)
        .take(width as usize * height as usize)
        .enumerate()
    {
        let x = (pixel as u32 % width) / scale;
        let y = (pixel as u32 / width) / scale;
        let mut value = frame_pixel(x, y, logical_width, logical_height, config);
        for (timer, bounds, frame) in &frames {
            if let Some(blend) = timer_pixel(x, y, *timer, *bounds, *frame) {
                value = opaque_mix(value & 0x00ff_ffff, timer.color, blend);
            }
        }
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Bounds {
    start: u32,
    end: u32,
    width: u32,
    height: u32,
}

fn timer_bounds(timer: TimerConfig, width: u32, height: u32) -> Result<Bounds, String> {
    let length = if matches!(timer.edge, Edge::Top | Edge::Bottom) {
        width
    } else {
        height
    };
    if timer.thickness_px
        > if matches!(timer.edge, Edge::Top | Edge::Bottom) {
            height
        } else {
            width
        }
    {
        return Err("timer thickness exceeds output bounds".into());
    }
    let resolve = |coordinate| match coordinate {
        Coordinate::Percent(value) => (u64::from(length) * u64::from(value) / 100) as u32,
        Coordinate::Pixels(value) => value,
    };
    let bounds = Bounds {
        start: resolve(timer.start),
        end: resolve(timer.end),
        width,
        height,
    };
    if bounds.start >= bounds.end || bounds.end > length {
        return Err("timer rendered bounds require start < end within the output".into());
    }
    Ok(bounds)
}

fn timer_pixel(
    x: u32,
    y: u32,
    timer: TimerConfig,
    bounds: Bounds,
    frame: TimerFrame,
) -> Option<u32> {
    // Vertical local coordinates increase from screen bottom to top.
    let (local, depth) = match timer.edge {
        Edge::Top => (x, y),
        Edge::Right => (bounds.height - 1 - y, bounds.width - 1 - x),
        Edge::Bottom => (x, bounds.height - 1 - y),
        Edge::Left => (bounds.height - 1 - y, x),
    };
    if depth >= timer.thickness_px || local < bounds.start || local >= bounds.end {
        return None;
    }
    let duration = frame.deadline_ns - frame.started_ns;
    let elapsed = frame.now_ns - frame.started_ns;
    let progress = elapsed.clamp(0, duration);
    let length = bounds.end - bounds.start;
    let visible = u32::try_from(i128::from(length) * progress / duration)
        .unwrap_or(length)
        .min(length);
    let activation = match timer.animation {
        Animation::Expand => {
            let start = bounds.start + (length - visible) / 2;
            if !(local >= start && local < start + visible) {
                return None;
            }
            let position = local - bounds.start;
            let needed = if position * 2 < length {
                length - position * 2 - 1
            } else {
                position * 2 - length + 2
            }
            .max(1);
            i128::from(needed) * duration / i128::from(length)
        }
        Animation::FlowUp => {
            if local >= bounds.start + visible {
                return None;
            }
            i128::from(local - bounds.start + 1) * duration / i128::from(length)
        }
        Animation::FlowDown => {
            if local < bounds.end - visible {
                return None;
            }
            i128::from(bounds.end - local) * duration / i128::from(length)
        }
        Animation::Static => 0,
        Animation::Blink => {
            let phase = elapsed % (2 * FADE_NS);
            if !timer.fade {
                return (phase < FADE_NS).then_some(255);
            }
            return Some(if phase < FADE_NS {
                (phase * 255 / FADE_NS) as u32
            } else {
                ((2 * FADE_NS - phase) * 255 / FADE_NS) as u32
            });
        }
    };
    Some(if timer.fade {
        ((elapsed - activation)
            .clamp(0, FADE_NS)
            .min((frame.deadline_ns - frame.now_ns).clamp(0, FADE_NS))
            * 255
            / FADE_NS) as u32
    } else {
        255
    })
}

fn opaque_mix(from: u32, to: u32, amount: u32) -> u32 {
    let mix = |shift: u32| {
        let from = (from >> shift) & 0xff_u32;
        let to = (to >> shift) & 0xff_u32;
        (from * (255 - amount) + to * amount + 127) / 255
    };
    0xff00_0000 | mix(16) << 16 | mix(8) << 8 | mix(0)
}

fn shadow(distance: u32, strength: u32, color: u32) -> u32 {
    let Some(&alpha) = SHADOW.get(distance as usize) else {
        return 0;
    };
    let alpha = (alpha * strength + 50) / 100;
    let red = ((color >> 16) * alpha + 127) / 255;
    let green = (((color >> 8) & 0xff) * alpha + 127) / 255;
    let blue = ((color & 0xff) * alpha + 127) / 255;
    alpha << 24 | red << 16 | green << 8 | blue
}

fn frame_pixel(x: u32, y: u32, width: u32, height: u32, config: &Config) -> u32 {
    let border = 0xff00_0000 | config.border_color;
    let near_x = x.min(width - 1 - x);
    let near_y = y.min(height - 1 - y);
    if config.corner_radius_px != 0 {
        let corner = config.border_thickness_px + config.corner_radius_px;
        if near_x < corner && near_y < corner {
            let dx = i64::from(near_x) - i64::from(corner);
            let dy = i64::from(near_y) - i64::from(corner);
            let distance_squared = dx * dx + dy * dy;
            if distance_squared > i64::from(config.corner_radius_px).pow(2) {
                return border;
            }
            for distance in 0..SHADOW_WIDTH {
                if distance_squared
                    > i64::from(config.corner_radius_px.saturating_sub(distance + 1)).pow(2)
                {
                    return shadow(
                        distance,
                        config.shadow_strength_percent,
                        config.shadow_color,
                    );
                }
            }
            return 0;
        }
    }
    let distance = near_x.min(near_y);
    if distance < config.border_thickness_px {
        border
    } else {
        shadow(
            distance - config.border_thickness_px,
            config.shadow_strength_percent,
            config.shadow_color,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(canvas: &[u8], width: u32, x: u32, y: u32) -> u32 {
        let start = ((y * width + x) * 4) as usize;
        u32::from_le_bytes(canvas[start..start + 4].try_into().unwrap())
    }

    #[test]
    fn compiled_defaults_and_strict_parser() {
        assert_eq!(Config::default(), parse_config(DEFAULT_CONFIG).unwrap());
        let config = parse_config("border_color = \"#123AbC\"\nevent_line_thickness_px = 4\n[timer]\nedge = \"left\"\nstart = \"2px\"\nend = \"90%\"\nduration = \"1d\"\nanimation = \"flow-up\"\nfade = false\ncolor = \"#abcdef\"\n").unwrap();
        assert_eq!(config.border_color, 0x123abc);
        assert_eq!(config.timer.thickness_px, 4);
        let fixed_range = parse_config("[timer]\nstart = \"20px\"\nend = \"30px\"").unwrap();
        assert!(timer_bounds(fixed_range.timer, 10, 10).is_err());
        for invalid in [
            "border_color = #000000",
            "border_color = \"#00000g\"",
            "unknown = 1",
            "[other]",
            "[highlight.default]\nedge = \"top\"", // incomplete
            "[timer]\nedge = \"bottom\"\nborder_color = \"#000000\"",
            "[timer]\nstart = \"101%\"",
            "[timer]\nstart = \"-1px\"",
            "event_line_thickness_px = 2\n[timer]\nthickness_px = 3",
            "border_thickness_px = 2\n[timer]\nthickness_px = 3",
            "border_thickness_px = 2\nevent_line_thickness_px = 3",
            "[timer]\nduration = \"1d1s\"",
            "[timer]\nduration = \"0s\"",
            "[timer]\nedge = \"top\"\nanimation = \"flow-up\"",
            "[timer]\nfade = 1",
            "[timer]\ncolor = \"#000000\"\ncolor = \"#ffffff\"",
        ] {
            assert!(parse_config(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    fn config_base() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "temporalshell-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn config_creation_is_exact_read_only_when_missing_and_never_replaces() {
        let base = config_base();
        let path = base.join("reEnvisioning/temporalShell/config.toml");
        assert_eq!(load_at(&path).unwrap(), Config::default());
        assert!(!path.exists());
        assert_eq!(load_or_create_at(&path).unwrap(), Config::default());
        assert_eq!(fs::read(&path).unwrap(), DEFAULT_CONFIG.as_bytes());
        fs::write(&path, "border_color = \"#123456\"\n").unwrap();
        assert_eq!(load_or_create_at(&path).unwrap().border_color, 0x123456);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "border_color = \"#123456\"\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            fs::remove_file(&path).unwrap();
            let outside = base.join("outside-config.toml");
            fs::write(&outside, "border_color = \"#654321\"\n").unwrap();
            symlink(&outside, &path).unwrap();
            assert_eq!(load_or_create_at(&path).unwrap().border_color, 0x654321);
            assert!(fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink());
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn config_creation_does_not_replace_a_concurrent_winner_and_cleans_failures() {
        use std::thread;

        let base = config_base();
        let path = base.join("reEnvisioning/temporalShell/config.toml");
        let left = path.clone();
        let right = path.clone();
        let one = thread::spawn(move || load_or_create_at(&left));
        let two = thread::spawn(move || load_or_create_at(&right));
        assert_eq!(one.join().unwrap().unwrap(), Config::default());
        assert_eq!(two.join().unwrap().unwrap(), Config::default());
        assert_eq!(fs::read(&path).unwrap(), DEFAULT_CONFIG.as_bytes());
        fs::remove_dir_all(base).unwrap();

        let base = config_base();
        let path = base.join("reEnvisioning/temporalShell/config.toml");
        private_directory(path.parent().unwrap()).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(load_or_create_at(&path).is_err());
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".config.toml.")
        }));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn parser_overlays_the_tracked_complete_defaults() {
        let config = parse_config("border_color = \"#123456\"\n[timer]\nfade = false\n").unwrap();
        assert_eq!(config.border_color, 0x123456);
        assert!(!config.timer.fade);
        assert_eq!(config.timer.edge, Config::default().timer.edge);
        assert_eq!(Config::default(), parse_config(DEFAULT_CONFIG).unwrap());
    }

    #[test]
    fn named_styles_are_complete_and_newest_overlays() {
        let config = parse_config(
            "[timer]\nfade = false\n[highlight.warm]\nedge = \"top\"\nstart = \"0%\"\nend = \"100%\"\nthickness_px = 1\nduration = \"1s\"\nanimation = \"static\"\nfade = false\ncolor = \"#ffffff\"\n",
        )
        .unwrap();
        assert!(parse_config("[highlight.bad]\nedge = \"top\"\n").is_err());
        let active = [
            crate::timer::ActiveHighlight {
                started_ns: 0,
                expires_ns: SECOND_NS,
                id: "old".into(),
                style: None,
            },
            crate::timer::ActiveHighlight {
                started_ns: 1,
                expires_ns: SECOND_NS + 1,
                id: "new".into(),
                style: config.named_style("warm"),
            },
        ];
        let mut canvas = vec![0; 10 * 10 * 4];
        paint_highlights(&mut canvas, 10, 10, 1, &config, &active, SECOND_NS).unwrap();
        assert_eq!(pixel(&canvas, 10, 5, 0), 0xffff_ffff);
        paint_highlights(&mut canvas, 10, 10, 1, &config, &[], SECOND_NS).unwrap();
        assert_eq!(pixel(&canvas, 10, 5, 0), 0xff00_0000);
    }

    #[test]
    fn direct_trigger_style_is_complete_and_strict() {
        let style = style_from_record(&[
            "top", "0%", "100%", "1", "10s", "static", "false", "#ffffff",
        ])
        .unwrap();
        assert_eq!(style.color, 0xffffff);
        assert_eq!(
            style_record(style),
            "top\t0%\t100%\t1\t10s\tstatic\tfalse\t#ffffff"
        );
        for fields in [
            &["top", "0%", "100%", "1", "10s", "static", "false"][..],
            &[
                "top", "0%", "100%", "0", "10s", "static", "false", "#ffffff",
            ][..],
            &[
                "top", "0%", "100%", "1", "10s", "flow-up", "false", "#ffffff",
            ][..],
            &["top", "0%", "100%", "1", "10s", "static", "false", "white"][..],
        ] {
            assert!(style_from_record(fields).is_err(), "accepted {fields:?}");
        }
    }

    #[test]
    fn raster_edges_coordinates_animations_and_color() {
        let defaults = Config::default();
        let mut config = Config {
            border_color: 0x102030,
            timer: TimerConfig {
                fade: false,
                animation: Animation::Static,
                start: Coordinate::Pixels(2),
                end: Coordinate::Percent(80),
                thickness_px: 2,
                ..defaults.timer
            },
            ..defaults
        };
        let frame = TimerFrame {
            started_ns: 0,
            deadline_ns: 10 * SECOND_NS,
            now_ns: 5 * SECOND_NS,
        };
        for edge in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
            config.timer.edge = edge;
            let mut canvas = vec![0; 10 * 10 * 4];
            paint(&mut canvas, 10, 10, 1, &config, Some(frame)).unwrap();
            assert_eq!(pixel(&canvas, 10, 0, 0), 0xff10_2030);
            match edge {
                Edge::Top => assert_eq!(pixel(&canvas, 10, 2, 0), 0xffb8_a890),
                Edge::Right => assert_eq!(pixel(&canvas, 10, 9, 7), 0xffb8_a890),
                Edge::Bottom => assert_eq!(pixel(&canvas, 10, 2, 9), 0xffb8_a890),
                Edge::Left => assert_eq!(pixel(&canvas, 10, 0, 7), 0xffb8_a890),
            }
        }
        config.timer.start = Coordinate::Pixels(8);
        config.timer.end = Coordinate::Pixels(2);
        paint(&mut vec![0; 400], 10, 10, 1, &config, Some(frame)).unwrap();
    }

    #[test]
    fn animation_masks_follow_local_coordinates_and_blink_at_two_hz() {
        let defaults = Config::default().timer;
        let bounds = timer_bounds(defaults, 10, 10).unwrap();
        let frame = TimerFrame {
            started_ns: 0,
            deadline_ns: 10 * SECOND_NS,
            now_ns: 5 * SECOND_NS,
        };
        let active = |animation, x, y, now_ns| {
            timer_pixel(
                x,
                y,
                TimerConfig {
                    edge: Edge::Left,
                    animation,
                    fade: false,
                    ..defaults
                },
                bounds,
                TimerFrame { now_ns, ..frame },
            )
            .is_some()
        };
        assert!(active(Animation::FlowUp, 0, 9, frame.now_ns));
        assert!(!active(Animation::FlowUp, 0, 0, frame.now_ns));
        assert!(!active(Animation::FlowDown, 0, 9, frame.now_ns));
        assert!(active(Animation::FlowDown, 0, 0, frame.now_ns));
        assert!(!active(Animation::Expand, 0, 9, frame.now_ns));
        assert!(active(Animation::Expand, 0, 5, frame.now_ns));
        assert_eq!(
            timer_pixel(
                0,
                9,
                TimerConfig {
                    edge: Edge::Left,
                    animation: Animation::Blink,
                    fade: true,
                    ..defaults
                },
                bounds,
                TimerFrame {
                    now_ns: FADE_NS,
                    ..frame
                },
            ),
            Some(255)
        );
        assert_eq!(
            timer_pixel(
                0,
                9,
                TimerConfig {
                    edge: Edge::Left,
                    animation: Animation::Blink,
                    fade: false,
                    ..defaults
                },
                bounds,
                TimerFrame {
                    now_ns: FADE_NS,
                    ..frame
                },
            ),
            None
        );
        let flow = TimerConfig {
            edge: Edge::Left,
            animation: Animation::FlowUp,
            ..defaults
        };
        assert_eq!(
            timer_pixel(
                0,
                6,
                flow,
                bounds,
                TimerFrame {
                    now_ns: 4_000_000_000,
                    ..frame
                },
            ),
            Some(0)
        );
        let fading = TimerConfig {
            animation: Animation::Static,
            fade: true,
            ..defaults
        };
        assert_eq!(
            timer_pixel(
                0,
                9,
                fading,
                bounds,
                TimerFrame {
                    now_ns: frame.deadline_ns - FADE_NS,
                    ..frame
                },
            ),
            Some(255)
        );
        assert_eq!(
            timer_pixel(
                0,
                9,
                fading,
                bounds,
                TimerFrame {
                    now_ns: frame.deadline_ns - 1,
                    ..frame
                },
            ),
            Some(0)
        );
    }

    #[test]
    fn frame_scale_padding_and_buffer_cap() {
        let mut frame = vec![0; 40 * 40 * 4 + 7];
        paint(&mut frame, 40, 40, 1, &Config::default(), None).unwrap();
        assert!(frame[6400..].iter().all(|byte| *byte == 0));
        assert_eq!(pixel(&frame, 40, 0, 0), 0xff00_0000);
        assert_eq!(pixel(&frame, 40, 20, 20), 0);
        let mut scaled = vec![0; 80 * 80 * 4];
        paint(&mut scaled, 80, 80, 2, &Config::default(), None).unwrap();
        for y in 0..40 {
            for x in 0..40 {
                for scale_y in 0..2 {
                    for scale_x in 0..2 {
                        assert_eq!(
                            pixel(&scaled, 80, x * 2 + scale_x, y * 2 + scale_y),
                            pixel(&frame, 40, x, y)
                        );
                    }
                }
            }
        }
        assert!(buffer_dimensions(3840, 2160, 1).is_ok());
        assert!(buffer_dimensions(5120, 2880, 1).is_ok());
        assert!(buffer_dimensions(7680, 4320, 1).is_err());
        assert!(buffer_dimensions(0, 16, 1).is_err());
        assert!(buffer_dimensions(16, 16, 0).is_err());
    }
}
