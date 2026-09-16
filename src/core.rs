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
const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coordinate {
    Percent(i32),
    Pixels(i32),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnimationConfig {
    pixels: String,
    curve: crate::animation::Curve,
    expression: Option<crate::animation::Expression>,
    mask: Option<crate::animation::Expression>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PixelConfig {
    curve: crate::animation::Curve,
    expression: Option<crate::animation::Expression>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TimerConfig {
    pub(crate) edge: Edge,
    pub(crate) start: Coordinate,
    pub(crate) end: Coordinate,
    pub(crate) duration_seconds: i64,
    pub(crate) animation: String,
    pub(crate) color: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Config {
    pub(crate) border_thickness_px: u32,
    pub(crate) highlight_thickness_px: u32,
    pub(crate) corner_radius_px: u32,
    pub(crate) shadow_width_px: u32,
    pub(crate) shadow_peak_opacity_percent: u32,
    pub(crate) shadow_strength_percent: u32,
    pub(crate) shadow_color: u32,
    pub(crate) border_color: u32,
    pub(crate) timer: TimerConfig,
    highlight_default: TimerConfig,
    highlights: Vec<(String, TimerConfig)>,
    animations: Vec<(String, AnimationConfig)>,
    pixels: Vec<(String, PixelConfig)>,
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

    pub(crate) fn default_highlight(&self) -> TimerConfig {
        self.highlight_default.clone()
    }

    pub(crate) fn has_animation(&self, name: &str) -> bool {
        self.animation(name).is_some()
    }

    fn animation(&self, name: &str) -> Option<&AnimationConfig> {
        self.animations
            .iter()
            .find(|(item, _)| item == name)
            .map(|(_, item)| item)
    }

    fn pixels(&self, name: &str) -> Option<&PixelConfig> {
        self.pixels
            .iter()
            .find(|(item, _)| item == name)
            .map(|(_, item)| item)
    }

    pub(crate) fn animation_changes(&self, style: &TimerConfig) -> bool {
        self.animation(&style.animation).is_some_and(|animation| {
            animation.curve.continuous()
                || animation
                    .expression
                    .as_ref()
                    .is_some_and(|value| value.uses_time())
                || animation
                    .mask
                    .as_ref()
                    .is_some_and(|value| value.uses_time())
                || self.pixels(&animation.pixels).is_some_and(|pixels| {
                    pixels.curve.continuous()
                        || pixels
                            .expression
                            .as_ref()
                            .is_some_and(|value| value.uses_time())
                })
        })
    }

    pub(crate) fn named_style(&self, name: &str) -> Option<TimerConfig> {
        self.highlights
            .iter()
            .find(|(item, _)| item == name)
            .map(|(_, style)| style.clone())
    }
}

fn empty_config() -> Config {
    Config {
        border_thickness_px: 0,
        highlight_thickness_px: 0,
        corner_radius_px: 0,
        shadow_width_px: 0,
        shadow_peak_opacity_percent: 0,
        shadow_strength_percent: 0,
        shadow_color: 0,
        border_color: 0,
        timer: TimerConfig {
            edge: Edge::Top,
            start: Coordinate::Percent(0),
            end: Coordinate::Percent(0),
            duration_seconds: 0,
            animation: String::new(),
            color: 0,
        },
        highlight_default: TimerConfig {
            edge: Edge::Top,
            start: Coordinate::Percent(0),
            end: Coordinate::Percent(0),
            duration_seconds: 0,
            animation: String::new(),
            color: 0,
        },
        highlights: Vec::new(),
        animations: Vec::new(),
        pixels: Vec::new(),
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
    Ok(base.join("temporalshell/config.toml"))
}

fn absolute_path(name: &str, value: std::ffi::OsString) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(format!("{name} must be a non-empty absolute path"));
    }
    Ok(path)
}

fn parse_config(text: &str) -> Result<Config, String> {
    parse_config_over(
        text,
        if text == DEFAULT_CONFIG {
            empty_config()
        } else {
            Config::default()
        },
    )
}

fn parse_config_over(text: &str, mut config: Config) -> Result<Config, String> {
    enum Table {
        Root,
        Timer(TimerConfig, Vec<String>),
        Default(TimerConfig, Vec<String>),
        Highlight(String, TimerConfig, Vec<String>),
        Animation(String, AnimationConfig, Vec<String>),
        Pixel(String, PixelConfig, Vec<String>),
    }
    let require_complete_default = config.border_thickness_px == 0;
    let mut table = Table::Root;
    let mut seen_root = Vec::new();
    let mut seen_timer = false;
    let mut seen_default = false;
    let mut named_started = false;
    let mut seen_animations = Vec::new();
    let mut seen_pixels = Vec::new();

    macro_rules! finish_table {
        ($line:expr) => {
            match &table {
                Table::Root => {}
                Table::Timer(style, _) => config.timer = style.clone(),
                Table::Default(style, seen) => {
                    if require_complete_default {
                        complete_style(seen, $line)?;
                    }
                    config.highlight_default = style.clone();
                }
                Table::Highlight(name, style, _) => {
                    config.highlights.push((name.clone(), style.clone()))
                }
                Table::Animation(name, animation, _) => {
                    config.animations.push((name.clone(), animation.clone()))
                }
                Table::Pixel(name, pixels, _) => config.pixels.push((name.clone(), pixels.clone())),
            }
        };
    }

    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = strip_comment(raw, line_number)?.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            finish_table!(line_number);
            table = if line == "[timer]" && !seen_timer && !seen_default && !named_started {
                seen_timer = true;
                Table::Timer(config.timer.clone(), Vec::new())
            } else if line == "[highlight.default]" && !seen_default && !named_started {
                seen_default = true;
                Table::Default(config.highlight_default.clone(), Vec::new())
            } else if let Some(name) = line
                .strip_prefix("[animation.")
                .and_then(|value| value.strip_suffix(']'))
            {
                if validate_highlight_name(name).is_err()
                    || seen_animations.iter().any(|item| item == name)
                {
                    return Err(format!("line {line_number}: unknown or duplicate table"));
                }
                seen_animations.push(name.to_owned());
                let animation = config
                    .animations
                    .iter()
                    .find(|(item, _)| item == name)
                    .map(|(_, item)| item.clone())
                    .unwrap_or(AnimationConfig {
                        pixels: String::new(),
                        curve: crate::animation::constant(),
                        expression: None,
                        mask: None,
                    });
                config.animations.retain(|(item, _)| item != name);
                Table::Animation(name.to_owned(), animation, Vec::new())
            } else if let Some(name) = line
                .strip_prefix("[pixel.")
                .and_then(|value| value.strip_suffix(']'))
            {
                if validate_highlight_name(name).is_err()
                    || seen_pixels.iter().any(|item| item == name)
                {
                    return Err(format!("line {line_number}: unknown or duplicate table"));
                }
                seen_pixels.push(name.to_owned());
                let pixels = config
                    .pixels
                    .iter()
                    .find(|(item, _)| item == name)
                    .map(|(_, item)| item.clone())
                    .unwrap_or(PixelConfig {
                        curve: crate::animation::constant(),
                        expression: None,
                    });
                config.pixels.retain(|(item, _)| item != name);
                Table::Pixel(name.to_owned(), pixels, Vec::new())
            } else if let Some(name) = line
                .strip_prefix("[highlight.")
                .and_then(|value| value.strip_suffix(']'))
            {
                if validate_highlight_name(name).is_err()
                    || name == "default"
                    || config.named_style(name).is_some()
                {
                    return Err(format!("line {line_number}: unknown or duplicate table"));
                }
                named_started = true;
                Table::Highlight(
                    name.to_owned(),
                    config.highlight_default.clone(),
                    Vec::new(),
                )
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
                    "highlight_thickness_px" => {
                        config.highlight_thickness_px = positive_integer(value, key, line_number)?
                    }
                    "corner_radius_px" => {
                        config.corner_radius_px = integer(value, key, line_number)?;
                        if config.corner_radius_px > MAX_CONFIG_VALUE {
                            return Err(format!(
                                "line {line_number}: {key} must be between 0 and {MAX_CONFIG_VALUE}"
                            ));
                        }
                    }
                    "shadow_width_px" => {
                        config.shadow_width_px = positive_integer(value, key, line_number)?
                    }
                    "shadow_peak_opacity_percent" => {
                        config.shadow_peak_opacity_percent = percent(value, key, line_number)?
                    }
                    "shadow_strength_percent" => {
                        config.shadow_strength_percent = percent(value, key, line_number)?
                    }
                    "shadow_color" => config.shadow_color = color(value, key, line_number)?,
                    "border_color" => config.border_color = color(value, key, line_number)?,
                    _ => return Err(format!("line {line_number}: unknown root key {key}")),
                }
            }
            Table::Timer(style, seen)
            | Table::Default(style, seen)
            | Table::Highlight(_, style, seen) => {
                if seen.iter().any(|item| item == key) {
                    return Err(format!("line {line_number}: duplicate key {key}"));
                }
                seen.push(key.to_owned());
                apply_style_key(style, key, value, line_number)?;
            }
            Table::Animation(_, animation, seen) => {
                if seen.iter().any(|item| item == key) {
                    return Err(format!("line {line_number}: duplicate key {key}"));
                }
                seen.push(key.to_owned());
                match key {
                    "pixels" => animation.pixels = quoted(value, key, line_number)?.to_owned(),
                    "keyframes" => {
                        animation.curve =
                            crate::animation::curve(value, Some(animation.curve.interpolation))
                                .map_err(|error| format!("line {line_number}: {error}"))?
                    }
                    "interpolation" => {
                        animation.curve.interpolation =
                            crate::animation::interpolation(quoted(value, key, line_number)?)
                                .map_err(|error| format!("line {line_number}: {error}"))?
                    }
                    "expression" => {
                        animation.expression = Some(
                            crate::animation::expression(quoted(value, key, line_number)?)
                                .map_err(|error| format!("line {line_number}: {error}"))?,
                        )
                    }
                    "mask" => {
                        animation.mask = Some(
                            crate::animation::expression(quoted(value, key, line_number)?)
                                .map_err(|error| format!("line {line_number}: {error}"))?,
                        )
                    }
                    _ => return Err(format!("line {line_number}: unknown animation key {key}")),
                }
            }
            Table::Pixel(_, pixels, seen) => {
                if seen.iter().any(|item| item == key) {
                    return Err(format!("line {line_number}: duplicate key {key}"));
                }
                seen.push(key.to_owned());
                match key {
                    "keyframes" => {
                        pixels.curve =
                            crate::animation::curve(value, Some(pixels.curve.interpolation))
                                .map_err(|error| format!("line {line_number}: {error}"))?
                    }
                    "interpolation" => {
                        pixels.curve.interpolation =
                            crate::animation::interpolation(quoted(value, key, line_number)?)
                                .map_err(|error| format!("line {line_number}: {error}"))?
                    }
                    "expression" => {
                        pixels.expression = Some(
                            crate::animation::expression(quoted(value, key, line_number)?)
                                .map_err(|error| format!("line {line_number}: {error}"))?,
                        )
                    }
                    _ => return Err(format!("line {line_number}: unknown pixel key {key}")),
                }
            }
        }
    }
    finish_table!(text.lines().count() + 1);
    if config.highlight_thickness_px == 0
        || config.highlight_thickness_px > config.border_thickness_px
    {
        return Err("highlight_thickness_px must be between 1 and border_thickness_px".into());
    }
    if config.shadow_width_px == 0 {
        return Err("shadow_width_px must be positive".into());
    }
    validate_style(&config.timer)?;
    validate_style(&config.highlight_default)?;
    for (_, style) in &config.highlights {
        validate_style(style)?;
    }
    for style in std::iter::once(&config.timer)
        .chain(std::iter::once(&config.highlight_default))
        .chain(config.highlights.iter().map(|(_, style)| style))
    {
        let animation = config
            .animation(&style.animation)
            .ok_or_else(|| format!("unknown animation {}", style.animation))?;
        if config.pixels(&animation.pixels).is_none() {
            return Err(format!("unknown pixels {}", animation.pixels));
        }
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
    format!(
        "{edge}\t{}\t{}\t{}s\t{}\t#{:06x}",
        coordinate(style.start),
        coordinate(style.end),
        style.duration_seconds,
        style.animation,
        style.color
    )
}

pub(crate) fn style_from_record(fields: &[&str]) -> Result<TimerConfig, String> {
    let [edge, start, end, duration, animation, color] = fields else {
        return Err("trigger style fields".into());
    };
    let mut style = empty_config().timer;
    for (key, value) in [
        ("edge", *edge),
        ("start", *start),
        ("end", *end),
        ("duration", *duration),
        ("animation", *animation),
        ("color", *color),
    ] {
        apply_trigger_option(&mut style, key, value)?;
    }
    validate_style(&style)?;
    Ok(style)
}

pub(crate) fn apply_trigger_option(
    style: &mut TimerConfig,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let quoted_value = format!("\"{value}\"");
    let value = &quoted_value;
    apply_style_key(style, key, value, 0)
        .map_err(|error| error.strip_prefix("line 0: ").unwrap_or(&error).to_owned())
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

fn complete_style(seen: &[String], line: usize) -> Result<(), String> {
    for key in ["edge", "start", "end", "duration", "animation", "color"] {
        if !seen.iter().any(|item| item == key) {
            return Err(format!("line {line}: highlight.default must contain {key}"));
        }
    }
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
                _ => return Err(format!("line {line}: invalid highlight edge")),
            }
        }
        "start" => style.start = coordinate(value, key, line)?,
        "end" => style.end = coordinate(value, key, line)?,
        "duration" => {
            let seconds = crate::timer::parse_duration(quoted(value, key, line)?)?;
            if seconds > 86_400 {
                return Err(format!(
                    "line {line}: highlight duration must not exceed 1d"
                ));
            }
            style.duration_seconds = seconds;
        }
        "animation" => style.animation = quoted(value, key, line)?.to_owned(),
        "color" => style.color = color(value, key, line)?,
        _ => return Err(format!("line {line}: unknown highlight key {key}")),
    };
    Ok(())
}

