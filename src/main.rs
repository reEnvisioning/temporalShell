use std::{env, fs, io, path::PathBuf};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, EventQueue, QueueHandle,
};

const DEFAULT_BORDER: u32 = 10;
const MAX_BORDER: u32 = 256;
const RADIUS: u32 = 16;
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Config {
    border_thickness_px: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            border_thickness_px: DEFAULT_BORDER,
        }
    }
}

impl Config {
    fn load() -> Result<Self, String> {
        let path = config_path()?;
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
            return Err(format!(
                "{} must be a regular file no larger than 64 KiB",
                path.display()
            ));
        }
        parse_config(
            &fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
        )
        .map_err(|error| format!("{}: {error}", path.display()))
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
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected key = value", index + 1))?;
        if key.trim() != "border_thickness_px" {
            return Err(format!("line {}: unknown key", index + 1));
        }
        if border.is_some() {
            return Err(format!("line {}: duplicate key", index + 1));
        }
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "line {}: border_thickness_px must be an integer",
                index + 1
            ));
        }
        let value: u32 = value
            .parse()
            .map_err(|_| format!("line {}: border_thickness_px is out of range", index + 1))?;
        if !(1..=MAX_BORDER).contains(&value) {
            return Err(format!(
                "line {}: border_thickness_px must be between 1 and {MAX_BORDER}",
                index + 1
            ));
        }
        border = Some(value);
    }
    Ok(Config {
        border_thickness_px: border.unwrap_or(DEFAULT_BORDER),
    })
}

#[derive(Clone, Copy)]
enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

struct Strip {
    edge: Edge,
    layer: LayerSurface,
    size: Option<(u32, u32)>,
    scale: i32,
    buffer: Option<Buffer>,
    buffer_size: Option<(i32, i32)>,
}

struct OutputBorders {
    output: wl_output::WlOutput,
    strips: [Strip; 4],
}

struct App {
    registry_state: RegistryState,
    compositor: CompositorState,
    output_state: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    pool: SlotPool,
    config: Config,
    outputs: Vec<OutputBorders>,
    error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Shell,
    Available,
}

enum RunError {
    Usage,
    Shell(String),
    Unavailable(String),
}

fn main() {
    if let Err(error) = run(env::args().skip(1)) {
        match error {
            RunError::Usage => eprintln!("temporalShell: usage: temporalShell [available]"),
            RunError::Shell(reason) => eprintln!("temporalShell: {reason}"),
            RunError::Unavailable(reason) => eprintln!("unavailable: {reason}"),
        }
        std::process::exit(1);
    }
}

fn run(args: impl IntoIterator<Item = String>) -> Result<(), RunError> {
    match parse(args).map_err(|()| RunError::Usage)? {
        Action::Shell => shell().map_err(RunError::Shell),
        Action::Available => available_command().map_err(RunError::Unavailable),
    }
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Action, ()> {
    let mut args = args.into_iter();
    match (args.next().as_deref(), args.next()) {
        (None, None) => Ok(Action::Shell),
        (Some("available"), None) => Ok(Action::Available),
        _ => Err(()),
    }
}

fn initialize(config: Config) -> Result<(EventQueue<App>, App), String> {
    let connection = Connection::connect_to_env()
        .map_err(|error| format!("cannot connect to a Wayland display: {error}"))?;
    let (globals, queue) = registry_queue_init(&connection)
        .map_err(|error| format!("cannot read Wayland globals: {error}"))?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|error| format!("missing wl_compositor capability: {error}"))?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|error| {
        format!("compositor does not advertise wlr-layer-shell; use a compositor that implements zwlr_layer_shell_v1: {error}")
    })?;
    let shm =
        Shm::bind(&globals, &qh).map_err(|error| format!("missing wl_shm capability: {error}"))?;
    let pool = SlotPool::new(1, &shm).map_err(|error| error.to_string())?;
    Ok((
        queue,
        App {
            registry_state: RegistryState::new(&globals),
            compositor,
            output_state: OutputState::new(&globals, &qh),
            shm,
            layer_shell,
            pool,
            config,
            outputs: Vec::new(),
            error: None,
        },
    ))
}

