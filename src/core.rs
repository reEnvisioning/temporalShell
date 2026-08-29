use std::{
    env, fs,
    io::{self, Read},
    path::PathBuf,
};

const DEFAULT_BORDER: u32 = 10;
const DEFAULT_EVENT_LINE_THICKNESS: u32 = 3;
const DEFAULT_CORNER_RADIUS: u32 = 0;
const DEFAULT_SHADOW_STRENGTH_PERCENT: u32 = 35;
const DEFAULT_SHADOW_COLOR: u32 = 0xb8a890;
pub(crate) const SHADOW_WIDTH: u32 = 3;
const SHADOW: [u32; SHADOW_WIDTH as usize] = [0x4d, 0x33, 0x1a];
const MAX_CONFIG_VALUE: u32 = 256;
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) border_thickness_px: u32,
    pub(crate) event_line_thickness_px: u32,
    pub(crate) corner_radius_px: u32,
    pub(crate) shadow_strength_percent: u32,
    pub(crate) shadow_color: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            border_thickness_px: DEFAULT_BORDER,
            event_line_thickness_px: DEFAULT_EVENT_LINE_THICKNESS,
            corner_radius_px: DEFAULT_CORNER_RADIUS,
            shadow_strength_percent: DEFAULT_SHADOW_STRENGTH_PERCENT,
            shadow_color: DEFAULT_SHADOW_COLOR,
        }
    }
}

