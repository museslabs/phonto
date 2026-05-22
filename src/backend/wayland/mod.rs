mod battery_observer;
mod decoder;
mod displays;
mod gl_renderer;

use std::{collections::HashMap, path::Path, sync::mpsc, time::Instant};

use anyhow::Context;
use gstreamer as gst;
use gstreamer_gl as gst_gl;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop,
    protocol::{wl_callback, wl_compositor, wl_output, wl_registry, wl_surface},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use self::gl_renderer::{GlRenderer, OutputRender};
use super::{Backend, PauseMode, RunOptions};
use crate::displays::DisplayInfo;
use crate::plan::Playback;
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum LayerMode {
    #[default]
    Background,
    Bottom,
    Top,
    Overlay,
}

impl LayerMode {
    fn to_wlr(self) -> zwlr_layer_shell_v1::Layer {
        match self {
            LayerMode::Background => zwlr_layer_shell_v1::Layer::Background,
            LayerMode::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
            LayerMode::Top => zwlr_layer_shell_v1::Layer::Top,
            LayerMode::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
        }
    }
}

pub struct WaylandBackend {
    state: State,
    eq: EventQueue<State>,
    qh: QueueHandle<State>,
    shader: Option<String>,
}

impl WaylandBackend {
    pub fn new(layer: LayerMode, shader: Option<String>) -> anyhow::Result<Self> {
        let conn = Connection::connect_to_env().context("connect to Wayland display")?;
        let eq = conn.new_event_queue();
        let qh = eq.handle();

        let state = State::new(conn, layer);
        state.conn.display().get_registry(&qh, ());
        // Defer roundtrips to run(), so the surface_policy is set before any
        // wl_output Done events are dispatched.
        Ok(Self {
            state,
            eq,
            qh,
            shader,
        })
    }
}

// How often to re-read sysfs to update paused state.
const BATTERY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

