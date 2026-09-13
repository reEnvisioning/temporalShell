use std::{
    io::ErrorKind,
    time::{Duration, Instant},
};

use rustix::{
    event::{poll, PollFd, PollFlags, Timespec},
    io::Errno,
};
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
use time::OffsetDateTime;
use wayland_client::{
    backend::WaylandError,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, EventQueue, QueueHandle,
};

use crate::{
    core::{buffer_dimensions, paint_highlights, Animation, Config, TimerFrame},
    timer::ActiveHighlight,
};

const RELOAD_INTERVAL: Duration = Duration::from_millis(250);
const BLINK_INTERVAL_NS: i128 = 250_000_000;
const FADE_NS: i128 = 250_000_000;
const BUFFER_RETRY: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderKey {
    width: u32,
    height: u32,
    scale: i32,
}

struct RenderState {
    render_key: Option<RenderKey>,
    dirty: bool,
    frame_pending: bool,
    retry_at: Option<Instant>,
}

impl RenderState {
    fn new() -> Self {
        Self {
            render_key: None,
            dirty: true,
            frame_pending: false,
            retry_at: None,
        }
    }

    fn reset_after_detach(&mut self, key: RenderKey) {
        self.render_key = Some(key);
        self.dirty = false;
        self.frame_pending = false;
        self.retry_at = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IoDisposition {
    Retry,
    WaitWritable,
    Fatal,
}

fn io_disposition(kind: ErrorKind) -> IoDisposition {
    match kind {
        ErrorKind::Interrupted => IoDisposition::Retry,
        ErrorKind::WouldBlock => IoDisposition::WaitWritable,
        _ => IoDisposition::Fatal,
    }
}

fn needs_pool_reset(render_key: Option<RenderKey>, key: RenderKey) -> bool {
    render_key.is_some_and(|old_key| old_key != key)
}

struct BufferSlot {
    buffer: Buffer,
    key: RenderKey,
}

struct OutputBorder {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    size: Option<(u32, u32)>,
    scale: i32,
    pool: SlotPool,
    buffers: Vec<BufferSlot>,
    state: RenderState,
}

struct App {
    connection: Connection,
    registry_state: RegistryState,
    compositor: CompositorState,
    output_state: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    config: Config,
    shell_started_ns: i128,
    timer_highlights: Vec<ActiveHighlight>,
    trigger_highlights: Vec<ActiveHighlight>,
    outputs: Vec<OutputBorder>,
    error: Option<String>,
    config_diagnostic: Option<String>,
    timer_diagnostic: Option<String>,
    trigger_diagnostic: Option<String>,
    render_diagnostic: Option<String>,
    next_reload: Instant,
    visual_key: Option<Vec<(i128, String)>>,
}

pub(crate) enum RunError {
    Shell(String),
    Unavailable(String),
}

pub(crate) fn run(command: &crate::cli::Command) -> Result<(), RunError> {
    match command {
        crate::cli::Command::Shell => shell().map_err(RunError::Shell),
        crate::cli::Command::Available => available_command().map_err(RunError::Unavailable),
        crate::cli::Command::Help
        | crate::cli::Command::Timer(_)
        | crate::cli::Command::Trigger(_) => Ok(()),
    }
}

fn initialize(
    config: Config,
    shell_started_ns: i128,
    timer_highlights: Vec<ActiveHighlight>,
    trigger_highlights: Vec<ActiveHighlight>,
) -> Result<(EventQueue<App>, App), String> {
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
            connection: connection.clone(),
            registry_state: RegistryState::new(&globals),
            compositor,
            output_state: OutputState::new(&globals, &qh),
            shm,
            layer_shell,
            config,
            shell_started_ns,
            timer_highlights,
            trigger_highlights,
            outputs: Vec::new(),
            error: None,
            config_diagnostic: None,
            timer_diagnostic: None,
            trigger_diagnostic: None,
            render_diagnostic: None,
            next_reload: Instant::now() + RELOAD_INTERVAL,
            visual_key: None,
        },
    ))
}

fn available_command() -> Result<(), String> {
    let (_, app) = initialize(Config::default(), 0, Vec::new(), Vec::new())?;
    if app.output_state.outputs().next().is_none() {
        return Err("no Wayland outputs available".into());
    }
    println!("available");
    Ok(())
}

