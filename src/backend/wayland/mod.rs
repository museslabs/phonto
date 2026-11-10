mod battery_observer;
mod decoder;
mod gl_renderer;
mod procedural;

use std::{
    collections::HashMap,
    path::Path,
    sync::mpsc,
    time::{Instant, SystemTime},
};

use anyhow::{Context, bail};
use gstreamer as gst;
use gstreamer::prelude::ElementExt;
use gstreamer_gl as gst_gl;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop,
    protocol::{
        wl_callback, wl_compositor, wl_output, wl_pointer, wl_registry, wl_seat, wl_surface,
    },
};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use self::gl_renderer::{GlRenderer, TextureRef};
use super::{Backend, DumpRequest, PauseMode, RunOptions};
use crate::displays::DisplayInfo;
use crate::plan::Playback;
use crate::procedural::{Clock, FrameInputs, FrameLimiter, Pack, Pointer, local_date};
use crate::scale::ScaleMode;
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
    shader: Option<String>,
}

impl WaylandBackend {
    pub fn new(shader: Option<String>) -> anyhow::Result<Self> {
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
            shader,
        })
    }
}

const BATTERY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// The image size of `dump` for a procedural wallpaper without `--size`.
const DEFAULT_DUMP_SIZE: (u32, u32) = (1920, 1080);

/// The frame rate of `dump` for an uncapped procedural wallpaper. A capped
/// wallpaper steps at its cap, so its buffers change as they do on screen.
const UNCAPPED_DUMP_FPS: f64 = 60.0;