impl Backend for WaylandBackend {
    fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
        displays::list_displays()
    }

    fn run(mut self, playback: Playback, options: RunOptions) -> anyhow::Result<()> {
        // Collect the per-assignment paths and set the surface policy before
        // any wl_output Done events fire (Done triggers try_create_layer_surface,
        // which consults the policy).
        let paths: Vec<String> = match &playback {
            Playback::Mirror(path) => vec![path.clone()],
            Playback::PerDisplay(assignments) => {
                assignments.iter().map(|a| a.path.clone()).collect()
            }
        };
        self.state.surface_policy = match &playback {
            Playback::Mirror(_) => SurfacePolicy::All,
            Playback::PerDisplay(assignments) => {
                let map: HashMap<String, usize> = assignments
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (a.native_id.clone(), i))
                    .collect();
                SurfacePolicy::PerDisplay(map)
            }
        };

        // Roundtrips after policy is set:
        //   1. registry Globals → bind compositor, layer_shell, wl_outputs.
        //   2. wl_output events (Name, Mode, Done) → create per-output layer surfaces.
        //   3. layer_surface Configure events.
        self.eq
            .roundtrip(&mut self.state)
            .context("initial Wayland roundtrip")?;
        self.eq
            .roundtrip(&mut self.state)
            .context("wl_output info roundtrip")?;
        self.eq
            .roundtrip(&mut self.state)
            .context("layer surface configure roundtrip")?;

        // Bootstrap the renderer from the first renderable matched output.
        loop {
            if self.state.first_renderable_output().is_some() {
                break;
            }
            self.eq
                .blocking_dispatch(&mut self.state)
                .context("waiting for an output to configure")?;
        }

        let bootstrap_name = self
            .state
            .first_renderable_output()
            .expect("loop guarantees one");
        let bootstrap = self
            .state
            .outputs
            .get(&bootstrap_name)
            .expect("just checked");
        let (renderer, first_render) = GlRenderer::new(
            &self.state.conn,
            bootstrap
                .wl_surface
                .as_ref()
                .expect("renderable has surface"),
            bootstrap.width.max(1),
            bootstrap.height.max(1),
            self.shader.as_deref(),
        )?;

        self.state.outputs.get_mut(&bootstrap_name).unwrap().render = Some(first_render);

        // Attach every other already-renderable output.
        let to_attach: Vec<u32> = self
            .state
            .outputs
            .iter()
            .filter(|(name, o)| **name != bootstrap_name && o.is_renderable())
            .map(|(name, _)| *name)
            .collect();
        for name in to_attach {
            let o = self.state.outputs.get_mut(&name).unwrap();
            let wl_surface = o.wl_surface.as_ref().expect("renderable has surface");
            let render = renderer.attach_output(wl_surface, o.width.max(1), o.height.max(1))?;
            o.render = Some(render);
        }

        // Spawn one decoder per assignment. Each gets its own gst_gl::GLContext
        // wrapping the same EGL context as the renderer.
        let decoders: Vec<DecoderHandle> = paths
            .into_iter()
            .map(|path| spawn_decoder(path, &renderer))
            .collect::<anyhow::Result<_>>()?;

        let mut paused = battery_observer::should_pause(&options.pause);
        let mut last_battery_check = Instant::now();
        log_pause_state(paused, &options.pause);

        loop {
            if last_battery_check.elapsed() >= BATTERY_POLL_INTERVAL {
                let new_paused = battery_observer::should_pause(&options.pause);
                if new_paused != paused {
                    paused = new_paused;
                    log_pause_state(paused, &options.pause);
                }
                last_battery_check = Instant::now();
            }

            if paused {
                self.eq
                    .blocking_dispatch(&mut self.state)
                    .context("dispatch while paused")?;
                continue;
            }

            // Attach renderers for outputs that became renderable since last tick.
            let to_attach: Vec<u32> = self
                .state
                .outputs
                .iter()
                .filter(|(_, o)| o.is_renderable() && o.render.is_none())
                .map(|(name, _)| *name)
                .collect();
            for name in to_attach {
                let o = self.state.outputs.get_mut(&name).unwrap();
                let wl_surface = o.wl_surface.as_ref().expect("renderable has surface");
                match renderer.attach_output(wl_surface, o.width.max(1), o.height.max(1)) {
                    Ok(render) => {
                        log::info!("attached new output: {}", o.name);
                        o.render = Some(render);
                    }
                    Err(e) => log::warn!("failed to attach output {}: {e:#}", o.name),
                }
            }

            // Wait until every rendering output's frame callback has fired.
            while self
                .state
                .outputs
                .values()
                .any(|o| o.render.is_some() && o.frame_callback_pending)
            {
                self.eq
                    .blocking_dispatch(&mut self.state)
                    .context("waiting for frame callback")?;
            }

            // Group renderable outputs by decoder (assignment_index). Mirror
            // collapses to one group; PerDisplay has one output per group.
            let mut groups: HashMap<usize, Vec<u32>> = HashMap::new();
            for (name, o) in self.state.outputs.iter() {
                if o.render.is_none() {
                    continue;
                }
                let Some(idx) = o.assignment_index else {
                    continue;
                };
                groups.entry(idx).or_default().push(*name);
            }

            for (idx, output_names) in groups {
                let decoder = &decoders[idx];
                let sample = decoder.rx.recv().context("receive decoded sample")?;
                let frame = decoder::sample_to_frame(sample, &decoder.gl_context)?;
                let video_dims = (frame.width, frame.height);

                for name in output_names {
                    let o = self.state.outputs.get_mut(&name).unwrap();
                    let target_dims = (o.width.max(1), o.height.max(1));

                    // Frame callback must precede the commit that swap_buffers
                    // performs, so the compositor schedules it for that commit.
                    let surface = o.wl_surface.as_ref().expect("filtered");
                    surface.frame(&self.qh, name);
                    o.frame_callback_pending = true;

                    let render = o.render.as_mut().expect("filtered");
                    if render.dims() != target_dims {
                        render.set_surface_size(&renderer, target_dims.0, target_dims.1);
                    }
                    renderer.render(render, &frame, options.scale, video_dims)?;
                }
            }

            self.eq
                .dispatch_pending(&mut self.state)
                .context("dispatch pending Wayland events")?;
            self.state
                .conn
                .flush()
                .context("flush Wayland connection")?;
        }
    }
}