fn shell() -> Result<(), String> {
    let shell_started_ns = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let config = Config::load_or_create()?;
    let timer_highlights = crate::timer::snapshot_timers(config.timer, shell_started_ns)?;
    let (mut queue, mut app) = initialize(
        config,
        shell_started_ns,
        timer_highlights,
        crate::timer::snapshot_triggers()?,
    )?;
    loop {
        queue
            .dispatch_pending(&mut app)
            .map_err(|error| error.to_string())?;
        if let Some(error) = app.error.take() {
            return Err(error);
        }
        app.reload();
        app.update_visual_key();
        let qh = queue.handle();
        app.refresh_pending(&qh)?;
        let flush_pending = loop {
            match queue.flush() {
                Ok(()) => break false,
                Err(WaylandError::Io(error)) => match io_disposition(error.kind()) {
                    IoDisposition::Retry => continue,
                    IoDisposition::WaitWritable => break true,
                    IoDisposition::Fatal => return Err(error.to_string()),
                },
                Err(error) => return Err(error.to_string()),
            }
        };

        let guard = match queue.prepare_read() {
            Some(guard) => guard,
            None => continue,
        };
        let events = poll_wayland(&app, flush_pending)?;
        if events.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL) {
            return Err("Wayland connection poll failed".into());
        }
        if events.contains(PollFlags::IN) {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(error))
                    if matches!(
                        io_disposition(error.kind()),
                        IoDisposition::Retry | IoDisposition::WaitWritable
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

fn poll_wayland(app: &App, want_write: bool) -> Result<PollFlags, String> {
    let deadline = Instant::now() + app.timeout();
    loop {
        let timeout = deadline.saturating_duration_since(Instant::now());
        if timeout.is_zero() {
            return Ok(PollFlags::empty());
        }
        let requested = PollFlags::IN
            | if want_write {
                PollFlags::OUT
            } else {
                PollFlags::empty()
            };
        let backend = app.connection.backend();
        let mut fds = [PollFd::from_borrowed_fd(backend.poll_fd(), requested)];
        let timeout = Timespec::try_from(timeout).map_err(|_| "poll timeout is invalid")?;
        match poll(&mut fds, Some(&timeout)) {
            Ok(_) => return Ok(fds[0].revents()),
            Err(Errno::INTR) if Instant::now() < deadline => continue,
            Err(Errno::INTR) => return Ok(PollFlags::empty()),
            Err(error) => return Err(format!("Wayland poll failed: {error}")),
        }
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
            buffers: Vec::new(),
            state: RenderState::new(),
        })
    }

    fn output_index(&self, layer: &LayerSurface) -> Option<usize> {
        self.outputs
            .iter()
            .position(|output| output.layer == *layer)
    }

    fn active_highlights(&self) -> Vec<ActiveHighlight> {
        let mut highlights = self.timer_highlights.clone();
        highlights.extend(self.trigger_highlights.clone());
        highlights.sort_by(|left, right| {
            left.started_ns
                .cmp(&right.started_ns)
                .then_with(|| left.id.cmp(&right.id))
        });
        highlights
    }

    fn redraw(&mut self, qh: &QueueHandle<Self>, index: usize) -> Result<(), String> {
        let (width, height, scale) = match self.outputs[index].size {
            Some((width, height)) => (width, height, self.outputs[index].scale),
            None => return Ok(()),
        };
        let key = RenderKey {
            width,
            height,
            scale,
        };
        if !self.outputs[index].state.dirty && self.outputs[index].state.render_key == Some(key) {
            return Ok(());
        }
        let highlights = self.active_highlights();
        self.render_diagnostic = None;
        let now_ns = OffsetDateTime::now_utc().unix_timestamp_nanos();
        let animate = highlights.iter().any(|highlight| {
            let timer = highlight.style.unwrap_or(self.config.timer);
            needs_frame_callback(
                timer.animation,
                timer.fade,
                TimerFrame {
                    started_ns: highlight.started_ns,
                    deadline_ns: highlight.expires_ns,
                    now_ns,
                },
            )
        });
        let (pixel_width, pixel_height, stride, bytes) =
            match buffer_dimensions(width, height, scale) {
                Ok(dimensions) => dimensions,
                Err(error) => {
                    let output = &mut self.outputs[index];
                    output.layer.wl_surface().attach(None, 0, 0);
                    output.layer.commit();
                    output.state.reset_after_detach(key);
                    diagnose_once(
                        &mut self.render_diagnostic,
                        format!("highlight hidden on {width}x{height} output: {error}"),
                    );
                    return Ok(());
                }
            };
        if self.outputs[index]
            .buffers
            .iter()
            .any(|slot| needs_pool_reset(Some(slot.key), key))
        {
            let released = {
                let output = &mut self.outputs[index];
                output
                    .buffers
                    .iter()
                    .all(|slot| slot.buffer.canvas(&mut output.pool).is_some())
            };
            if !released {
                self.outputs[index].state.dirty = true;
                self.outputs[index].state.retry_at = Some(Instant::now() + BUFFER_RETRY);
                return Ok(());
            }
            self.outputs[index].buffers.clear();
            let pool = SlotPool::new(1, &self.shm).map_err(|error| error.to_string())?;
            self.outputs[index].pool = pool;
        }
        let output = &mut self.outputs[index];
        let buffer_index = output
            .buffers
            .iter()
            .position(|slot| slot.key == key && slot.buffer.canvas(&mut output.pool).is_some());
        let buffer_index = match buffer_index {
            Some(index) => index,
            None if output.buffers.len() < 2 => {
                let (buffer, _) = output
                    .pool
                    .create_buffer(pixel_width, pixel_height, stride, wl_shm::Format::Argb8888)
                    .map_err(|error| error.to_string())?;
                output.buffers.push(BufferSlot { buffer, key });
                output.buffers.len() - 1
            }
            None => {
                output.state.dirty = true;
                output.state.retry_at = Some(Instant::now() + BUFFER_RETRY);
                return Ok(());
            }
        };
        let canvas = output.buffers[buffer_index]
            .buffer
            .canvas(&mut output.pool)
            .ok_or("released shared-memory buffer became busy")?;
        if canvas.len() < bytes {
            return Err("shared-memory buffer is smaller than requested".into());
        }
        if let Err(error) = paint_highlights(
            canvas,
            pixel_width as u32,
            pixel_height as u32,
            scale as u32,
            &self.config,
            &highlights,
            now_ns,
        ) {
            output.layer.wl_surface().attach(None, 0, 0);
            output.layer.commit();
            output.state.reset_after_detach(key);
            diagnose_once(
                &mut self.render_diagnostic,
                format!(
                    "highlight hidden on {width}x{height} output: {error}; adjust timer start/end or use percentage bounds"
                ),
            );
            return Ok(());
        }
        if animate && !output.state.frame_pending {
            output
                .layer
                .wl_surface()
                .frame(qh, output.layer.wl_surface().clone());
            output.state.frame_pending = true;
        }
        output
            .layer
            .wl_surface()
            .damage_buffer(0, 0, pixel_width, pixel_height);
        output.buffers[buffer_index]
            .buffer
            .attach_to(output.layer.wl_surface())
            .map_err(|error| error.to_string())?;
        output.layer.commit();
        output.state.render_key = Some(key);
        output.state.dirty = false;
        output.state.retry_at = None;
        Ok(())
    }

    fn refresh_pending(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        for index in 0..self.outputs.len() {
            self.redraw(qh, index)?;
        }
        Ok(())
    }

    fn dirty_all(&mut self) {
        for output in &mut self.outputs {
            output.state.dirty = true;
        }
    }

    fn reload(&mut self) {
        if Instant::now() < self.next_reload {
            return;
        }
        self.next_reload = Instant::now() + RELOAD_INTERVAL;
        match Config::load() {
            Ok(config) => {
                self.config_diagnostic = None;
                if config != self.config {
                    self.config = config;
                    self.dirty_all();
                }
            }
            Err(error) => diagnose_once(&mut self.config_diagnostic, error),
        }
        match crate::timer::snapshot_timers(self.config.timer, self.shell_started_ns) {
            Ok(highlights) => {
                self.timer_diagnostic = None;
                if highlights != self.timer_highlights {
                    self.timer_highlights = highlights;
                    self.dirty_all();
                }
            }
            Err(error) => diagnose_once(&mut self.timer_diagnostic, error),
        }
        match crate::timer::snapshot_triggers() {
            Ok(highlights) => {
                self.trigger_diagnostic = None;
                if highlights != self.trigger_highlights {
                    self.trigger_highlights = highlights;
                    self.dirty_all();
                }
            }
            Err(error) => diagnose_once(&mut self.trigger_diagnostic, error),
        }
    }

    fn update_visual_key(&mut self) {
        let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
        let key = Some(
            self.active_highlights()
                .into_iter()
                .filter(|highlight| now < highlight.expires_ns)
                .map(|highlight| {
                    let timer = highlight.style.unwrap_or(self.config.timer);
                    let phase = match timer.animation {
                        Animation::Blink => (now - highlight.started_ns) / BLINK_INTERVAL_NS,
                        _ if timer.fade && now - highlight.started_ns < FADE_NS => 0,
                        _ if timer.fade && highlight.expires_ns - now <= FADE_NS => 2,
                        _ => 1,
                    };
                    (highlight.started_ns, format!("{}:{phase}", highlight.id))
                })
                .collect(),
        );
        if key != self.visual_key {
            self.visual_key = key;
            self.dirty_all();
        }
    }

    fn timeout(&self) -> Duration {
        let now = Instant::now();
        let reload = self.next_reload.saturating_duration_since(now);
        self.outputs
            .iter()
            .filter_map(|output| output.state.retry_at)
            .map(|retry| retry_timeout(retry, now))
            .fold(
                reload.min(self.visual_timeout().unwrap_or(reload)),
                Duration::min,
            )
    }

    fn visual_timeout(&self) -> Option<Duration> {
        let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
        self.active_highlights()
            .into_iter()
            .filter_map(|highlight| {
                let timer = highlight.style.unwrap_or(self.config.timer);
                let elapsed = now - highlight.started_ns;
                let remaining = highlight.expires_ns - now;
                let animation_delay = match timer.animation {
                    Animation::Blink => {
                        Some(BLINK_INTERVAL_NS - elapsed.rem_euclid(BLINK_INTERVAL_NS))
                    }
                    _ if timer.fade && elapsed < FADE_NS => Some(FADE_NS - elapsed),
                    _ if timer.fade && remaining > FADE_NS => Some(remaining - FADE_NS),
                    _ => None,
                };
                [
                    highlight.expires_ns - now,
                    animation_delay.unwrap_or(i128::MAX),
                ]
                .into_iter()
                .filter(|delay| *delay > 0)
                .min()
                .map(|delay| Duration::from_nanos(u64::try_from(delay).unwrap_or(u64::MAX)))
            })
            .min()
    }

    fn remove_output_for_layer(&mut self, layer: &LayerSurface) {
        if let Some(index) = self.output_index(layer) {
            self.outputs.remove(index);
        }
    }
}