pub(crate) fn validate_style(style: &TimerConfig) -> Result<(), String> {
    validate_highlight_name(&style.animation)
        .map_err(|_| "invalid highlight animation".to_owned())?;
    if matches!(
        (style.start, style.end),
        (Coordinate::Percent(start), Coordinate::Percent(end))
            | (Coordinate::Pixels(start), Coordinate::Pixels(end))
            if start >= end
    ) {
        return Err("highlight start must be less than end".into());
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

fn percent(value: &str, key: &str, line: usize) -> Result<u32, String> {
    let number = integer(value, key, line)?;
    if number > 100 {
        return Err(format!("line {line}: {key} must be between 0 and 100"));
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
        let number = signed_integer(digits, key, line)?;
        if (-100..=200).contains(&number) {
            return Ok(Coordinate::Percent(number));
        }
    } else if let Some(digits) = value.strip_suffix("px") {
        return Ok(Coordinate::Pixels(signed_integer(digits, key, line)?));
    }
    Err(format!(
        "line {line}: {key} must be -100%..200% or a signed pixel bound"
    ))
}

fn signed_integer(value: &str, key: &str, line: usize) -> Result<i32, String> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("line {line}: {key} must be an integer"));
    }
    value
        .parse()
        .map_err(|_| format!("line {line}: {key} is out of range"))
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
                let timer = highlight
                    .style
                    .clone()
                    .unwrap_or_else(|| config.timer.clone());
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
            timer_bounds(
                timer.clone(),
                logical_width,
                logical_height,
                if config.corner_radius_px == 0 {
                    0
                } else {
                    config.border_thickness_px + config.corner_radius_px
                },
                config.highlight_thickness_px,
            )
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
        if could_hit_highlight(x, y, logical_width, logical_height, config) {
            for (timer, bounds, frame) in &frames {
                if let Some(blend) = timer_pixel(x, y, timer, *bounds, *frame, config) {
                    value = opaque_mix(value & 0x00ff_ffff, timer.color, blend);
                }
            }
        }
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    Ok(())
}