impl Config {
    pub(crate) fn load() -> Result<Self, String> {
        let path = config_path()?;
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
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
    let mut border = None;
    let mut event_line = None;
    let mut radius = None;
    let mut shadow_strength = None;
    let mut shadow_color = None;
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected key = value", index + 1))?;
        let key = key.trim();
        let value = value.trim();
        match key {
            "shadow_color" => {
                if shadow_color.is_some() {
                    return Err(format!("line {}: duplicate key", index + 1));
                }
                let color = value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .filter(|value| {
                        value.len() == 7
                            && value.starts_with('#')
                            && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                    .ok_or_else(|| {
                        format!("line {}: shadow_color must be quoted #RRGGBB", index + 1)
                    })?;
                shadow_color = Some(
                    u32::from_str_radix(&color[1..], 16)
                        .map_err(|_| format!("line {}: shadow_color is invalid", index + 1))?,
                );
            }
            "border_thickness_px"
            | "event_line_thickness_px"
            | "corner_radius_px"
            | "shadow_strength_percent" => {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(format!("line {}: {key} must be an integer", index + 1));
                }
                let value: u32 = value
                    .parse()
                    .map_err(|_| format!("line {}: {key} is out of range", index + 1))?;
                match key {
                    "border_thickness_px" => {
                        if border.is_some() {
                            return Err(format!("line {}: duplicate key", index + 1));
                        }
                        if !(1..=MAX_CONFIG_VALUE).contains(&value) {
                            return Err(format!(
                        "line {}: border_thickness_px must be between 1 and {MAX_CONFIG_VALUE}",
                        index + 1
                    ));
                        }
                        border = Some(value);
                    }
                    "event_line_thickness_px" => {
                        if event_line.is_some() {
                            return Err(format!("line {}: duplicate key", index + 1));
                        }
                        event_line = Some(value);
                    }
                    "corner_radius_px" => {
                        if radius.is_some() {
                            return Err(format!("line {}: duplicate key", index + 1));
                        }
                        if value > MAX_CONFIG_VALUE {
                            return Err(format!(
                        "line {}: corner_radius_px must be between 0 and {MAX_CONFIG_VALUE}",
                        index + 1
                    ));
                        }
                        radius = Some(value);
                    }
                    "shadow_strength_percent" => {
                        if shadow_strength.is_some() {
                            return Err(format!("line {}: duplicate key", index + 1));
                        }
                        if value > 100 {
                            return Err(format!(
                                "line {}: shadow_strength_percent must be between 0 and 100",
                                index + 1
                            ));
                        }
                        shadow_strength = Some(value);
                    }
                    _ => unreachable!(),
                }
            }
            _ => return Err(format!("line {}: unknown key", index + 1)),
        }
    }
    let border_thickness_px = border.unwrap_or(DEFAULT_BORDER);
    let event_line_thickness_px =
        event_line.unwrap_or(DEFAULT_EVENT_LINE_THICKNESS.min(border_thickness_px));
    if !(1..=border_thickness_px).contains(&event_line_thickness_px) {
        return Err("event_line_thickness_px must be between 1 and border_thickness_px".into());
    }
    Ok(Config {
        border_thickness_px,
        event_line_thickness_px,
        corner_radius_px: radius.unwrap_or(DEFAULT_CORNER_RADIUS),
        shadow_strength_percent: shadow_strength.unwrap_or(DEFAULT_SHADOW_STRENGTH_PERCENT),
        shadow_color: shadow_color.unwrap_or(DEFAULT_SHADOW_COLOR),
    })
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

pub(crate) fn paint(canvas: &mut [u8], width: u32, height: u32, scale: u32, config: Config) {
    let logical_width = width / scale;
    let logical_height = height / scale;
    for (pixel, chunk) in canvas
        .chunks_exact_mut(4)
        .take(width as usize * height as usize)
        .enumerate()
    {
        let x = (pixel as u32 % width) / scale;
        let y = (pixel as u32 / width) / scale;
        chunk.copy_from_slice(
            &frame_pixel(x, y, logical_width, logical_height, config).to_le_bytes(),
        );
    }
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

fn frame_pixel(x: u32, y: u32, width: u32, height: u32, config: Config) -> u32 {
    let near_x = x.min(width - 1 - x);
    let near_y = y.min(height - 1 - y);
    if config.corner_radius_px != 0 {
        let corner = config.border_thickness_px + config.corner_radius_px;
        if near_x < corner && near_y < corner {
            let dx = i64::from(near_x) - i64::from(corner);
            let dy = i64::from(near_y) - i64::from(corner);
            let distance_squared = dx * dx + dy * dy;
            if distance_squared > i64::from(config.corner_radius_px).pow(2) {
                return 0xff00_0000;
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
        0xff00_0000
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

    #[test]
    fn config_is_strict_and_defaults() {
        assert_eq!(
            parse_config("").unwrap(),
            Config {
                border_thickness_px: 10,
                event_line_thickness_px: 3,
                corner_radius_px: 0,
                shadow_strength_percent: 35,
                shadow_color: 0xb8a890,
            }
        );
        assert_eq!(
            parse_config(
                "# border\nborder_thickness_px = 24\nevent_line_thickness_px = 8\ncorner_radius_px = 8\nshadow_strength_percent = 0\nshadow_color = \"#a1B2c3\"\n"
            )
            .unwrap(),
            Config {
                border_thickness_px: 24,
                event_line_thickness_px: 8,
                corner_radius_px: 8,
                shadow_strength_percent: 0,
                shadow_color: 0xa1b2c3,
            }
        );
        assert_eq!(
            parse_config("shadow_strength_percent = 100\nshadow_color = \"#ABCDEF\"").unwrap(),
            Config {
                shadow_strength_percent: 100,
                shadow_color: 0xabcdef,
                ..Config::default()
            }
        );
        for (border, line) in [(1, 1), (2, 2), (4, 3), (10, 3)] {
            assert_eq!(
                parse_config(&format!("border_thickness_px = {border}"))
                    .unwrap()
                    .event_line_thickness_px,
                line
            );
        }
        for invalid in [
            "border_thickness_px = nope",
            "border_thickness_px = -1",
            "border_thickness_px = 0",
            "border_thickness_px = 257",
            "event_line_thickness_px = 0",
            "event_line_thickness_px = 11",
            "border_thickness_px = 7\nevent_line_thickness_px = 8",
            "border_thickness_px = 8\nevent_line_thickness_px = 10",
            "event_line_thickness_px = 3\nevent_line_thickness_px = 5",
            "corner_radius_px = -1",
            "corner_radius_px = 257",
            "shadow_strength_percent = -1",
            "shadow_strength_percent = 101",
            "shadow_strength_percent = 1.0",
            "shadow_color = #000000",
            "shadow_color = \"000000\"",
            "shadow_color = \"#00000\"",
            "shadow_color = \"#00000g\"",
            "shadow_color = \"#000\"",
            "shadow_color = \"#00000000\"",
            "shadow_color = \"#000000\" trailing",
            "unknown = 10",
            "shadow_strength = 30",
            "border_thickness_px = 10\nborder_thickness_px = 11",
            "corner_radius_px = 10\ncorner_radius_px = 11",
            "shadow_strength_percent = 10\nshadow_strength_percent = 11",
            "shadow_color = \"#000000\"\nshadow_color = \"#ffffff\"",
            "border_thickness_px 10",
        ] {
            assert!(parse_config(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    fn pixel(canvas: &[u8], width: u32, x: u32, y: u32) -> u32 {
        let start = ((y * width + x) * 4) as usize;
        u32::from_le_bytes(canvas[start..start + 4].try_into().unwrap())
    }

    #[test]
    fn event_line_config_does_not_change_raster() {
        let mut default_line = vec![0; 20 * 20 * 4];
        let mut alternate_line = vec![0; 20 * 20 * 4];
        paint(&mut default_line, 20, 20, 1, Config::default());
        paint(
            &mut alternate_line,
            20,
            20,
            1,
            Config {
                event_line_thickness_px: 1,
                ..Config::default()
            },
        );
        assert_eq!(default_line, alternate_line);
    }

    #[test]
    fn shadow_strength_and_color_are_premultiplied() {
        assert_eq!(
            shadow(
                0,
                Config::default().shadow_strength_percent,
                Config::default().shadow_color
            ),
            0x1b13_120f
        );
        assert_eq!(
            shadow(
                1,
                Config::default().shadow_strength_percent,
                Config::default().shadow_color
            ),
            0x120d_0c0a
        );
        assert_eq!(
            shadow(
                2,
                Config::default().shadow_strength_percent,
                Config::default().shadow_color
            ),
            0x0906_0605
        );
        assert_eq!(shadow(0, 100, 0), 0x4d00_0000);
        assert_eq!(shadow(1, 100, 0), 0x3300_0000);
        assert_eq!(shadow(2, 100, 0), 0x1a00_0000);
        assert_eq!(shadow(0, 50, 0), 0x2700_0000);
        assert_eq!(shadow(1, 50, 0), 0x1a00_0000);
        assert_eq!(shadow(2, 50, 0), 0x0d00_0000);
        assert_eq!(shadow(0, 100, 0xff0000), 0x4d4d_0000);
        assert_eq!(shadow(0, 100, 0xffffff), 0x4d4d_4d4d);
        assert_eq!(shadow(0, 0, 0xffffff), 0);
        assert_eq!(
            frame_pixel(
                40,
                6,
                80,
                80,
                Config {
                    shadow_strength_percent: 0,
                    shadow_color: 0xffffff,
                    ..Config::default()
                },
            ),
            0xff00_0000
        );
    }

    #[test]
    fn full_frame_has_continuous_edges_and_inward_shadows() {
        let config = Config {
            shadow_strength_percent: 100,
            shadow_color: 0,
            ..Config::default()
        };
        let mut frame = vec![0; 80 * 80 * 4];
        paint(&mut frame, 80, 80, 1, config);
        for coordinate in 0..80 {
            assert_eq!(pixel(&frame, 80, coordinate, 0), 0xff00_0000);
            assert_eq!(pixel(&frame, 80, coordinate, 79), 0xff00_0000);
            assert_eq!(pixel(&frame, 80, 0, coordinate), 0xff00_0000);
            assert_eq!(pixel(&frame, 80, 79, coordinate), 0xff00_0000);
        }
        assert_eq!(pixel(&frame, 80, 40, 40), 0);
        for (x, y) in [(40, 10), (40, 69), (10, 40), (69, 40)] {
            assert_eq!(pixel(&frame, 80, x, y), 0x4d00_0000);
        }
        for (x, y) in [(40, 11), (40, 68), (11, 40), (68, 40)] {
            assert_eq!(pixel(&frame, 80, x, y), 0x3300_0000);
        }
        for (x, y) in [(40, 12), (40, 67), (12, 40), (67, 40)] {
            assert_eq!(pixel(&frame, 80, x, y), 0x1a00_0000);
        }
    }

    #[test]
    fn rounded_corners_are_symmetric_and_scale_exactly() {
        let rounded = Config {
            border_thickness_px: 7,
            corner_radius_px: 16,
            shadow_strength_percent: 100,
            shadow_color: 0,
            ..Config::default()
        };
        let mut frame = vec![0; 80 * 80 * 4];
        paint(&mut frame, 80, 80, 1, rounded);
        assert_eq!(pixel(&frame, 80, 0, 0), 0xff00_0000);
        assert_eq!(pixel(&frame, 80, 12, 12), 0x4d00_0000);
        assert_eq!(pixel(&frame, 80, 13, 13), 0x3300_0000);
        assert_eq!(pixel(&frame, 80, 14, 14), 0);
        for y in 0..80 {
            for x in 0..80 {
                assert_eq!(pixel(&frame, 80, x, y), pixel(&frame, 80, 79 - x, y));
                assert_eq!(pixel(&frame, 80, x, y), pixel(&frame, 80, x, 79 - y));
            }
        }

        let mut scaled = vec![0; 160 * 160 * 4];
        paint(&mut scaled, 160, 160, 2, rounded);
        for y in 0..80 {
            for x in 0..80 {
                let expected = pixel(&frame, 80, x, y);
                for scale_y in 0..2 {
                    for scale_x in 0..2 {
                        assert_eq!(
                            pixel(&scaled, 160, x * 2 + scale_x, y * 2 + scale_y),
                            expected
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn paint_ignores_pool_padding() {
        let width = 81;
        let height = 79;
        let bytes = width * height * 4;
        let mut canvas = vec![0xaa; bytes + 63];
        paint(
            &mut canvas,
            width as u32,
            height as u32,
            1,
            Config::default(),
        );
        assert!(canvas[bytes..].iter().all(|byte| *byte == 0xaa));
    }

    #[test]
    fn buffer_cap_accepts_4k_and_5k_but_rejects_8k_and_overflow() {
        assert!(buffer_dimensions(3840, 2160, 1).is_ok());
        assert!(buffer_dimensions(5120, 2880, 1).is_ok());
        assert!(buffer_dimensions(4096, 4096, 1).is_ok());
        assert!(buffer_dimensions(3840, 2160, 2).is_err());
        assert!(buffer_dimensions(7680, 4320, 1).is_err());
        assert!(buffer_dimensions(4096, 4097, 1).is_err());
        assert!(buffer_dimensions(0, 16, 1).is_err());
        assert!(buffer_dimensions(u32::MAX, 16, 1).is_err());
        assert!(buffer_dimensions(16, 16, 0).is_err());
    }
}
