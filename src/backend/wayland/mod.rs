mod battery_observer;
mod decoder;
mod gl_renderer;

use std::{collections::HashMap, sync::mpsc, time::Instant};

use anyhow::{Context, bail};
use gstreamer as gst;
use gstreamer_gl as gst_gl;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop,
    protocol::{wl_callback, wl_compositor, wl_output, wl_registry, wl_surface},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use self::gl_renderer::GlRenderer;
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

pub struct WaylandBackend {
    state: State,
    eq: EventQueue<State>,
    qh: QueueHandle<State>,
    layer: LayerMode,
    shader: Option<String>,
}

impl WaylandBackend {
    pub fn new(layer: LayerMode, shader: Option<String>) -> anyhow::Result<Self> {
        let conn = Connection::connect_to_env().context("connect to Wayland display")?;
        let mut eq = conn.new_event_queue();
        let qh = eq.handle();
        let mut state = State::new(conn);
        state.conn.display().get_registry(&qh, ());
        eq.roundtrip(&mut state).context("registry roundtrip")?;
        eq.roundtrip(&mut state).context("output info roundtrip")?;

        Ok(Self {
            state,
            eq,
            qh,
            layer,
            shader,
        })
    }
}

const BATTERY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

impl Backend for WaylandBackend {
    fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
        let conn = Connection::connect_to_env().context("connect to Wayland display")?;
        let mut eq = conn.new_event_queue();
        let qh = eq.handle();
        let mut state = State::new(conn);
        state.conn.display().get_registry(&qh, ());
        eq.roundtrip(&mut state).context("registry roundtrip")?;
        eq.roundtrip(&mut state).context("output info roundtrip")?;

        let mut out: Vec<DisplayInfo> = state
            .outputs
            .into_values()
            .map(|o| DisplayInfo {
                id: o.name,
                description: o.description,
                width: o.width,
                height: o.height,
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    fn run(mut self, playback: Playback, options: RunOptions) -> anyhow::Result<()> {
        // Each video path gets one decoder; `decoder_for` maps an output name
        // to the index of the decoder that feeds it.
        let (paths, decoder_for): (Vec<String>, HashMap<String, usize>) = match playback {
            Playback::Mirror(path) => (vec![path], HashMap::new()),
            Playback::PerDisplay(assignments) => {
                let paths = assignments.iter().map(|a| a.path.clone()).collect();
                let map = assignments
                    .into_iter()
                    .enumerate()
                    .map(|(i, a)| (a.native_id, i))
                    .collect();
                (paths, map)
            }
        };
        let mirror = decoder_for.is_empty();

        let compositor = self
            .state
            .compositor
            .clone()
            .context("wl_compositor missing")?;
        let layer_shell = self
            .state
            .layer_shell
            .clone()
            .context("zwlr_layer_shell_v1 missing")?;

        // Pick the outputs to render on, in stable order by name.
        let mut chosen: Vec<u32> = self
            .state
            .outputs
            .iter()
            .filter(|(_, o)| mirror || decoder_for.contains_key(&o.name))
            .map(|(name, _)| *name)
            .collect();
        chosen.sort_by(|a, b| self.state.outputs[a].name.cmp(&self.state.outputs[b].name));
        if chosen.is_empty() {
            bail!("no matching wl_outputs to render on");
        }

        // Create one layer surface per chosen output.
        for &name in &chosen {
            let o = self.state.outputs.get_mut(&name).unwrap();
            o.create_layer_surface(name, &compositor, &layer_shell, self.layer, &self.qh);
            o.decoder_idx = if mirror {
                Some(0)
            } else {
                decoder_for.get(&o.name).copied()
            };
        }

        // Block until every chosen output has configured, dropping outputs
        // that disappear while dispatching configure events.
        while self.state.any_unconfigured(&mut chosen) {
            self.eq
                .blocking_dispatch(&mut self.state)
                .context("waiting for layer surface configure")?;
        }
        if chosen.is_empty() {
            bail!("all matching wl_outputs disappeared before configuring");
        }

        // Build the renderer from the first chosen output, then attach the rest.
        let mut chosen_iter = chosen.iter();
        let first_name = *chosen_iter.next().unwrap();
        let first = self.state.outputs.get_mut(&first_name).unwrap();
        let mut renderer = GlRenderer::new(
            &self.state.conn,
            first.wl_surface.as_ref().unwrap(),
            first.width.max(1),
            first.height.max(1),
            self.shader.as_deref(),
        )?;
        first.surface_idx = Some(0);
        for &name in chosen_iter {
            let o = self.state.outputs.get_mut(&name).unwrap();
            let idx = renderer.add_surface(
                o.wl_surface.as_ref().unwrap(),
                o.width.max(1),
                o.height.max(1),
            )?;
            o.surface_idx = Some(idx);
        }

        // One decoder per video path. Each wraps the same EGL context.
        let mut decoders: Vec<DecoderHandle> = Vec::with_capacity(paths.len());
        for path in paths {
            let (gl_display, gl_context) =
                decoder::wrap_gl(renderer.egl_display(), renderer.egl_context())?;
            let (tx, rx) = mpsc::sync_channel(1);
            let decoder_gl_context = gl_context.clone();
            let short = if crate::config::is_url(&path) {
                path.split('/').next_back().unwrap_or(&path).to_string()
            } else {
                std::path::Path::new(&path)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone())
            };
            let thread_name = format!("decoder:{short}");
            std::thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    if let Err(e) = decoder::run(&path, gl_display, decoder_gl_context, tx) {
                        log::error!("decoder error for {path}: {e:#}");
                    }
                })?;
            decoders.push(DecoderHandle { rx, gl_context });
        }

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