fn could_hit_highlight(x: u32, y: u32, width: u32, height: u32, config: &Config) -> bool {
    let near_edge = x.min(width - 1 - x).min(y.min(height - 1 - y));
    if near_edge < config.highlight_thickness_px {
        return true;
    }
    if config.corner_radius_px == 0 {
        return false;
    }
    let radius = config.border_thickness_px + config.corner_radius_px;
    (x < radius || x >= width.saturating_sub(radius))
        && (y < radius || y >= height.saturating_sub(radius))
}

#[derive(Clone, Copy)]
struct Bounds {
    start: i64,
    end: i64,
    perimeter: i64,
    anchor: i64,
    forward: bool,
    width: u32,
    height: u32,
    radius: u32,
    thickness: u32,
}

fn timer_bounds(
    timer: TimerConfig,
    width: u32,
    height: u32,
    radius: u32,
    thickness: u32,
) -> Result<Bounds, String> {
    if width == 0 || height == 0 || thickness > width.min(height) {
        return Err("highlight thickness exceeds output bounds".into());
    }
    if radius != 0 && (radius >= width / 2 || radius >= height / 2) {
        return Err("rounded highlight does not fit output".into());
    }
    let horizontal = i64::from(width - 2 * radius);
    let vertical = i64::from(height - 2 * radius);
    if horizontal == 0 || vertical == 0 {
        return Err("rounded highlight does not fit output".into());
    }
    let arc = (std::f64::consts::FRAC_PI_2 * f64::from(radius)).round() as i64;
    let (own, neighbor, anchor, forward) = match timer.edge {
        Edge::Top => (horizontal, vertical, 0, true),
        Edge::Right => (vertical, horizontal, horizontal + arc + vertical, false),
        Edge::Bottom => (
            horizontal,
            vertical,
            2 * horizontal + 2 * arc + vertical,
            false,
        ),
        Edge::Left => (
            vertical,
            horizontal,
            2 * horizontal + 3 * arc + vertical,
            true,
        ),
    };
    let resolve = |coordinate| match coordinate {
        Coordinate::Percent(value) if value < 0 => {
            -arc - (neighbor * i64::from(-value)).div_euclid(100)
        }
        Coordinate::Percent(value) if value > 100 => {
            own + arc + (neighbor * i64::from(value - 100)).div_euclid(100)
        }
        Coordinate::Percent(value) => (own * i64::from(value)).div_euclid(100),
        Coordinate::Pixels(value) => i64::from(value),
    };
    let start = resolve(timer.start);
    let end = resolve(timer.end);
    let adjacent = neighbor + arc;
    if start >= end || start < -adjacent || end > own + adjacent {
        return Err(
            "highlight bounds must satisfy start < end and wrap at most one adjacent edge".into(),
        );
    }
    Ok(Bounds {
        start,
        end,
        perimeter: 2 * horizontal + 2 * vertical + 4 * arc,
        anchor,
        forward,
        width,
        height,
        radius,
        thickness,
    })
}