impl WaylandBackend {
    fn dump_procedural(self, request: DumpRequest) -> anyhow::Result<()> {
        let pack = Pack::load(Path::new(&request.path))?;
        let compositor = self
            .state
            .compositor
            .as_ref()
            .context("wl_compositor missing")?;
        let wl_surface = compositor.create_surface(&self.qh, ());
        let renderer =
            GlRenderer::new(&self.state.conn, &wl_surface, 1, 1, self.shader.as_deref())?;
        let gl = renderer.gl();
        let program = procedural::Program::compile(gl, &pack)?;
        let (width, height) = request.size.unwrap_or(DEFAULT_DUMP_SIZE);

        let result = (|| {
            let mut target = program.create_target(gl, program.target_dims((width, height)))?;
            let fps = pack.fps.map_or(UNCAPPED_DUMP_FPS, f64::from);
            let frames = (request.at * fps).floor() as u64;
            let date = local_date(SystemTime::now());
            let mut texture = None;
            let mut previous = 0.0;
            for i in 0..=frames {
                let time = if i == frames {
                    request.at
                } else {
                    i as f64 / fps
                };
                let inputs = FrameInputs {
                    time: time as f32,
                    time_delta: (time - previous) as f32,
                    frame_rate: fps as f32,
                    date,
                    ..FrameInputs::default()
                };
                previous = time;
                texture = Some(program.render(gl, &mut target, &inputs));
            }
            let texture = texture.expect("rendered at least one frame");
            let rgba = renderer.render_texture_to_rgba(texture, width, height);
            target.destroy(gl);
            image::save_buffer(&request.out, &rgba?, width, height, image::ColorType::Rgba8)
                .with_context(|| format!("save frame to {}", request.out.display()))
        })();

        program.destroy(gl);
        result
    }
}

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

    fn dump(self, request: DumpRequest) -> anyhow::Result<()> {
        if crate::procedural::is_procedural(&request.path) {
            return self.dump_procedural(request);
        }
        if request.size.is_some() {
            log::warn!("--size only applies to procedural wallpapers. Videos keep their own size.");
        }
        let DumpRequest { path, at, out, .. } = request;

        let compositor = self
            .state
            .compositor
            .as_ref()
            .context("wl_compositor missing")?;

        let wl_surface = compositor.create_surface(&self.qh, ());
        let renderer =
            GlRenderer::new(&self.state.conn, &wl_surface, 1, 1, self.shader.as_deref())?;

        let (gl_display, gl_context) =
            decoder::wrap_gl(renderer.egl_display(), renderer.egl_context())?;

        let pipeline = decoder::build_pipeline(&path)?;
        decoder::set_pipeline_gl_context(&pipeline, &gl_display, &gl_context);

        let result = (|| {
            let sample = decoder::pull_sample_at(&pipeline, at)?;
            let frame = decoder::sample_to_frame(sample, &gl_context)?;
            let rgba = renderer.render_texture_to_rgba(
                TextureRef::from_frame(&frame)?,
                frame.width,
                frame.height,
            )?;

            image::save_buffer(
                &out,
                &rgba,
                frame.width,
                frame.height,
                image::ColorType::Rgba8,
            )
            .with_context(|| format!("save frame to {}", out.display()))
        })();

        // Tear the pipeline down no matter how the frame grab went, so GL and
        // GStreamer resources aren't left alive on the error path. Don't let a
        // teardown failure mask the original error if there was one.
        let teardown = pipeline.set_state(gst::State::Null);
        if result.is_ok() {
            teardown.context("reset pipeline to Null")?;
        }
        result
    }

    fn run(mut self, playback: Playback, options: RunOptions) -> anyhow::Result<()> {
        // Each path gets one source. `source_for` maps an output name to the
        // index of its source.
        let (paths, source_for): (Vec<String>, HashMap<String, usize>) = match playback {
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
        let mirror = source_for.is_empty();

        // Load the procedural packs before a surface exists. Then a broken
        // manifest or a missing file fails before the wallpaper shows.
        let specs: Vec<SourceSpec> = paths
            .into_iter()
            .map(SourceSpec::load)
            .collect::<anyhow::Result<_>>()?;
        if specs.iter().any(|s| matches!(s, SourceSpec::Procedural(_))) {
            // Only procedural wallpapers use the pointer. A session with only
            // videos does not bind it.
            self.state.enable_pointer(&self.qh);
        }

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
            .filter(|(_, o)| mirror || source_for.contains_key(&o.name))
            .map(|(name, _)| *name)
            .collect();
        chosen.sort_by(|a, b| self.state.outputs[a].name.cmp(&self.state.outputs[b].name));
        if chosen.is_empty() {
            bail!("no matching wl_outputs to render on");
        }

        // Create one layer surface per chosen output.
        for &name in &chosen {
            let o = self.state.outputs.get_mut(&name).unwrap();
            o.create_layer_surface(name, &compositor, &layer_shell, options.layer, &self.qh);
            o.source_idx = if mirror {
                Some(0)
            } else {
                source_for.get(&o.name).copied()
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

        let mut sources: Vec<Source> = Vec::with_capacity(specs.len());
        for spec in specs {
            sources.push(match spec {
                SourceSpec::Video(path) => Source::Video(spawn_decoder(path, &renderer)?),
                SourceSpec::Procedural(pack) => {
                    Source::Procedural(ProceduralSource::new(renderer.gl(), &pack)?)
                }
            });
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
                        && (mirror || source_for.contains_key(&o.name))
                })
                .map(|(name, _)| *name)
                .collect();
            for &name in &new_outputs {
                let o = self.state.outputs.get_mut(&name).unwrap();
                o.create_layer_surface(name, &compositor, &layer_shell, options.layer, &self.qh);
                o.source_idx = if mirror {
                    Some(0)
                } else {
                    source_for.get(&o.name).copied()
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
            self.state.release_detached(&mut renderer);

            self.state.wait_for_frame_callbacks(&mut self.eq)?;
            self.state.release_detached(&mut renderer);

            // Drop entries for outputs that were removed during frame callback dispatch.
            self.state.retain_live(&mut chosen);

            // Group the active outputs by their source.
            let mut groups: HashMap<usize, Vec<u32>> = HashMap::new();
            for &name in &chosen {
                let Some(o) = self.state.outputs.get(&name) else {
                    continue;
                };
                let Some(source_idx) = o.source_idx else {
                    continue;
                };
                groups.entry(source_idx).or_default().push(name);
            }

            if groups.is_empty() {
                // There is nothing to draw, because all outputs are gone. Wait
                // for the compositor, and do not spin.
                self.eq
                    .blocking_dispatch(&mut self.state)
                    .context("dispatch with no outputs")?;
                continue;
            }

            // Keep procedural sources at their frame-rate cap. Sleep until the
            // first source is due, then draw only the sources that are due.
            let earliest = groups
                .keys()
                .map(|&i| sources[i].next_frame())
                .min()
                .flatten();
            if let Some(wait) = earliest.and_then(|t| t.checked_duration_since(Instant::now())) {
                std::thread::sleep(wait);
            }
            let now = Instant::now();
            groups.retain(|&i, _| sources[i].next_frame().is_none_or(|t| t <= now));

            // Get one new frame from each video source of an active output.
            let mut frames: HashMap<usize, decoder::Frame> = HashMap::new();
            for &source_idx in groups.keys() {
                if let Source::Video(decoder) = &sources[source_idx] {
                    let sample = decoder.rx.recv().context("receive decoded sample")?;
                    let frame = decoder::sample_to_frame(sample, &decoder.gl_context)?;
                    frames.insert(source_idx, frame);
                }
            }

            // Draw each output from its source. The frame callback is
            // requested before swap_buffers, so the compositor schedules it
            // with that commit.
            for (source_idx, output_names) in groups {
                let source = &mut sources[source_idx];
                let tick = match source {
                    Source::Procedural(p) => {
                        Some((p.clock.tick(now), local_date(SystemTime::now())))
                    }
                    Source::Video(_) => None,
                };
                for name in output_names {
                    let o = self.state.outputs.get_mut(&name).unwrap();
                    let surface_idx = o.surface_idx.unwrap();
                    let surface_dims = (o.width.max(1), o.height.max(1));
                    if renderer.surface_dims(surface_idx) != surface_dims {
                        renderer.set_surface_size(surface_idx, surface_dims.0, surface_dims.1);
                    }
                    o.wl_surface.as_ref().unwrap().frame(&self.qh, name);
                    o.frame_callback_pending = true;

                    match source {
                        Source::Video(_) => {
                            let frame = frames.get(&source_idx).expect("frame fetched for group");
                            renderer.render(surface_idx, frame, options.scale)?;
                        }
                        Source::Procedural(p) => {
                            let (tick, date) = tick.expect("procedural sources tick");
                            renderer.make_current(surface_idx)?;
                            let gl = renderer.gl();
                            let dims = p.program.target_dims(surface_dims);
                            if o.target.as_ref().map(procedural::Target::dims) != Some(dims) {
                                if let Some(old) = o.target.take() {
                                    old.destroy(gl);
                                }
                                o.target = Some(p.program.create_target(gl, dims)?);
                            }
                            let (mouse, pointer) = o.pointer.uniforms(surface_dims, dims);
                            let inputs = FrameInputs {
                                time: tick.time,
                                time_delta: tick.delta,
                                frame_rate: tick.frame_rate,
                                date,
                                mouse,
                                pointer,
                            };
                            let target = o.target.as_mut().expect("target created above");
                            let texture = p.program.render(gl, target, &inputs);
                            // Procedural output always fills the surface.
                            // `--scale` is only for videos.
                            renderer.present(surface_idx, texture, ScaleMode::Stretch)?;
                        }
                    }
                }
                if let Source::Procedural(p) = source {
                    p.limiter.rendered(now);
                }
            }

            self.eq
                .dispatch_pending(&mut self.state)
                .context("dispatch pending Wayland events")?;
            self.state.release_detached(&mut renderer);
            self.state
                .conn
                .flush()
                .context("flush Wayland connection")?;
        }
    }
}

/// The kind of a path, known before GL exists.
enum SourceSpec {
    Video(String),
    Procedural(Box<Pack>),
}

impl SourceSpec {
    fn load(path: String) -> anyhow::Result<Self> {
        if !crate::procedural::is_procedural(&path) {
            return Ok(Self::Video(path));
        }
        let pack = Pack::load(Path::new(&path))?;
        log::info!(
            "procedural wallpaper `{}`: {} buffer pass(es), {} fps cap, render scale {}",
            pack.name,
            pack.buffers.len(),
            pack.fps.map_or_else(|| "no".to_string(), |f| f.to_string()),
            pack.render_scale
        );
        Ok(Self::Procedural(Box::new(pack)))
    }
}

/// The source of the frames of an output.
enum Source {
    Video(DecoderHandle),
    Procedural(ProceduralSource),
}

impl Source {
    /// The earliest time that this source draws again. `None` means now.
    fn next_frame(&self) -> Option<Instant> {
        match self {
            Source::Video(_) => None,
            Source::Procedural(p) => p.limiter.next_frame(),
        }
    }
}

struct ProceduralSource {
    program: procedural::Program,
    /// All outputs that show this source share it, so mirrored outputs stay
    /// in sync.
    clock: Clock,
    limiter: FrameLimiter,
}

impl ProceduralSource {
    fn new(gl: &glow::Context, pack: &Pack) -> anyhow::Result<Self> {
        Ok(Self {
            program: procedural::Program::compile(gl, pack)?,
            clock: Clock::new(pack.fps),
            limiter: FrameLimiter::new(pack.fps),
        })
    }
}

struct DecoderHandle {
    rx: mpsc::Receiver<gst::Sample>,
    gl_context: gst_gl::GLContext,
}

/// Starts a decoder thread for `path` that shares the EGL context of the
/// renderer.
fn spawn_decoder(path: String, renderer: &GlRenderer) -> anyhow::Result<DecoderHandle> {
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
    Ok(DecoderHandle { rx, gl_context })
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
    cursor_shape: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    outputs: HashMap<u32, OutputState>,
    seats: HashMap<u32, SeatState>,
    /// If true, bind the pointer of each seat that has one. It is false
    /// unless a procedural wallpaper plays.
    pointer_enabled: bool,
    /// The registry name of the output under the pointer.
    pointer_focus: Option<u32>,
    detached_surface_idxs: Vec<usize>,
    detached_targets: Vec<procedural::Target>,
}

struct SeatState {
    seat: wl_seat::WlSeat,
    has_pointer: bool,
    pointer: Option<wl_pointer::WlPointer>,
    cursor: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
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
    source_idx: Option<usize>,
    /// The render targets, if a procedural source feeds this output.
    target: Option<procedural::Target>,
    pointer: Pointer,
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
            source_idx: None,
            target: None,
            pointer: Pointer::default(),
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
            cursor_shape: None,
            outputs: HashMap::new(),
            seats: HashMap::new(),
            pointer_enabled: false,
            pointer_focus: None,
            detached_surface_idxs: Vec::new(),
            detached_targets: Vec::new(),
        }
    }

    fn enable_pointer(&mut self, qh: &QueueHandle<Self>) {
        self.pointer_enabled = true;
        let seats: Vec<u32> = self.seats.keys().copied().collect();
        for name in seats {
            self.sync_pointer(name, qh);
        }
    }

    /// Binds or releases the pointer of the seat to match its capabilities.
    fn sync_pointer(&mut self, seat_name: u32, qh: &QueueHandle<Self>) {
        let Some(seat) = self.seats.get_mut(&seat_name) else {
            return;
        };
        if self.pointer_enabled && seat.has_pointer && seat.pointer.is_none() {
            let pointer = seat.seat.get_pointer(qh, ());
            seat.cursor = self
                .cursor_shape
                .as_ref()
                .map(|m| m.get_pointer(&pointer, qh, ()));
            seat.pointer = Some(pointer);
        } else if !seat.has_pointer {
            seat.release_pointer();
        }
    }

    fn focused_output(&mut self) -> Option<&mut OutputState> {
        self.pointer_focus.and_then(|n| self.outputs.get_mut(&n))
    }

    fn remove_output(&mut self, name: u32, reason: &str) {
        if let Some(removed) = self.outputs.remove(&name) {
            if let Some(idx) = removed.surface_idx {
                self.detached_surface_idxs.push(idx);
            }
            if let Some(target) = removed.target {
                self.detached_targets.push(target);
            }
            if self.pointer_focus == Some(name) {
                self.pointer_focus = None;
            }
            let label = if removed.name.is_empty() {
                format!("wl_output#{name}")
            } else {
                removed.name
            };
            log::info!("{reason}: {label}");
        }
    }

    /// Frees the GL resources of outputs that are gone. The targets go first,
    /// while the context can still be current on their surface.
    fn release_detached(&mut self, renderer: &mut GlRenderer) {
        for target in self.detached_targets.drain(..) {
            target.destroy(renderer.gl());
        }
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
                "wl_seat" => {
                    let seat: wl_seat::WlSeat = registry.bind(name, version.min(5), qh, name);
                    state.seats.insert(
                        name,
                        SeatState {
                            seat,
                            has_pointer: false,
                            pointer: None,
                            cursor: None,
                        },
                    );
                }
                "wp_cursor_shape_manager_v1" => {
                    state.cursor_shape = Some(registry.bind(name, version.min(1), qh, ()));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(mut seat) = state.seats.remove(&name) {
                    seat.release_pointer();
                    return;
                }
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

impl SeatState {
    fn release_pointer(&mut self) {
        if let Some(cursor) = self.cursor.take() {
            cursor.destroy();
        }
        if let Some(pointer) = self.pointer.take()
            && pointer.version() >= 3
        {
            pointer.release();
        }
    }
}

impl Dispatch<wl_seat::WlSeat, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        seat_name: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if let Some(seat) = state.seats.get_mut(seat_name) {
                seat.has_pointer = caps.contains(wl_seat::Capability::Pointer);
            }
            state.sync_pointer(*seat_name, qh);
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                // A client that binds a pointer sets the cursor image over its
                // surfaces. Ask for the normal arrow, so that the cursor does
                // not disappear or keep the shape of the last window.
                if let Some(cursor) = state
                    .seats
                    .values()
                    .find(|s| s.pointer.as_ref() == Some(pointer))
                    .and_then(|s| s.cursor.as_ref())
                {
                    cursor.set_shape(serial, wp_cursor_shape_device_v1::Shape::Default);
                }
                state.pointer_focus = state
                    .outputs
                    .iter()
                    .find(|(_, o)| o.wl_surface.as_ref() == Some(&surface))
                    .map(|(name, _)| *name);
                if let Some(o) = state.focused_output() {
                    o.pointer.enter(surface_x, surface_y);
                }
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(o) = state.focused_output() {
                    o.pointer.leave();
                }
                state.pointer_focus = None;
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if let Some(o) = state.focused_output() {
                    o.pointer.motion(surface_x, surface_y);
                }
            }
            wl_pointer::Event::Button {
                button,
                state: button_state,
                ..
            } => {
                let pressed =
                    matches!(button_state, WEnum::Value(wl_pointer::ButtonState::Pressed));
                if let Some(o) = state.focused_output() {
                    o.pointer.button(button, pressed);
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(State: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