            // Attach layer surfaces for outputs that appeared since startup.
            let mut new_outputs: Vec<u32> = self
                .state
                .outputs
                .iter()
                .filter(|(_, o)| {
                    o.output_done
                        && o.wl_surface.is_none()
                        && (mirror || decoder_for.contains_key(&o.name))
                })
                .map(|(name, _)| *name)
                .collect();
            for &name in &new_outputs {
                let o = self.state.outputs.get_mut(&name).unwrap();
                o.create_layer_surface(name, &compositor, &layer_shell, self.layer, &self.qh);
                o.decoder_idx = if mirror {
                    Some(0)
                } else {
                    decoder_for.get(&o.name).copied()
                };
            }
            while self.state.any_unconfigured(&mut new_outputs) {
                self.eq
                    .blocking_dispatch(&mut self.state)
                    .context("waiting for hotplug configure")?;
            }
            self.state.retain_live(&mut new_outputs);
            for &name in &new_outputs {
                let o = self.state.outputs.get_mut(&name).unwrap();
                let idx = renderer.add_surface(
                    o.wl_surface.as_ref().unwrap(),
                    o.width.max(1),
                    o.height.max(1),
                )?;
                o.surface_idx = Some(idx);
                chosen.push(name);
                log::info!("hotplug: attached output {}", o.name);
            }
            self.state.remove_detached_surfaces(&mut renderer);

            self.state.wait_for_frame_callbacks(&mut self.eq)?;
            self.state.remove_detached_surfaces(&mut renderer);

            // Drop entries for outputs that were removed during frame callback dispatch.
            self.state.retain_live(&mut chosen);

            // One fresh frame from each decoder that feeds an active output.
            let mut groups: HashMap<usize, Vec<u32>> = HashMap::new();
            for &name in &chosen {
                let Some(o) = self.state.outputs.get(&name) else {
                    continue;
                };
                let Some(decoder_idx) = o.decoder_idx else {
                    continue;
                };
                groups.entry(decoder_idx).or_default().push(name);
            }

            let mut frames: HashMap<usize, decoder::Frame> = HashMap::new();
            for &decoder_idx in groups.keys() {
                let decoder = &decoders[decoder_idx];
                let sample = decoder.rx.recv().context("receive decoded sample")?;
                let frame = decoder::sample_to_frame(sample, &decoder.gl_context)?;
                frames.insert(decoder_idx, frame);
            }

            // Render each chosen output with its decoder's frame. The frame
            // callback is requested before swap_buffers so the compositor
            // schedules it with that commit.
            for (decoder_idx, output_names) in groups {
                let frame = frames.get(&decoder_idx).expect("frame fetched for group");
                for name in output_names {
                    let o = self.state.outputs.get_mut(&name).unwrap();
                    let surface_idx = o.surface_idx.unwrap();
                    let target_dims = (o.width.max(1), o.height.max(1));
                    if renderer.surface_dims(surface_idx) != target_dims {
                        renderer.set_surface_size(surface_idx, target_dims.0, target_dims.1);
                    }
                    o.wl_surface.as_ref().unwrap().frame(&self.qh, name);
                    o.frame_callback_pending = true;
                    renderer.render(surface_idx, frame, options.scale)?;
                }
            }