struct DecoderHandle {
    rx: mpsc::Receiver<gst::Sample>,
    gl_context: gst_gl::GLContext,
    _handle: std::thread::JoinHandle<()>,
}

fn spawn_decoder(path: String, renderer: &GlRenderer) -> anyhow::Result<DecoderHandle> {
    let (gl_display, gl_context) =
        decoder::wrap_gl(renderer.egl_display(), renderer.egl_context())?;
    let (tx, rx) = mpsc::sync_channel(1);
    let decoder_gl_context = gl_context.clone();
    let thread_name = format!(
        "decoder:{}",
        std::path::Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone())
    );
    let handle = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            if let Err(e) = decoder::run(Path::new(&path), gl_display, decoder_gl_context, tx) {
                log::error!("decoder error for {path}: {e:#}");
            }
        })?;
    Ok(DecoderHandle {
        rx,
        gl_context,
        _handle: handle,
    })
}

fn log_pause_state(paused: bool, mode: &PauseMode) {
    let on_batt = matches!(mode, PauseMode::Never).then_some(false);
    if paused {
        log::info!("pausing wallpaper (battery pause active)");
    } else if !matches!(mode, PauseMode::Never) {
        log::info!(
            "playing wallpaper (battery pause inactive, on_battery={:?})",
            on_batt
        );
    }
}

struct OutputState {
    wl_output: wl_output::WlOutput,
    name: String,
    output_done: bool,
    wl_surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    width: u32,
    height: u32,
    configured: bool,
    frame_callback_pending: bool,
    render: Option<OutputRender>,
    /// Index into the backend's `decoders` Vec. Set by `try_create_layer_surface`
    /// when the output matches the active SurfacePolicy.
    assignment_index: Option<usize>,
}

impl OutputState {
    fn new(wl_output: wl_output::WlOutput) -> Self {
        Self {
            wl_output,
            name: String::new(),
            output_done: false,
            wl_surface: None,
            layer_surface: None,
            width: 1,
            height: 1,
            configured: false,
            frame_callback_pending: false,
            render: None,
            assignment_index: None,
        }
    }

    fn is_renderable(&self) -> bool {
        self.configured && self.wl_surface.is_some() && self.assignment_index.is_some()
    }
}

enum SurfacePolicy {
    /// Every output gets a surface fed by decoder 0 (Mirror mode).
    All,
    /// Only outputs whose name matches a key in this map get a surface;
    /// the value is the index into the backend's `decoders` Vec.
    PerDisplay(HashMap<String, usize>),
}

struct State {
    conn: Connection,
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    layer_mode: LayerMode,
    outputs: HashMap<u32, OutputState>,
    surface_policy: SurfacePolicy,
}

impl State {
    fn new(conn: Connection, layer_mode: LayerMode) -> Self {
        Self {
            conn,
            compositor: None,
            layer_shell: None,
            layer_mode,
            outputs: HashMap::new(),
            // Replaced in Backend::run before any roundtrips happen.
            surface_policy: SurfacePolicy::All,
        }
    }

    fn first_renderable_output(&self) -> Option<u32> {
        self.outputs
            .iter()
            .find(|(_, o)| o.is_renderable())
            .map(|(name, _)| *name)
    }

