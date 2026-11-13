mod audio;
mod battery_observer;
mod color_scheme;
mod decoder;
mod gl_renderer;
mod procedural;

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant, SystemTime},
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
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use self::battery_observer::Battery;
use self::gl_renderer::{GlRenderer, TextureRef};
use super::{Backend, DumpRequest, PauseMode, RunOptions};
use crate::config::{ActionCommand, InputCommand};
use crate::displays::DisplayInfo;
use crate::plan::Playback;
use crate::procedural::{
    ActionRunner, AudioFrame, Clock, CommandInputs, FrameInputs, FrameLimiter, MAX_DISPLAYS, Pack,
    Pointer, TextureWatcher, VideoQueue, label_texels, local_date, location, read_inputs_once,
};
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

/// The longest wait of `dump` for the next frame of a video.
const DUMP_VIDEO_WAIT: Duration = Duration::from_secs(10);

impl WaylandBackend {
    fn dump_procedural(self, request: DumpRequest) -> anyhow::Result<()> {
        let mut pack = Pack::load(Path::new(&request.path))?;
        // An input command can write an image that the pack watches, so the
        // commands run first. Then the watched images load again.
        let user_inputs = read_inputs_once(&pack.inputs, &request.inputs);
        pack.reload_watched();
        let compositor = self
            .state
            .compositor
            .as_ref()
            .context("wl_compositor missing")?;
        let wl_surface = compositor.create_surface(&self.qh, ());
        let renderer =
            GlRenderer::new(&self.state.conn, &wl_surface, 1, 1, self.shader.as_deref())?;
        let gl = renderer.gl();
        let mut program = procedural::Program::compile(gl, &pack)?;
        if let Some(actions) = &pack.actions {
            program.set_action_labels(gl, &label_texels(&actions.names, &request.actions))?;
        }
        let (width, height) = request.size.unwrap_or(DEFAULT_DUMP_SIZE);

        let result = (|| {
            // The videos play during the frames before `--at`, as on screen,
            // so a buffer that keeps its last frame sees each video frame.
            let mut videos = pack
                .videos
                .iter()
                .map(|path| VideoChannel::start(path, &renderer))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mut shown = vec![false; videos.len()];
            let dims = program.target_dims((width, height));
            let mut target = program.create_target(gl, dims)?;
            let whole = [0, 0, width as i32, height as i32];
            let (displays, display_count) = display_rects(whole, &[whole], dims);
            let fps = pack.fps.map_or(UNCAPPED_DUMP_FPS, f64::from);
            let frames = (request.at * fps).floor() as u64;
            let wall = SystemTime::now();
            // dump uses the system inputs of this moment, as it does for iDate.
            let system = FrameInputs {
                date: local_date(wall),
                color_scheme: if color_scheme::read_once() { 1.0 } else { 0.0 },
                battery: Battery::read().uniform(),
                location: location(wall, request.location),
                inputs: user_inputs,
                displays,
                display_count,
                ..FrameInputs::default()
            };
            let mut texture = None;
            let mut previous = 0.0;
            for i in 0..=frames {
                let time = if i == frames {
                    request.at
                } else {
                    i as f64 / fps
                };
                let delta = time - previous;
                previous = time;
                for (index, video) in videos.iter_mut().enumerate() {
                    if let Some(sample) = video.queue.advance(delta, DUMP_VIDEO_WAIT) {
                        video.upload(gl, &mut program, index, sample);
                        video.release_copied(gl, 0);
                        shown[index] = true;
                    }
                }
                let inputs = FrameInputs {
                    time: time as f32,
                    time_delta: delta as f32,
                    frame_rate: fps as f32,
                    ..system
                };
                texture = Some(program.render(gl, &mut target, &inputs));
            }
            if let Some(index) = shown.iter().position(|shown| !shown) {
                bail!(
                    "video {}: the decoder gave no frame",
                    pack.videos[index].display()
                );
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
            if options.span && self.state.xdg_output_manager.is_none() {
                bail!(
                    "--span needs the place of each display, but this compositor does not \
                     offer the xdg-output protocol"
                );
            }
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
                SourceSpec::Procedural(pack) => Source::Procedural(Box::new(
                    ProceduralSource::new(&renderer, &pack, &options.inputs, &options.actions)?,
                )),
            });
        }

        // One capture gives audio to all the sources that use it, on all
        // outputs.
        let mut audio = if sources
            .iter()
            .any(|s| matches!(s, Source::Procedural(p) if p.audio))
        {
            match audio::AudioCapture::start() {
                Ok(capture) => Some(capture),
                Err(err) => {
                    log::warn!("no system audio, so the wallpaper draws without it: {err:#}");
                    None
                }
            }
        } else {
            None
        };

        // Procedural wallpapers follow the light or dark preference of the
        // desktop.
        let dark_scheme = sources
            .iter()
            .any(|s| matches!(s, Source::Procedural(_)))
            .then(color_scheme::watch);
        if options.span && sources.iter().any(|s| matches!(s, Source::Video(_))) {
            log::warn!(
                "--span only applies to procedural wallpapers. A video plays on each display."
            );
        }

        let mut battery = Battery::read();
        let mut paused = battery.should_pause(&options.pause);
        let mut last_battery_check = Instant::now();
        log_pause_state(paused, &options.pause);
        pause_readers(paused, audio.as_ref(), &sources);

        loop {
            if last_battery_check.elapsed() >= BATTERY_POLL_INTERVAL {
                battery = Battery::read();
                let new_paused = battery.should_pause(&options.pause);
                if new_paused != paused {
                    paused = new_paused;
                    log_pause_state(paused, &options.pause);
                    pause_readers(paused, audio.as_ref(), &sources);
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
                // A spanned output draws its part only after the compositor
                // tells where it is, for example just after a hotplug.
                let spanned = options.span && matches!(sources[source_idx], Source::Procedural(_));
                if spanned && o.position.is_none() {
                    continue;
                }
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
            let audio_frame = audio
                .as_mut()
                .map_or_else(AudioFrame::default, |a| a.snapshot());
            let color_scheme = match &dark_scheme {
                Some(dark) if dark.load(Ordering::Relaxed) => 1.0,
                _ => 0.0,
            };
            // Some(true) if the pointer moved onto an action, and Some(false)
            // if it moved off.
            let mut hand = None;
            for (source_idx, output_names) in groups {
                let source = &mut sources[source_idx];
                let first_surface = output_names
                    .iter()
                    .find_map(|n| self.state.outputs.get(n)?.surface_idx)
                    .expect("a group has an output");
                let tick = match source {
                    Source::Procedural(p) => {
                        let tick = p.clock.tick(now);
                        p.upload(&mut renderer, first_surface, f64::from(tick.delta))?;
                        Some(tick)
                    }
                    Source::Video(_) => None,
                };
                // The inputs that all outputs of a procedural source share.
                let shared = match (&*source, tick) {
                    (Source::Procedural(p), Some(tick)) => {
                        let wall = SystemTime::now();
                        Some(FrameInputs {
                            time: tick.time,
                            time_delta: tick.delta,
                            frame_rate: tick.frame_rate,
                            date: local_date(wall),
                            audio: if p.audio {
                                audio_frame
                            } else {
                                AudioFrame::default()
                            },
                            color_scheme,
                            battery: battery.uniform(),
                            location: location(wall, options.location),
                            inputs: p.commands.values(),
                            ..FrameInputs::default()
                        })
                    }
                    _ => None,
                };
                // A spanned wallpaper draws once for the whole desktop. Each
                // output then shows its own part.
                let spanned = match (&mut *source, shared) {
                    (Source::Procedural(p), Some(shared)) if options.span => {
                        let rects: Vec<[i32; 4]> = output_names
                            .iter()
                            .filter_map(|n| self.state.outputs.get(n)?.rect())
                            .collect();
                        let desktop = desktop_rect(rects.iter().copied());
                        renderer.make_current(first_surface)?;
                        let gl = renderer.gl();
                        let dims = p
                            .program
                            .target_dims((desktop[2] as u32, desktop[3] as u32));
                        if p.span_target.as_ref().map(procedural::Target::dims) != Some(dims) {
                            if let Some(old) = p.span_target.take() {
                                old.destroy(gl);
                            }
                            p.span_target = Some(p.program.create_target(gl, dims)?);
                            // Read the action under the pointer again.
                            self.state.desktop_hover = None;
                        }
                        let area = desktop.map(f64::from);
                        // The first frame of a new target leaves the click
                        // for the next frame, as `handle_actions` does.
                        let drawn = p
                            .span_target
                            .as_ref()
                            .is_some_and(procedural::Target::has_drawn);
                        let (mouse, pointer) =
                            self.state.desktop_pointer.uniforms(area, dims, drawn);
                        let (displays, display_count) = display_rects(desktop, &rects, dims);
                        let inputs = FrameInputs {
                            mouse,
                            pointer,
                            displays,
                            display_count,
                            ..shared
                        };
                        let target = p.span_target.as_mut().expect("target created above");
                        let over = handle_actions(
                            gl,
                            &p.program,
                            p.actions.as_ref(),
                            target,
                            &mut self.state.desktop_pointer,
                            area,
                            &mut self.state.desktop_hover,
                        );
                        if over.is_some() {
                            hand = over;
                        }
                        Some((p.program.render(gl, target, &inputs), desktop))
                    }
                    _ => None,
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
                        Source::Procedural(_) if spanned.is_some() => {
                            let (texture, desktop) = spanned.expect("checked above");
                            let rect = o.rect().expect("spanned outputs have a position");
                            renderer.present_part(surface_idx, texture, crop(desktop, rect))?;
                        }
                        Source::Procedural(p) => {
                            let shared = shared.expect("procedural sources have inputs");
                            renderer.make_current(surface_idx)?;
                            let gl = renderer.gl();
                            let dims = p.program.target_dims(surface_dims);
                            if o.target.as_ref().map(procedural::Target::dims) != Some(dims) {
                                if let Some(old) = o.target.take() {
                                    old.destroy(gl);
                                }
                                o.target = Some(p.program.create_target(gl, dims)?);
                                // Read the action under the pointer again.
                                o.hover = None;
                            }
                            let area = [
                                0.0,
                                0.0,
                                f64::from(surface_dims.0),
                                f64::from(surface_dims.1),
                            ];
                            // The first frame of a new target leaves the
                            // click for the next frame, as `handle_actions`
                            // does.
                            let drawn =
                                o.target.as_ref().is_some_and(procedural::Target::has_drawn);
                            let (mouse, pointer) = o.pointer.uniforms(area, dims, drawn);
                            let whole = [0, 0, surface_dims.0 as i32, surface_dims.1 as i32];
                            let (displays, display_count) = display_rects(whole, &[whole], dims);
                            let inputs = FrameInputs {
                                mouse,
                                pointer,
                                displays,
                                display_count,
                                ..shared
                            };
                            let target = o.target.as_mut().expect("target created above");
                            let over = handle_actions(
                                gl,
                                &p.program,
                                p.actions.as_ref(),
                                target,
                                &mut o.pointer,
                                area,
                                &mut o.hover,
                            );
                            // Only the output under the pointer sets the
                            // cursor.
                            if over.is_some() && self.state.pointer_focus == Some(name) {
                                hand = over;
                            }
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
            if let Some(hand) = hand {
                self.state.set_hand(hand);
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
    Procedural(Box<ProceduralSource>),
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
    audio: bool,
    /// All outputs that show this source share it, so mirrored outputs stay
    /// in sync.
    clock: Clock,
    limiter: FrameLimiter,
    /// The render targets for the whole desktop, with `--span`.
    span_target: Option<procedural::Target>,
    /// The values of the inputs that the pack reads.
    commands: CommandInputs,
    watcher: Option<TextureWatcher>,
    /// The videos of the channels, in the order of `Pack::videos`.
    videos: Vec<VideoChannel>,
    /// The actions that a click runs, if the pack has any.
    actions: Option<ActionRunner>,
}

impl ProceduralSource {
    fn new(
        renderer: &GlRenderer,
        pack: &Pack,
        commands: &[InputCommand],
        actions: &[ActionCommand],
    ) -> anyhow::Result<Self> {
        let videos = pack
            .videos
            .iter()
            .map(|path| VideoChannel::start(path, renderer))
            .collect::<anyhow::Result<_>>()?;
        let mut program = procedural::Program::compile(renderer.gl(), pack)?;
        if let Some(a) = &pack.actions {
            program.set_action_labels(renderer.gl(), &label_texels(&a.names, actions))?;
        }
        Ok(Self {
            program,
            audio: pack.audio,
            clock: Clock::new(pack.fps),
            limiter: FrameLimiter::new(pack.fps),
            span_target: None,
            commands: CommandInputs::start(&pack.inputs, commands),
            watcher: TextureWatcher::start(&pack.textures),
            videos,
            actions: pack
                .actions
                .as_ref()
                .map(|a| ActionRunner::new(&a.names, actions)),
        })
    }

    /// Uploads the new images of watched files, and the video frames that
    /// are due after `delta` more seconds of playback. The context becomes
    /// current on `surface` if there is something to upload.
    fn upload(
        &mut self,
        renderer: &mut GlRenderer,
        surface: usize,
        delta: f64,
    ) -> anyhow::Result<()> {
        let updates = self
            .watcher
            .as_ref()
            .map(TextureWatcher::take)
            .unwrap_or_default();
        let frames: Vec<(usize, gst::Sample)> = self
            .videos
            .iter_mut()
            .enumerate()
            .filter_map(|(i, v)| Some((i, v.advance(delta)?)))
            .collect();
        if updates.is_empty()
            && frames.is_empty()
            && self.videos.iter().all(|v| v.copying.is_empty())
        {
            return Ok(());
        }
        renderer.make_current(surface)?;
        let gl = renderer.gl();
        for video in &mut self.videos {
            video.release_copied(gl, MAX_COPYING);
        }
        for (index, texture) in updates {
            if let Err(err) = self.program.replace_texture(gl, index, &texture) {
                log::warn!("{err:#}");
            }
        }
        for (index, sample) in frames {
            self.videos[index].upload(gl, &mut self.program, index, sample);
        }
        Ok(())
    }
}

/// Runs the action under each click since the last frame, and finds the
/// action under the pointer. Returns `Some(true)` if the pointer is over an
/// action, `Some(false)` if it is not, and `None` if nothing changed or the
/// pack has no actions.
///
/// phonto reads the actions from `target`, which holds the last frame, so a
/// click runs the action that the user saw. A new target has no frame yet. Its
/// clicks wait for its first frame. `iMouse` also gives them to the frame
/// after it, so the action and the shader see a click on the same frame.
///
/// `hover` is where phonto last read the action under the pointer. If the
/// actions buffer draws only once, the actions do not move, so phonto reads
/// again only when the pointer moves.
fn handle_actions(
    gl: &glow::Context,
    program: &procedural::Program,
    actions: Option<&ActionRunner>,
    target: &procedural::Target,
    pointer: &mut Pointer,
    area: [f64; 4],
    hover: &mut Option<[f64; 2]>,
) -> Option<bool> {
    let actions = actions?;
    if !target.has_drawn() {
        return None;
    }
    let action_at = |at: [f64; 2]| {
        program
            .read_action(gl, target, at)
            .and_then(|value| actions.index(value))
    };
    for at in pointer.take_presses(area) {
        if let Some(index) = action_at(at) {
            actions.run(index);
        }
    }
    let now = pointer.hover(area);
    if now == *hover && program.actions_are_static() {
        return None;
    }
    *hover = now;
    Some(now.and_then(action_at).is_some())
}

/// A video that a channel of a procedural wallpaper shows. It plays at the
/// speed of the playback clock of the wallpaper, so it stops while time
/// stops.
struct VideoChannel {
    queue: Arc<VideoQueue<gst::Sample>>,
    gl_context: gst_gl::GLContext,
    pipeline: gst::Pipeline,
    /// Decoded frames that the GPU still copies, with the fence that tells
    /// when the copy is done. A sample keeps its texture from the decoder
    /// until its fence signals.
    copying: VecDeque<(gst::Sample, glow::Fence)>,
    /// True after a frame did not upload, until one works again.
    failing: bool,
    /// True while more copies wait than the last release allowed, because
    /// the GPU did not finish the oldest one in time.
    stalled: bool,
}

/// The most frames that wait for their copy. If the GPU falls behind, phonto
/// waits for the oldest copy, so the decoder does not run out of buffers.
const MAX_COPYING: usize = 3;

/// Takes the copies that are done off the front of `copying`. `done(fence,
/// wait)` tells if a copy is done, and with `wait` it first waits for it a
/// short time. If more than `max` copies wait, this waits for the oldest
/// ones. A copy that is not done stays, and so do the copies after it.
fn take_done<S, F>(
    copying: &mut VecDeque<(S, F)>,
    max: usize,
    mut done: impl FnMut(&F, bool) -> bool,
) -> Vec<(S, F)> {
    let mut taken = Vec::new();
    while let Some((_, fence)) = copying.front() {
        if !done(fence, copying.len() > max) {
            break;
        }
        taken.extend(copying.pop_front());
    }
    taken
}

/// The longest wait for a video that is behind, as a part of the time since
/// the last frame of the wallpaper. It is never more than `MAX_CATCH_UP_WAIT`
/// seconds.
const CATCH_UP_SHARE: f64 = 0.25;
const MAX_CATCH_UP_WAIT: f64 = 0.025;

impl VideoChannel {
    fn start(path: &Path, renderer: &GlRenderer) -> anyhow::Result<Self> {
        let (gl_display, gl_context) =
            decoder::wrap_gl(renderer.egl_display(), renderer.egl_context())?;
        let queue = Arc::new(VideoQueue::new());
        let pipeline = decoder::start_channel(
            &path.to_string_lossy(),
            &gl_display,
            &gl_context,
            Arc::clone(&queue),
        )
        .with_context(|| format!("video {}", path.display()))?;
        Ok(Self {
            queue,
            gl_context,
            pipeline,
            copying: VecDeque::new(),
            failing: false,
            stalled: false,
        })
    }

    /// Moves the video `delta` seconds on, and returns the newest frame that
    /// is due, if a new one is.
    ///
    /// Between two frames of the wallpaper, the decoder catches up with the
    /// time of the last frame. Then phonto waits a short time until the
    /// decoder gives a frame that is not due yet, so that the frame of this
    /// moment shows. If the decoder is too slow, its newest frame shows.
    fn advance(&self, delta: f64) -> Option<gst::Sample> {
        let wait = (delta * CATCH_UP_SHARE).min(MAX_CATCH_UP_WAIT);
        self.queue.advance(delta, Duration::from_secs_f64(wait))
    }

    /// Copies `sample` into video `index` of `program`. The sample stays until
    /// the copy is done. The context must be current.
    fn upload(
        &mut self,
        gl: &glow::Context,
        program: &mut procedural::Program,
        index: usize,
        sample: gst::Sample,
    ) {
        // While the GPU did not finish the old copies, a new frame does not
        // show, so that the copies do not pile up. The GPU never read this
        // sample, so it can go.
        if self.stalled {
            return;
        }
        let copied = decoder::with_frame(&sample, &self.gl_context, |frame| {
            program.update_video(gl, index, &TextureRef::from_frame(frame)?)
        });
        // Warn once until a frame works again.
        match copied.and_then(|r| r) {
            Ok(fence) => {
                self.copying.push_back((sample, fence));
                self.failing = false;
            }
            Err(err) if !self.failing => {
                log::warn!("video channel: {err:#}");
                self.failing = true;
            }
            Err(_) => {}
        }
    }

    /// Gives back to the decoder the samples whose copy is done. If more
    /// than `max` copies run, this waits for the oldest ones. The context
    /// must be current.
    ///
    /// The decoder uses the texture of a sample again after the sample goes.
    /// So a sample stays until its copy is done, however long the GPU takes.
    fn release_copied(&mut self, gl: &glow::Context, max: usize) {
        let done = take_done(&mut self.copying, max, |fence, wait| {
            procedural::fence_signaled(gl, *fence, wait)
        });
        for (sample, fence) in done {
            procedural::delete_fence(gl, fence);
            drop(sample);
        }
        let stalled = self.copying.len() > max;
        if stalled && !self.stalled {
            log::warn!("video channel: the GPU did not finish a copy in time");
        }
        self.stalled = stalled;
    }
}

impl Drop for VideoChannel {
    fn drop(&mut self) {
        // The decoder can wait to give a frame. Close the queue first, so
        // that the wait ends and the pipeline can stop.
        self.queue.close();
        // A copy can still run on the GPU. Once the pipeline stops, the
        // decoder does not write to its texture again. GL keeps a texture
        // until the work that reads it is done. The fences need a current
        // context to go, so they stay: this happens only when phonto stops.
        let _ = self.pipeline.set_state(gst::State::Null);
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

/// The rectangle that covers `outputs`, in logical pixels: x, y, width, and
/// height.
fn desktop_rect(outputs: impl Iterator<Item = [i32; 4]>) -> [i32; 4] {
    let mut bounds: Option<[i32; 4]> = None;
    for [x, y, w, h] in outputs {
        let [left, top, right, bottom] = bounds.get_or_insert([x, y, x + w, y + h]);
        *left = (*left).min(x);
        *top = (*top).min(y);
        *right = (*right).max(x + w);
        *bottom = (*bottom).max(y + h);
    }
    let [left, top, right, bottom] = bounds.unwrap_or([0, 0, 1, 1]);
    [left, top, (right - left).max(1), (bottom - top).max(1)]
}

/// `iDisplays` and `iDisplayCount`: the place of each of `outputs` in an
/// image of `dims` pixels that shows `desktop`, from the bottom left.
fn display_rects(
    desktop: [i32; 4],
    outputs: &[[i32; 4]],
    dims: (u32, u32),
) -> ([[f32; 4]; MAX_DISPLAYS], i32) {
    let [dx, dy, dw, dh] = desktop.map(|v| v as f32);
    let (sx, sy) = (dims.0 as f32 / dw, dims.1 as f32 / dh);
    let mut displays = [[0.0; 4]; MAX_DISPLAYS];
    for (display, output) in displays.iter_mut().zip(outputs) {
        let [x, y, w, h] = output.map(|v| v as f32);
        *display = [(x - dx) * sx, (dy + dh - y - h) * sy, w * sx, h * sy];
    }
    (displays, outputs.len().min(MAX_DISPLAYS) as i32)
}

/// The part of `desktop` that `output` shows, as fractions from the top left.
fn crop(desktop: [i32; 4], output: [i32; 4]) -> [f32; 4] {
    let [dx, dy, dw, dh] = desktop.map(|v| v as f32);
    let [x, y, w, h] = output.map(|v| v as f32);
    [(x - dx) / dw, (y - dy) / dh, w / dw, h / dh]
}

/// Pauses or resumes the work beside the drawing: the audio capture and the
/// input commands.
fn pause_readers(paused: bool, audio: Option<&audio::AudioCapture>, sources: &[Source]) {
    if let Some(audio) = audio {
        audio.set_paused(paused);
    }
    for source in sources {
        if let Source::Procedural(p) = source {
            p.commands.set_paused(paused);
        }
    }
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
    /// Gives the place of each output on the desktop, for `--span`.
    xdg_output_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    outputs: HashMap<u32, OutputState>,
    seats: HashMap<u32, SeatState>,
    /// If true, bind the pointer of each seat that has one. It is false
    /// unless a procedural wallpaper plays.
    pointer_enabled: bool,
    /// The registry name of the output under the pointer.
    pointer_focus: Option<u32>,
    /// The pointer in desktop coordinates, for `--span`.
    desktop_pointer: Pointer,
    /// Where phonto last read the action under `desktop_pointer`.
    desktop_hover: Option<[f64; 2]>,
    /// The seat and the serial of the last time the pointer entered a
    /// wallpaper. A new cursor shape needs them.
    pointer_enter: Option<(u32, u32)>,
    /// True while the cursor is a hand, over an action.
    hand: bool,
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
    xdg_output: Option<zxdg_output_v1::ZxdgOutputV1>,
    /// The top left of the output on the desktop, in logical pixels.
    position: Option<(i32, i32)>,
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
    /// Where phonto last read the action under `pointer`.
    hover: Option<[f64; 2]>,
}

impl OutputState {
    fn new(wl_output: wl_output::WlOutput) -> Self {
        Self {
            wl_output,
            xdg_output: None,
            position: None,
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
            hover: None,
        }
    }

    /// The place of the output on the desktop, in logical pixels: x, y,
    /// width, and height. Its size is the size of its layer surface. `None`
    /// until the compositor gives its position.
    fn rect(&self) -> Option<[i32; 4]> {
        let (x, y) = self.position?;
        Some([x, y, self.width.max(1) as i32, self.height.max(1) as i32])
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
            xdg_output_manager: None,
            outputs: HashMap::new(),
            seats: HashMap::new(),
            pointer_enabled: false,
            pointer_focus: None,
            desktop_pointer: Pointer::default(),
            desktop_hover: None,
            pointer_enter: None,
            hand: false,
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

    /// Shows a hand over an action, and the normal arrow elsewhere.
    fn set_hand(&mut self, hand: bool) {
        if self.hand == hand {
            return;
        }
        let Some((seat, serial)) = self.pointer_enter else {
            return;
        };
        if let Some(cursor) = self.seats.get(&seat).and_then(|s| s.cursor.as_ref()) {
            let shape = if hand {
                wp_cursor_shape_device_v1::Shape::Pointer
            } else {
                wp_cursor_shape_device_v1::Shape::Default
            };
            cursor.set_shape(serial, shape);
        }
        self.hand = hand;
    }

    fn focused_output(&mut self) -> Option<&mut OutputState> {
        self.pointer_focus.and_then(|n| self.outputs.get_mut(&n))
    }

    /// The desktop position of the output under the pointer.
    fn focus_offset(&self) -> (f64, f64) {
        let (x, y) = self
            .pointer_focus
            .and_then(|n| self.outputs.get(&n)?.position)
            .unwrap_or_default();
        (f64::from(x), f64::from(y))
    }

    fn remove_output(&mut self, name: u32, reason: &str) {
        if let Some(removed) = self.outputs.remove(&name) {
            if let Some(xdg_output) = &removed.xdg_output {
                xdg_output.destroy();
            }
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
                    let mut o = OutputState::new(output);
                    if let Some(manager) = &state.xdg_output_manager {
                        o.xdg_output = Some(manager.get_xdg_output(&o.wl_output, qh, name));
                    }
                    state.outputs.insert(name, o);
                }
                "zxdg_output_manager_v1" => {
                    let manager: zxdg_output_manager_v1::ZxdgOutputManagerV1 =
                        registry.bind(name, version.min(3), qh, ());
                    for (output_name, o) in &mut state.outputs {
                        o.xdg_output = Some(manager.get_xdg_output(&o.wl_output, qh, *output_name));
                    }
                    state.xdg_output_manager = Some(manager);
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

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        registry_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_output_v1::Event::LogicalPosition { x, y } = event
            && let Some(o) = state.outputs.get_mut(registry_name)
        {
            o.position = Some((x, y));
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
                let seat = state
                    .seats
                    .iter()
                    .find(|(_, s)| s.pointer.as_ref() == Some(pointer));
                if let Some(cursor) = seat.and_then(|(_, s)| s.cursor.as_ref()) {
                    cursor.set_shape(serial, wp_cursor_shape_device_v1::Shape::Default);
                }
                state.pointer_enter = seat.map(|(name, _)| (*name, serial));
                state.hand = false;
                state.pointer_focus = state
                    .outputs
                    .iter()
                    .find(|(_, o)| o.wl_surface.as_ref() == Some(&surface))
                    .map(|(name, _)| *name);
                // The cursor is the arrow again, so read the action under
                // the pointer on the next frame.
                state.desktop_hover = None;
                if let Some(o) = state.focused_output() {
                    o.pointer.enter(surface_x, surface_y);
                    o.hover = None;
                }
                let (x, y) = state.focus_offset();
                state.desktop_pointer.enter(surface_x + x, surface_y + y);
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(o) = state.focused_output() {
                    o.pointer.leave();
                }
                state.desktop_pointer.leave();
                state.pointer_focus = None;
                state.pointer_enter = None;
                state.hand = false;
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if let Some(o) = state.focused_output() {
                    o.pointer.motion(surface_x, surface_y);
                }
                let (x, y) = state.focus_offset();
                state.desktop_pointer.motion(surface_x + x, surface_y + y);
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
                state.desktop_pointer.button(button, pressed);
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore zxdg_output_manager_v1::ZxdgOutputManagerV1);
delegate_noop!(State: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(State: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_that_is_not_done_keeps_its_sample() {
        // Five copies, each a sample with its fence. Copy 1 does not finish,
        // also after a wait.
        let mut copying: VecDeque<(usize, usize)> = (0..5).map(|n| (n, n)).collect();
        let mut waited = Vec::new();
        let done = take_done(&mut copying, 2, |fence, wait| {
            if wait {
                waited.push(*fence);
            }
            *fence != 1
        });
        assert_eq!(done, [(0, 0)]);
        assert_eq!(waited, [0, 1], "more than 2 copies wait, so it waits");
        assert_eq!(copying.front(), Some(&(1, 1)), "copy 1 and its sample stay");
        assert_eq!(copying.len(), 4);
        // Copy 1 finishes later, and the copies after it go with it.
        let done = take_done(&mut copying, 2, |_, _| true);
        assert_eq!(done.len(), 4);
    }

    #[test]
    fn desktop_covers_all_outputs() {
        // A laptop at the left, and a taller monitor above and to the right.
        let laptop = [0, 300, 1280, 800];
        let monitor = [1280, 0, 1920, 1080];
        let desktop = desktop_rect([laptop, monitor].into_iter());
        assert_eq!(desktop, [0, 0, 3200, 1100]);
        assert_eq!(crop(desktop, monitor), [0.4, 0.0, 0.6, 1080.0 / 1100.0]);
        assert_eq!(
            crop(desktop, laptop),
            [0.0, 300.0 / 1100.0, 0.4, 800.0 / 1100.0]
        );
    }

    #[test]
    fn displays_are_in_image_pixels_from_the_bottom_left() {
        let laptop = [0, 300, 1280, 800];
        let monitor = [1280, 0, 1920, 1080];
        let desktop = desktop_rect([laptop, monitor].into_iter());
        // The image is half the size of the desktop.
        let (displays, count) = display_rects(desktop, &[laptop, monitor], (1600, 550));
        assert_eq!(count, 2);
        assert_eq!(displays[0], [0.0, 0.0, 640.0, 400.0]);
        assert_eq!(displays[1], [640.0, 10.0, 960.0, 540.0]);
    }

    #[test]
    fn one_output_shows_everything() {
        let output = [-1920, 0, 1920, 1080];
        let desktop = desktop_rect(std::iter::once(output));
        assert_eq!(desktop, output);
        assert_eq!(crop(desktop, output), [0.0, 0.0, 1.0, 1.0]);
    }
}