fn retry_timeout(retry_at: Instant, now: Instant) -> Duration {
    retry_at.saturating_duration_since(now)
}

fn needs_frame_callback(animation: Animation, fade: bool, frame: TimerFrame) -> bool {
    let elapsed = frame.now_ns - frame.started_ns;
    let remaining = frame.deadline_ns - frame.now_ns;
    if remaining <= 0 {
        return false;
    }
    matches!(
        animation,
        Animation::Expand | Animation::FlowUp | Animation::FlowDown
    ) && elapsed < frame.deadline_ns - frame.started_ns
        || fade
            && (matches!(animation, Animation::Blink)
                || elapsed < FADE_NS
                || 0 < remaining && remaining <= FADE_NS)
}

fn diagnose_once(previous: &mut Option<String>, error: String) {
    if previous.as_deref() != Some(&error) {
        eprintln!("temporalshell: reload retained last valid state: {error}");
        *previous = Some(error);
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
                output.state.dirty = true;
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

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        for output in &mut self.outputs {
            if output.layer.wl_surface() == surface {
                output.state.frame_pending = false;
                output.state.dirty = true;
            }
        }
    }

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
        qh: &QueueHandle<Self>,
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
            self.outputs[index].state.dirty = true;
            if let Err(error) = self.redraw(qh, index) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_timeout_is_bounded_without_events() {
        let now = Instant::now();
        assert_eq!(retry_timeout(now, now), Duration::ZERO);
        assert_eq!(retry_timeout(now + BUFFER_RETRY, now), BUFFER_RETRY);
    }

    #[test]
    fn io_errors_are_classified_for_retry_and_writable_polling() {
        assert_eq!(io_disposition(ErrorKind::Interrupted), IoDisposition::Retry);
        assert_eq!(
            io_disposition(ErrorKind::WouldBlock),
            IoDisposition::WaitWritable
        );
        assert_eq!(io_disposition(ErrorKind::NotFound), IoDisposition::Fatal);
    }

    #[test]
    fn detach_clears_pending_frame_state() {
        let key = RenderKey {
            width: 1,
            height: 1,
            scale: 1,
        };
        let mut state = RenderState::new();
        state.frame_pending = true;
        state.retry_at = Some(Instant::now());
        state.reset_after_detach(key);
        assert_eq!(state.render_key, Some(key));
        assert!(!state.dirty);
        assert!(!state.frame_pending);
        assert!(state.retry_at.is_none());
    }

    #[test]
    fn key_changes_reset_the_pool() {
        let key = RenderKey {
            width: 1,
            height: 1,
            scale: 1,
        };
        assert!(!needs_pool_reset(None, key));
        assert!(!needs_pool_reset(Some(key), key));
        assert!(needs_pool_reset(Some(RenderKey { width: 2, ..key }), key));
    }

    #[test]
    fn idle_static_and_non_fading_blink_do_not_request_frames() {
        let frame = TimerFrame {
            started_ns: 0,
            deadline_ns: 10_000_000_000,
            now_ns: 5_000_000_000,
        };
        assert!(!needs_frame_callback(Animation::Static, false, frame));
        assert!(!needs_frame_callback(Animation::Blink, false, frame));
        assert!(needs_frame_callback(Animation::Expand, false, frame));
        assert!(needs_frame_callback(
            Animation::Static,
            true,
            TimerFrame { now_ns: 1, ..frame }
        ));
        assert!(needs_frame_callback(
            Animation::Static,
            true,
            TimerFrame {
                now_ns: frame.deadline_ns - 1,
                ..frame
            }
        ));
        assert!(!needs_frame_callback(
            Animation::Blink,
            true,
            TimerFrame {
                now_ns: frame.deadline_ns,
                ..frame
            }
        ));
    }
}
