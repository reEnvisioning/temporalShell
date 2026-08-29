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

use crate::core::{buffer_dimensions, paint, Config};

#[derive(Clone, Copy, PartialEq, Eq)]
struct RenderKey {
    width: u32,
    height: u32,
    scale: i32,
}

struct OutputBorder {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    size: Option<(u32, u32)>,
    scale: i32,
    pool: SlotPool,
    buffer: Option<Buffer>,
    render_key: Option<RenderKey>,
}

struct App {
    registry_state: RegistryState,
    compositor: CompositorState,
    output_state: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    config: Config,
    outputs: Vec<OutputBorder>,
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
    Ok((
        queue,
        App {
            registry_state: RegistryState::new(&globals),
            compositor,
            output_state: OutputState::new(&globals, &qh),
            shm,
            layer_shell,
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
    fn create_output_surface(
        &self,
        qh: &QueueHandle<Self>,
        output: &wl_output::WlOutput,
    ) -> Result<OutputBorder, String> {
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Top,
            Some("temporalshell"),
            Some(output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let region = Region::new(&self.compositor).map_err(|error| error.to_string())?;
        layer
            .wl_surface()
            .set_input_region(Some(region.wl_region()));
        layer.commit();
        Ok(OutputBorder {
            output: output.clone(),
            layer,
            size: None,
            scale: 1,
            pool: SlotPool::new(1, &self.shm).map_err(|error| error.to_string())?,
            buffer: None,
            render_key: None,
        })
    }

    fn output_index(&self, layer: &LayerSurface) -> Option<usize> {
        self.outputs
            .iter()
            .position(|output| output.layer == *layer)
    }

    fn redraw(&mut self, index: usize) -> Result<(), String> {
        let (width, height, scale) = match self.outputs[index].size {
            Some((width, height)) => (width, height, self.outputs[index].scale),
            None => return Ok(()),
        };
        let key = RenderKey {
            width,
            height,
            scale,
        };
        if self.outputs[index].render_key == Some(key) {
            return Ok(());
        }
        let (pixel_width, pixel_height, stride, bytes) = buffer_dimensions(width, height, scale)?;
        let output = &mut self.outputs[index];
        if let Some(buffer) = output.buffer.take() {
            if buffer.canvas(&mut output.pool).is_none() {
                output.buffer = Some(buffer);
                return Ok(());
            }
        }
        let (buffer, canvas) = output
            .pool
            .create_buffer(pixel_width, pixel_height, stride, wl_shm::Format::Argb8888)
            .map_err(|error| error.to_string())?;
        if canvas.len() < bytes {
            return Err("shared-memory buffer is smaller than requested".into());
        }
        paint(
            canvas,
            pixel_width as u32,
            pixel_height as u32,
            scale as u32,
            self.config,
        );
        output
            .layer
            .wl_surface()
            .damage_buffer(0, 0, pixel_width, pixel_height);
        buffer
            .attach_to(output.layer.wl_surface())
            .map_err(|error| error.to_string())?;
        output.layer.commit();
        output.buffer = Some(buffer);
        output.render_key = Some(key);
        Ok(())
    }

    fn refresh_pending(&mut self) -> Result<(), String> {
        for index in 0..self.outputs.len() {
            self.redraw(index)?;
        }
        Ok(())
    }

    fn remove_output_for_layer(&mut self, layer: &LayerSurface) {
        if let Some(index) = self.output_index(layer) {
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
            if output.layer.wl_surface() == surface {
                output.scale = scale;
                surface.set_buffer_scale(scale);
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
        if self.outputs.iter().any(|border| border.output == output) {
            return;
        }
        match self.create_output_surface(qh, &output) {
            Ok(border) => self.outputs.push(border),
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
            .position(|border| border.output == output)
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
            self.error = Some("compositor configured a zero-sized output border".into());
            return;
        }
        if let Some(index) = self.output_index(layer) {
            self.outputs[index].size = Some(configure.new_size);
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