fn inside_rounded(x: f64, y: f64, width: f64, height: f64, inset: f64, radius: f64) -> bool {
    if inset * 2.0 >= width || inset * 2.0 >= height {
        return false;
    }
    let left = inset;
    let top = inset;
    let right = width - inset;
    let bottom = height - inset;
    if x < left || x >= right || y < top || y >= bottom {
        return false;
    }
    let radius = (radius - inset).max(0.0);
    if radius == 0.0 {
        return true;
    }
    let nearest_x = x.clamp(left + radius, right - radius);
    let nearest_y = y.clamp(top + radius, bottom - radius);
    (x - nearest_x).powi(2) + (y - nearest_y).powi(2) <= radius.powi(2)
}

fn contour_position(x: u32, y: u32, edge: Edge, bounds: Bounds, thickness: u32) -> Option<i64> {
    let x = f64::from(x) + 0.5;
    let y = f64::from(y) + 0.5;
    let width = f64::from(bounds.width);
    let height = f64::from(bounds.height);
    let radius = f64::from(bounds.radius);
    if !inside_rounded(x, y, width, height, 0.0, radius)
        || inside_rounded(x, y, width, height, f64::from(thickness), radius)
    {
        return None;
    }
    if bounds.radius == 0 {
        let distances = [y, width - x, height - y, x];
        let minimum = distances.iter().copied().reduce(f64::min)?;
        let selected = match edge {
            Edge::Top => 0,
            Edge::Right => 1,
            Edge::Bottom => 2,
            Edge::Left => 3,
        };
        let side = if distances[selected] == minimum {
            selected
        } else {
            distances.iter().position(|distance| *distance == minimum)?
        };
        return Some(
            match side {
                0 => x,
                1 => width + y,
                2 => 2.0 * width + height - x,
                _ => 2.0 * width + 2.0 * height - y,
            }
            .floor() as i64,
        );
    }
    let horizontal = f64::from(bounds.width - 2 * bounds.radius);
    let vertical = f64::from(bounds.height - 2 * bounds.radius);
    let arc = std::f64::consts::FRAC_PI_2 * radius;
    let position = if x < radius && y < radius {
        let angle = (y - radius).atan2(x - radius);
        2.0 * horizontal + 2.0 * vertical + 3.0 * arc + (angle + std::f64::consts::PI) * radius
    } else if x >= width - radius && y < radius {
        let angle = (y - radius).atan2(x - (width - radius));
        horizontal + (angle + std::f64::consts::FRAC_PI_2) * radius
    } else if x >= width - radius && y >= height - radius {
        let angle = (y - (height - radius)).atan2(x - (width - radius));
        horizontal + arc + vertical + angle * radius
    } else if x < radius && y >= height - radius {
        let angle = (y - (height - radius)).atan2(x - radius);
        2.0 * horizontal + 2.0 * arc + vertical + (angle - std::f64::consts::FRAC_PI_2) * radius
    } else {
        let distances = [y, width - x, height - y, x];
        match distances
            .iter()
            .enumerate()
            .min_by(|left, right| left.1.total_cmp(right.1))?
            .0
        {
            0 => x - radius,
            1 => horizontal + arc + y - radius,
            2 => 2.0 * horizontal + 2.0 * arc + vertical - (x - radius),
            _ => 2.0 * horizontal + 3.0 * arc + 2.0 * vertical - (y - radius),
        }
    };
    Some(position.round() as i64)
}

