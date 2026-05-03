use anyhow::Context;
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    protocol::{wl_callback, wl_compositor, wl_registry, wl_surface},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

pub struct State {
    pub conn: Connection,
    compositor: Option<wl_compositor::WlCompositor>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    width: u32,
    height: u32,
    configured: bool,
    frame_callback_pending: bool,
    surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
}

impl State {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn,
            compositor: None,
            layer_shell: None,
            width: 1,
            height: 1,
            configured: false,
            frame_callback_pending: false,
            surface: None,
            layer_surface: None,
        }
    }

    pub fn create_background_surface(&mut self, qh: &QueueHandle<Self>) -> anyhow::Result<()> {
        let compositor = self.compositor.as_ref().context("wl_compositor missing")?;
        let layer_shell = self
            .layer_shell
            .as_ref()
            .context("zwlr_layer_shell_v1 missing")?;

        let surface = compositor.create_surface(qh, ());
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Background,
            "phonto".to_string(),
            qh,
            (),
        );

        layer_surface.set_size(0, 0);
        layer_surface.set_anchor(zwlr_layer_surface_v1::Anchor::all());
        layer_surface.set_exclusive_zone(-1);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);

        surface.commit();

        self.surface = Some(surface);
        self.layer_surface = Some(layer_surface);
        Ok(())
    }

    pub fn wait_until_configured(
        &mut self,
        event_queue: &mut EventQueue<Self>,
    ) -> anyhow::Result<()> {
        while !self.configured {
            event_queue
                .blocking_dispatch(self)
                .context("waiting for layer surface configure")?;
        }
        Ok(())
    }

    pub fn wait_for_frame_callback(
        &mut self,
        event_queue: &mut EventQueue<Self>,
    ) -> anyhow::Result<()> {
        while self.frame_callback_pending {
            event_queue
                .blocking_dispatch(self)
                .context("waiting for frame callback")?;
        }
        Ok(())
    }

    pub fn request_frame_callback(&mut self, qh: &QueueHandle<Self>) {
        self.surface
            .as_ref()
            .expect("wl_surface missing")
            .frame(qh, ());
        self.frame_callback_pending = true;
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
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };

        match interface.as_str() {
            "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
            }
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            layer_surface.ack_configure(serial);
            if width > 0 && height > 0 {
                state.width = width;
                state.height = height;
            }
            state.configured = true;
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.frame_callback_pending = false;
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