    fn try_create_layer_surface(&mut self, registry_name: u32, qh: &QueueHandle<Self>) {
        let Some(compositor) = self.compositor.as_ref() else {
            return;
        };
        let Some(layer_shell) = self.layer_shell.as_ref() else {
            return;
        };
        let layer = self.layer_mode.to_wlr();

        // Decide whether this output gets a surface (and which decoder feeds it)
        // based on the active SurfacePolicy.
        let assignment_index = {
            let Some(output) = self.outputs.get(&registry_name) else {
                return;
            };
            if !output.output_done || output.wl_surface.is_some() {
                return;
            }
            match &self.surface_policy {
                SurfacePolicy::All => 0,
                SurfacePolicy::PerDisplay(map) => match map.get(&output.name) {
                    Some(&idx) => idx,
                    None => {
                        log::info!(
                            "wl_output {:?} not in [[display]] assignments; leaving alone",
                            output.name
                        );
                        return;
                    }
                },
            }
        };

        let Some(output) = self.outputs.get_mut(&registry_name) else {
            return;
        };

        let wl_surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &wl_surface,
            Some(&output.wl_output),
            layer,
            "phonto".to_string(),
            qh,
            registry_name,
        );
        layer_surface.set_size(0, 0);
        layer_surface.set_anchor(zwlr_layer_surface_v1::Anchor::all());
        layer_surface.set_exclusive_zone(-1);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);

        wl_surface.commit();

        let log_name = if output.name.is_empty() {
            format!("wl_output#{registry_name}")
        } else {
            output.name.clone()
        };
        log::info!("created layer surface for {log_name}");

        output.wl_surface = Some(wl_surface);
        output.layer_surface = Some(layer_surface);
        output.assignment_index = Some(assignment_index);
    }

    /// After compositor/layer_shell appear, try to create layer surfaces for
    /// any wl_outputs that already had their Done event delivered.
    fn maybe_create_pending_layer_surfaces(&mut self, qh: &QueueHandle<Self>) {
        let pending: Vec<u32> = self
            .outputs
            .iter()
            .filter(|(_, o)| o.output_done && o.wl_surface.is_none())
            .map(|(name, _)| *name)
            .collect();
        for name in pending {
            self.try_create_layer_surface(name, qh);
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                    state.maybe_create_pending_layer_surfaces(qh);
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                    state.maybe_create_pending_layer_surfaces(qh);
                }
                "wl_output" => {
                    log::info!("wl_output advertised (registry name {name})");
                    let output: wl_output::WlOutput = registry.bind(name, version.min(4), qh, name);
                    state.outputs.insert(name, OutputState::new(output));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(removed) = state.outputs.remove(&name) {
                    let label = if removed.name.is_empty() {
                        format!("wl_output#{name}")
                    } else {
                        removed.name.clone()
                    };
                    log::info!("output removed: {label}");
                    // OutputState's Drop tears down layer_surface, wl_surface,
                    // wl_output proxies and the OutputRender (EGL surface).
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        registry_name: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let registry_name = *registry_name;
        match event {
            wl_output::Event::Name { name } => {
                if let Some(output) = state.outputs.get_mut(&registry_name) {
                    output.name = name;
                }
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                if let Some(output) = state.outputs.get_mut(&registry_name) {
                    let is_current = matches!(
                        flags,
                        WEnum::Value(m) if m.contains(wl_output::Mode::Current)
                    );
                    if is_current {
                        output.width = width.max(0) as u32;
                        output.height = height.max(0) as u32;
                    }
                }
            }
            wl_output::Event::Done => {
                if let Some(output) = state.outputs.get_mut(&registry_name) {
                    output.output_done = true;
                }
                state.try_create_layer_surface(registry_name, qh);
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, u32> for State {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        registry_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                if let Some(output) = state.outputs.get_mut(registry_name) {
                    if width > 0 && height > 0 {
                        output.width = width;
                        output.height = height;
                    }
                    output.configured = true;
                    log::info!(
                        "layer surface configured for {}: {}x{}",
                        if output.name.is_empty() {
                            format!("wl_output#{registry_name}")
                        } else {
                            output.name.clone()
                        },
                        output.width,
                        output.height,
                    );
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                if let Some(removed) = state.outputs.remove(registry_name) {
                    let label = if removed.name.is_empty() {
                        format!("wl_output#{registry_name}")
                    } else {
                        removed.name.clone()
                    };
                    log::info!("layer surface closed for {label}");
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        registry_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event
            && let Some(output) = state.outputs.get_mut(registry_name)
        {
            output.frame_callback_pending = false;
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