fn timer_pixel(
    x: u32,
    y: u32,
    timer: &TimerConfig,
    bounds: Bounds,
    frame: TimerFrame,
    config: &Config,
) -> Option<u32> {
    let position = contour_position(x, y, timer.edge, bounds, bounds.thickness)?;
    let distance = if bounds.forward {
        (position - bounds.anchor).rem_euclid(bounds.perimeter)
    } else {
        (bounds.anchor - position).rem_euclid(bounds.perimeter)
    };
    let local = [distance, distance - bounds.perimeter]
        .into_iter()
        .find(|local| *local >= bounds.start && *local < bounds.end)?;
    let duration = frame.deadline_ns - frame.started_ns;
    if duration <= 0 {
        return None;
    }
    let elapsed_ns = (frame.now_ns - frame.started_ns).clamp(0, duration);
    let progress = (elapsed_ns * crate::animation::SCALE as i128 / duration) as i64;
    let animation = config.animation(&timer.animation)?;
    let keyframe = animation.curve.sample(progress);
    let position = (local - bounds.start) * crate::animation::SCALE / (bounds.end - bounds.start);
    let seconds = |value: i128| value as f64 / 1_000_000_000.0;
    let mut context = crate::animation::Context {
        phase: progress as f64 / crate::animation::SCALE as f64,
        keyframe: keyframe as f64 / crate::animation::SCALE as f64,
        from: 0.0,
        to: 1.0,
        value: keyframe as f64 / crate::animation::SCALE as f64,
        elapsed: seconds(elapsed_ns),
        remaining: seconds(duration - elapsed_ns),
        duration: seconds(duration),
        position: position as f64 / crate::animation::SCALE as f64,
        pixel: 1.0 / (bounds.end - bounds.start).max(1) as f64,
        coverage: 1.0,
    };
    if let Some(expression) = &animation.expression {
        context.keyframe = expression.evaluate_context(context);
        context.value = context.from + (context.to - context.from) * context.keyframe;
    }
    if animation
        .mask
        .as_ref()
        .is_some_and(|mask| mask.evaluate_context(context) < 0.0)
    {
        return None;
    }
    let pixels = config.pixels(&animation.pixels)?;
    let opacity = pixels
        .curve
        .sample((context.keyframe * crate::animation::SCALE as f64).round() as i64);
    context.coverage = opacity as f64 / crate::animation::SCALE as f64;
    let opacity = pixels
        .expression
        .as_ref()
        .map_or(context.coverage, |expression| {
            expression.evaluate_context(context)
        });
    Some((opacity.clamp(0.0, 1.0) * 255.0).round() as u32)
}

