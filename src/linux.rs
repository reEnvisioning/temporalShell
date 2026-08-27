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

use crate::core::{buffer_dimensions, paint, Config, Edge, SHADOW_WIDTH};

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

pub(crate) enum RunError {
    Shell(String),
    Unavailable(String),
}

pub(crate) fn run(command: &crate::cli::Command) -> Result<(), RunError> {
    match command {
        crate::cli::Command::Shell => shell().map_err(RunError::Shell),
        crate::cli::Command::Available => available_command().map_err(RunError::Unavailable),
        crate::cli::Command::Help | crate::cli::Command::Timer(_) => Ok(()),
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
        let corner_strip = border + self.config.corner_radius_px + SHADOW_WIDTH;
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
                layer.set_size(border + SHADOW_WIDTH, 0);
            }
            Edge::Right => {
                layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::RIGHT);
                layer.set_size(border + SHADOW_WIDTH, 0);
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
            self.config,
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