fn available_command() -> Result<(), String> {
    let (_, app) = initialize(Config::default())?;
    if app.output_state.outputs().next().is_none() {
        return Err("no Wayland outputs available".into());
    }
    println!("available");
    Ok(())
}

fn shell() -> Result<(), String> {
    let (mut queue, mut app) = initialize(Config::load()?)?;
    loop {
        queue
            .blocking_dispatch(&mut app)
            .map_err(|error| error.to_string())?;
        if let Some(error) = app.error.take() {
            return Err(error);
        }
        app.refresh_pending()?;
    }
}

impl App {
    fn create_strip(
        &self,
        qh: &QueueHandle<Self>,
        output: &wl_output::WlOutput,
        edge: Edge,
    ) -> Result<Strip, String> {
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Top,
            Some("temporalshell"),
            Some(output),
        );
        let border = self.config.border_thickness_px;
        let corner_strip = border + RADIUS;
        match edge {
            Edge::Top => {
                layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
                layer.set_size(0, corner_strip);
            }
            Edge::Bottom => {
                layer.set_anchor(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
                layer.set_size(0, corner_strip);
            }
            Edge::Left => {
                layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT);
                layer.set_size(border, 0);
            }
            Edge::Right => {
                layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::RIGHT);
                layer.set_size(border, 0);
            }
        }
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let region = Region::new(&self.compositor).map_err(|error| error.to_string())?;
        layer
            .wl_surface()
            .set_input_region(Some(region.wl_region()));
        layer.commit();
        Ok(Strip {
            edge,
            layer,
            size: None,
            scale: 1,
            buffer: None,
            buffer_size: None,
        })
    }

    fn strip_index(&self, layer: &LayerSurface) -> Option<usize> {
        self.outputs
            .iter()
            .position(|borders| borders.strips.iter().any(|strip| strip.layer == *layer))
            .and_then(|output_index| {
                self.outputs[output_index]
                    .strips
                    .iter()
                    .position(|strip| strip.layer == *layer)
                    .map(|strip_index| output_index * 4 + strip_index)
            })
    }

    fn strip_mut(&mut self, index: usize) -> &mut Strip {
        &mut self.outputs[index / 4].strips[index % 4]
    }

    fn redraw(&mut self, index: usize) -> Result<(), String> {
        let (edge, width, height, scale) = match self.strip_mut(index) {
            Strip {
                edge,
                size: Some((width, height)),
                scale,
                ..
            } => (*edge, *width, *height, *scale),
            _ => return Ok(()),
        };
        let (pixel_width, pixel_height, stride, bytes) = buffer_dimensions(width, height, scale)?;
        let strip = self.strip_mut(index);
        let old_buffer = strip.buffer.take();
        let old_size = strip.buffer_size.take();
        if let Some(buffer) = old_buffer {
            if old_size == Some((pixel_width, pixel_height)) {
                let strip = self.strip_mut(index);
                strip.buffer = Some(buffer);
                strip.buffer_size = old_size;
                return Ok(());
            }
            if buffer.canvas(&mut self.pool).is_none() {
                let strip = self.strip_mut(index);
                strip.buffer = Some(buffer);
                strip.buffer_size = old_size;
                return Ok(());
            }
        }
        let (buffer, canvas) = self
            .pool
            .create_buffer(pixel_width, pixel_height, stride, wl_shm::Format::Argb8888)
            .map_err(|error| error.to_string())?;
        if canvas.len() < bytes {
            return Err("shared-memory buffer is smaller than requested".into());
        }
        paint(
            canvas,
            edge,
            pixel_width as u32,
            pixel_height as u32,
            scale as u32,
            self.config.border_thickness_px,
        );
        let layer = self.strip_mut(index).layer.clone();
        layer
            .wl_surface()
            .damage_buffer(0, 0, pixel_width, pixel_height);
        buffer
            .attach_to(layer.wl_surface())
            .map_err(|error| error.to_string())?;
        layer.commit();
        let strip = self.strip_mut(index);
        strip.buffer = Some(buffer);
        strip.buffer_size = Some((pixel_width, pixel_height));
        Ok(())
    }

    fn refresh_pending(&mut self) -> Result<(), String> {
        for index in 0..self.outputs.len() * 4 {
            self.redraw(index)?;
        }
        Ok(())
    }

    fn remove_output_for_layer(&mut self, layer: &LayerSurface) {
        if let Some(index) = self
            .outputs
            .iter()
            .position(|borders| borders.strips.iter().any(|strip| strip.layer == *layer))
        {
            self.outputs.remove(index);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        scale: i32,
    ) {
        if scale <= 0 {
            self.error = Some("compositor supplied a non-positive buffer scale".into());
            return;
        }
        for output in &mut self.outputs {
            for strip in &mut output.strips {
                if strip.layer.wl_surface() == surface {
                    strip.scale = scale;
                    surface.set_buffer_scale(scale);
                }
            }
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.outputs.iter().any(|borders| borders.output == output) {
            return;
        }
        let strips = (|| {
            Ok([
                self.create_strip(qh, &output, Edge::Top)?,
                self.create_strip(qh, &output, Edge::Bottom)?,
                self.create_strip(qh, &output, Edge::Left)?,
                self.create_strip(qh, &output, Edge::Right)?,
            ])
        })();
        match strips {
            Ok(strips) => self.outputs.push(OutputBorders { output, strips }),
            Err(error) => self.error = Some(error),
        }
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some(index) = self
            .outputs
            .iter()
            .position(|borders| borders.output == output)
        {
            self.outputs.remove(index);
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.remove_output_for_layer(layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        if configure.new_size.0 == 0 || configure.new_size.1 == 0 {
            self.error = Some("compositor configured a zero-sized border strip".into());
            return;
        }
        if let Some(index) = self.strip_index(layer) {
            self.strip_mut(index).size = Some(configure.new_size);
            if let Err(error) = self.redraw(index) {
                self.error = Some(error);
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_layer!(App);
delegate_registry!(App);

fn buffer_dimensions(
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

fn paint(canvas: &mut [u8], edge: Edge, width: u32, height: u32, scale: u32, border: u32) {
    for (pixel, chunk) in canvas.chunks_exact_mut(4).enumerate() {
        let x = (pixel as u32 % width) / scale;
        let y = (pixel as u32 / width) / scale;
        let color = match edge {
            Edge::Left | Edge::Right
                if y >= border + RADIUS && y < (height / scale).saturating_sub(border + RADIUS) =>
            {
                0xff00_0000
            }
            Edge::Left | Edge::Right => 0,
            Edge::Top => top_pixel(x, y, width / scale, border),
            Edge::Bottom => top_pixel(x, height / scale - 1 - y, width / scale, border),
        };
        chunk.copy_from_slice(&color.to_le_bytes());
    }
}

fn top_pixel(x: u32, y: u32, width: u32, border: u32) -> u32 {
    if y < border {
        return 0xff00_0000;
    }
    let corner_strip = border + RADIUS;
    if x >= corner_strip && x < width.saturating_sub(corner_strip) {
        return 0;
    }
    let corner_x = if x < corner_strip { x } else { width - 1 - x } as i64;
    let dx = corner_x - i64::from(corner_strip);
    let dy = i64::from(y) - i64::from(corner_strip);
    if dx * dx + dy * dy <= i64::from(RADIUS).pow(2) {
        0
    } else {
        0xff00_0000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shell_and_available() {
        assert_eq!(parse(Vec::<String>::new()), Ok(Action::Shell));
        assert_eq!(parse(["available".into()]), Ok(Action::Available));
    }

    #[test]
    fn rejects_invalid_arguments() {
        assert!(parse(["--help".into()]).is_err());
        assert!(parse(["available".into(), "extra".into()]).is_err());
    }

    #[test]
    fn config_is_strict_and_defaults() {
        assert_eq!(parse_config("").unwrap().border_thickness_px, 10);
        assert_eq!(
            parse_config("# border\nborder_thickness_px = 24\n")
                .unwrap()
                .border_thickness_px,
            24
        );
        for invalid in [
            "border_thickness_px = nope",
            "border_thickness_px = 0",
            "border_thickness_px = 257",
            "unknown = 10",
            "border_thickness_px = 10\nborder_thickness_px = 11",
            "border_thickness_px 10",
        ] {
            assert!(parse_config(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn border_geometry_matches_the_reference() {
        let corner_strip = DEFAULT_BORDER + RADIUS;
        assert_eq!(top_pixel(100, 0, 200, 10), 0xff00_0000);
        assert_eq!(top_pixel(100, 9, 200, 10), 0xff00_0000);
        assert_eq!(top_pixel(100, 10, 200, 10), 0);
        assert_eq!(top_pixel(100, 25, 200, 10), 0);
        assert_eq!(top_pixel(10, 25, 200, 10), 0xff00_0000);
        assert_eq!(top_pixel(11, 25, 200, 10), 0);
        for y in 0..corner_strip {
            assert_eq!(top_pixel(0, y, 200, 10), top_pixel(199, y, 200, 10));
        }
        assert_eq!(top_pixel(0, 20, 8, 10), top_pixel(7, 20, 8, 10));
    }

    #[test]
    fn sides_and_bottom_are_black_and_mirrored() {
        let mut top = [0_u8; 52 * 26 * 4];
        let mut bottom = top;
        let mut side = [0_u8; 10 * 26 * 4];
        paint(&mut top, Edge::Top, 52, 26, 1, 10);
        paint(&mut bottom, Edge::Bottom, 52, 26, 1, 10);
        paint(&mut side, Edge::Left, 10, 26, 1, 10);
        for y in 0..26 {
            for x in 0..52 {
                assert_eq!(
                    &top[((y * 52 + x) * 4) as usize..][..4],
                    &bottom[(((25 - y) * 52 + x) * 4) as usize..][..4]
                );
            }
        }
        assert!(side
            .chunks_exact(4)
            .all(|pixel| pixel == 0_u32.to_le_bytes()));
        let mut tall_side = [0_u8; 10 * 60 * 4];
        paint(&mut tall_side, Edge::Left, 10, 60, 1, 10);
        assert_eq!(&tall_side[..4], &0_u32.to_le_bytes());
        assert_eq!(
            &tall_side[(30 * 10 * 4)..][..4],
            &0xff00_0000_u32.to_le_bytes()
        );
    }

    #[test]
    fn integer_scale_is_logically_equivalent() {
        let mut one = [0_u8; 52 * 26 * 4];
        let mut two = [0_u8; 104 * 52 * 4];
        paint(&mut one, Edge::Top, 52, 26, 1, 10);
        paint(&mut two, Edge::Top, 104, 52, 2, 10);
        for y in 0..26 {
            for x in 0..52 {
                assert_eq!(
                    &one[((y * 52 + x) * 4) as usize..][..4],
                    &two[(((y * 2 * 104) + x * 2) * 4) as usize..][..4]
                );
            }
        }
    }

    #[test]
    fn invalid_buffers_are_rejected() {
        assert!(buffer_dimensions(0, 16, 1).is_err());
        assert!(buffer_dimensions(u32::MAX, 16, 1).is_err());
        assert!(buffer_dimensions(16, 16, 0).is_err());
    }
}