fn opaque_mix(from: u32, to: u32, amount: u32) -> u32 {
    let mix = |shift: u32| {
        let from = (from >> shift) & 0xff_u32;
        let to = (to >> shift) & 0xff_u32;
        (from * (255 - amount) + to * amount + 127) / 255
    };
    0xff00_0000 | mix(16) << 16 | mix(8) << 8 | mix(0)
}

fn shadow(distance: u32, config: &Config) -> u32 {
    if distance >= config.shadow_width_px {
        return 0;
    }
    let width = u64::from(config.shadow_width_px);
    let alpha = (255
        * u64::from(config.shadow_peak_opacity_percent)
        * u64::from(config.shadow_width_px - distance)
        + width * 50)
        / (width * 100);
    let alpha = (alpha as u32 * config.shadow_strength_percent + 50) / 100;
    let red = ((config.shadow_color >> 16) * alpha + 127) / 255;
    let green = (((config.shadow_color >> 8) & 0xff) * alpha + 127) / 255;
    let blue = ((config.shadow_color & 0xff) * alpha + 127) / 255;
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
            for distance in 0..config.shadow_width_px {
                if distance_squared
                    > i64::from(config.corner_radius_px.saturating_sub(distance + 1)).pow(2)
                {
                    return shadow(distance, config);
                }
            }
            return 0;
        }
    }
    let distance = near_x.min(near_y);
    if distance < config.border_thickness_px {
        border
    } else {
        shadow(distance - config.border_thickness_px, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND_NS: i128 = 1_000_000_000;

    fn pixel(canvas: &[u8], width: u32, x: u32, y: u32) -> u32 {
        let start = ((y * width + x) * 4) as usize;
        u32::from_le_bytes(canvas[start..start + 4].try_into().unwrap())
    }

    fn paint(
        canvas: &mut [u8],
        width: u32,
        height: u32,
        scale: u32,
        config: &Config,
        frame: Option<TimerFrame>,
    ) {
        paint_frames(
            canvas,
            width,
            height,
            scale,
            config,
            frame.into_iter().map(|frame| (config.timer.clone(), frame)),
        )
        .unwrap();
    }

    #[test]
    fn declarative_defaults_are_strict() {
        assert_eq!(Config::default(), parse_config(DEFAULT_CONFIG).unwrap());
        let config = parse_config("border_color = \"#123AbC\"\n[timer]\nedge = \"left\"\nstart = \"2px\"\nend = \"90%\"\nduration = \"1d\"\nanimation = \"flow-up\"\ncolor = \"#abcdef\"\n").unwrap();
        assert_eq!(config.border_color, 0x123abc);
        assert_eq!(config.timer.animation, "flow-up");
        assert!(timer_bounds(
            TimerConfig {
                start: Coordinate::Pixels(20),
                end: Coordinate::Pixels(30),
                ..config.timer.clone()
            },
            10,
            10,
            0,
            3
        )
        .is_err());
        for invalid in [
            "border_color = #000000",
            "border_color = \"#00000g\"",
            "unknown = 1",
            "[other]",
            "[highlight.default]\nedge = \"top\"\nedge = \"bottom\"",
            "[timer]\nedge = \"bottom\"\nborder_color = \"#000000\"",
            "[timer]\nstart = \"201%\"",
            "[timer]\nduration = \"1d1s\"",
            "[timer]\nduration = \"0s\"",
            "[timer]\nfade = true",
            "fade_duration_ms = 1",
            "[animation.expand]\npixels = \"missing\"",
            "[pixel.solid]\nkeyframes = []",
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
        let path = base.join("temporalshell/config.toml");
        assert_eq!(load_at(&path).unwrap(), Config::default());
        assert!(!path.exists());
        assert_eq!(load_or_create_at(&path).unwrap(), Config::default());
        assert_eq!(fs::read(&path).unwrap(), DEFAULT_CONFIG.as_bytes());
        fs::write(&path, "border_color = \"#123456\"\n").unwrap();
        assert_eq!(load_or_create_at(&path).unwrap().border_color, 0x123456);
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
        let path = base.join("temporalshell/config.toml");
        let one = thread::spawn({
            let path = path.clone();
            move || load_or_create_at(&path)
        });
        let two = thread::spawn({
            let path = path.clone();
            move || load_or_create_at(&path)
        });
        assert_eq!(one.join().unwrap().unwrap(), Config::default());
        assert_eq!(two.join().unwrap().unwrap(), Config::default());
        assert_eq!(fs::read(&path).unwrap(), DEFAULT_CONFIG.as_bytes());
        fs::remove_dir_all(base).unwrap();

        let base = config_base();
        let path = base.join("temporalshell/config.toml");
        private_directory(path.parent().unwrap()).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(load_or_create_at(&path).is_err());
        assert!(fs::read_dir(path.parent().unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".config.toml.")));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn parser_overlays_defaults_and_named_styles() {
        let config = parse_config("border_color = \"#123456\"\n[timer]\ncolor = \"#abcdef\"\n[highlight.warm]\nedge = \"top\"\nanimation = \"static\"\ncolor = \"#ffffff\"\n").unwrap();
        assert_eq!(config.border_color, 0x123456);
        assert_eq!(config.timer.color, 0xabcdef);
        assert_eq!(config.named_style("warm").unwrap().edge, Edge::Top);
        assert!(parse_config("[highlight.bad]\nanimation = \"missing\"\n").is_err());

        let before = parse_config(
            "[animation.expand]\ninterpolation = \"smooth\"\nkeyframes = [\"0:0\", \"100:100\"]\n",
        )
        .unwrap();
        let after = parse_config(
            "[animation.expand]\nkeyframes = [\"0:0\", \"100:100\"]\ninterpolation = \"smooth\"\n",
        )
        .unwrap();
        assert_eq!(
            before.animation("expand").unwrap().curve,
            after.animation("expand").unwrap().curve
        );
    }

    #[test]
    fn named_style_overlays_older_highlights() {
        let config = parse_config("[timer]\nedge = \"top\"\nanimation = \"static\"\ncolor = \"#123456\"\n[highlight.warm]\nedge = \"top\"\nanimation = \"static\"\ncolor = \"#ffffff\"\n").unwrap();
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
        paint_highlights(&mut canvas, 10, 10, 1, &config, &active, SECOND_NS - 1).unwrap();
        assert_eq!(pixel(&canvas, 10, 5, 0), 0xffff_ffff);
    }

    #[test]
    fn trigger_records_name_animations() {
        let style = style_from_record(&["top", "0%", "100%", "10s", "static", "#ffffff"]).unwrap();
        assert_eq!(style_record(style), "top\t0%\t100%\t10s\tstatic\t#ffffff");
        assert!(
            style_from_record(&["top", "0%", "100%", "10s", "static", "false", "#ffffff"]).is_err()
        );
    }

    #[test]
    fn raster_edges_coordinates_and_color() {
        let defaults = Config::default();
        let mut config = Config {
            border_color: 0x102030,
            highlight_thickness_px: 2,
            timer: TimerConfig {
                start: Coordinate::Pixels(2),
                end: Coordinate::Percent(80),
                animation: "static".into(),
                color: 0xb8a890,
                ..defaults.timer.clone()
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
            paint(&mut canvas, 10, 10, 1, &config, Some(frame));
            assert_eq!(pixel(&canvas, 10, 0, 0), 0xff10_2030);
            match edge {
                Edge::Top => assert_eq!(pixel(&canvas, 10, 2, 0), 0xffb8_a890),
                Edge::Right => assert_eq!(pixel(&canvas, 10, 9, 7), 0xffb8_a890),
                Edge::Bottom => assert_eq!(pixel(&canvas, 10, 2, 9), 0xffb8_a890),
                Edge::Left => assert_eq!(pixel(&canvas, 10, 0, 7), 0xffb8_a890),
            }
        }
    }

    #[test]
    fn animation_masks_follow_local_coordinates_and_declarative_opacity() {
        let config = Config::default();
        let frame = TimerFrame {
            started_ns: 0,
            deadline_ns: 10 * SECOND_NS,
            now_ns: 5 * SECOND_NS,
        };
        let active = |animation: &str, y, now_ns| {
            let style = TimerConfig {
                edge: Edge::Left,
                animation: animation.into(),
                ..config.timer.clone()
            };
            timer_pixel(
                0,
                y,
                &style,
                timer_bounds(style.clone(), 10, 10, 0, config.highlight_thickness_px).unwrap(),
                TimerFrame { now_ns, ..frame },
                &config,
            )
            .is_some()
        };
        assert!(active("flow-up", 9, frame.now_ns));
        assert!(!active("flow-up", 0, frame.now_ns));
        assert!(!active("flow-down", 9, frame.now_ns));
        assert!(active("flow-down", 1, frame.now_ns));
        assert!(!active("expand", 9, frame.now_ns));
        assert!(active("expand", 4, frame.now_ns));
        let style = TimerConfig {
            edge: Edge::Left,
            animation: "flow-up".into(),
            ..config.timer.clone()
        };
        assert_eq!(
            timer_pixel(
                0,
                9,
                &style,
                timer_bounds(style.clone(), 10, 10, 0, config.highlight_thickness_px).unwrap(),
                TimerFrame {
                    now_ns: 4 * SECOND_NS,
                    ..frame
                },
                &config
            ),
            Some(255)
        );
    }

    #[test]
    fn signed_overflow_follows_each_hard_and_rounded_corner_once() {
        for radius in [0, 2] {
            let config = Config {
                corner_radius_px: radius,
                ..Config::default()
            };
            for edge in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
                let style = TimerConfig {
                    edge,
                    start: Coordinate::Percent(-100),
                    end: Coordinate::Percent(200),
                    ..config.timer.clone()
                };
                assert!(timer_bounds(
                    style,
                    40,
                    40,
                    if radius == 0 {
                        0
                    } else {
                        config.border_thickness_px + radius
                    },
                    config.highlight_thickness_px
                )
                .is_ok());
            }
        }
        let config = Config::default();
        assert!(timer_bounds(
            TimerConfig {
                start: Coordinate::Percent(-100),
                end: Coordinate::Percent(300),
                ..config.timer.clone()
            },
            20,
            20,
            0,
            config.highlight_thickness_px
        )
        .is_err());
    }

    #[test]
    fn frame_scale_padding_and_buffer_cap() {
        let mut frame = vec![0; 40 * 40 * 4 + 7];
        paint(&mut frame, 40, 40, 1, &Config::default(), None);
        assert!(frame[6400..].iter().all(|byte| *byte == 0));
        assert_eq!(pixel(&frame, 40, 0, 0), 0xff00_0000);
        assert_eq!(pixel(&frame, 40, 20, 20), 0);
        let mut scaled = vec![0; 80 * 80 * 4];
        paint(&mut scaled, 80, 80, 2, &Config::default(), None);
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