            self.eq
                .dispatch_pending(&mut self.state)
                .context("dispatch pending Wayland events")?;
            self.state.remove_detached_surfaces(&mut renderer);
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

struct State {
    conn: Connection,
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    outputs: HashMap<u32, OutputState>,
    detached_surface_idxs: Vec<usize>,
}

struct OutputState {
    wl_output: wl_output::WlOutput,
    name: String,
    description: String,
    width: u32,
    height: u32,
    output_done: bool,
    wl_surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    configured: bool,
    frame_callback_pending: bool,
    surface_idx: Option<usize>,
    decoder_idx: Option<usize>,
}

impl OutputState {
    fn new(wl_output: wl_output::WlOutput) -> Self {
        Self {
            wl_output,
            name: String::new(),
            description: String::new(),
            width: 1,
            height: 1,
            output_done: false,
            wl_surface: None,
            layer_surface: None,
            configured: false,
            frame_callback_pending: false,
            surface_idx: None,
            decoder_idx: None,
        }
    }

    fn create_layer_surface(
        &mut self,
        registry_name: u32,
        compositor: &wl_compositor::WlCompositor,
        layer_shell: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        layer: LayerMode,
        qh: &QueueHandle<State>,
    ) {
        let wlr_layer = match layer {
            LayerMode::Background => zwlr_layer_shell_v1::Layer::Background,
            LayerMode::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
            LayerMode::Top => zwlr_layer_shell_v1::Layer::Top,
            LayerMode::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
        };

        let wl_surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &wl_surface,
            Some(&self.wl_output),
            wlr_layer,
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

        self.wl_surface = Some(wl_surface);
        self.layer_surface = Some(layer_surface);
    }
}

impl State {
    fn new(conn: Connection) -> Self {
        Self {
            conn,
            compositor: None,
            layer_shell: None,
            outputs: HashMap::new(),
            detached_surface_idxs: Vec::new(),
        }
    }

    fn remove_output(&mut self, name: u32, reason: &str) {
        if let Some(removed) = self.outputs.remove(&name) {
            if let Some(idx) = removed.surface_idx {
                self.detached_surface_idxs.push(idx);
            }
            let label = if removed.name.is_empty() {
                format!("wl_output#{name}")
            } else {
                removed.name
            };
            log::info!("{reason}: {label}");
        }
    }

    fn remove_detached_surfaces(&mut self, renderer: &mut GlRenderer) {
        for idx in self.detached_surface_idxs.drain(..) {
            renderer.remove_surface(idx);
        }
    }

    fn retain_live(&self, names: &mut Vec<u32>) {
        names.retain(|n| self.outputs.contains_key(n));
    }

    fn any_unconfigured(&self, names: &mut Vec<u32>) -> bool {
        names.retain(|n| self.outputs.contains_key(n));
        names
            .iter()
            .any(|n| self.outputs.get(n).is_some_and(|o| !o.configured))
    }

    fn wait_for_frame_callbacks(&mut self, eq: &mut EventQueue<Self>) -> anyhow::Result<()> {
        while self.outputs.values().any(|o| o.frame_callback_pending) {
            eq.blocking_dispatch(self)
                .context("waiting for frame callback")?;
        }
        Ok(())
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
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_output" => {
                    let output: wl_output::WlOutput = registry.bind(name, version.min(4), qh, name);
                    state.outputs.insert(name, OutputState::new(output));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                state.remove_output(name, "output removed");
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
        _: &QueueHandle<Self>,
    ) {
        let Some(o) = state.outputs.get_mut(registry_name) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => {
                o.name = name;
            }
            wl_output::Event::Description { description } => {
                o.description = description;
            }
            wl_output::Event::Geometry { make, model, .. } if o.description.is_empty() => {
                o.description = format!("{make} {model}").trim().to_string();
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                let is_current = matches!(
                    flags,
                    WEnum::Value(m) if m.contains(wl_output::Mode::Current)
                );
                if is_current {
                    o.width = width.max(0) as u32;
                    o.height = height.max(0) as u32;
                }
            }
            wl_output::Event::Done => {
                o.output_done = true;
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
                if let Some(o) = state.outputs.get_mut(registry_name) {
                    if width > 0 && height > 0 {
                        o.width = width;
                        o.height = height;
                    }
                    o.configured = true;
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.remove_output(*registry_name, "layer surface closed");
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
            && let Some(o) = state.outputs.get_mut(registry_name)
        {
            o.frame_callback_pending = false;
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
