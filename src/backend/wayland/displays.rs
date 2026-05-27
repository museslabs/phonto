use std::collections::HashMap;

use anyhow::Context;
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{wl_output, wl_registry},
};

use crate::displays::DisplayInfo;

pub fn list_displays() -> anyhow::Result<Vec<DisplayInfo>> {
    let conn = Connection::connect_to_env().context("connect to Wayland display")?;
    let mut eq = conn.new_event_queue();
    let qh = eq.handle();

    let mut state = DisplaysState::default();
    conn.display().get_registry(&qh, ());

    eq.roundtrip(&mut state).context("registry roundtrip")?;
    eq.roundtrip(&mut state)
        .context("output events roundtrip")?;

    let mut out: Vec<DisplayInfo> = state.outputs.into_values().collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

#[derive(Default)]
struct DisplaysState {
    outputs: HashMap<u32, DisplayInfo>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for DisplaysState {
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
        if interface == "wl_output" {
            let v = version.min(4);
            let _: wl_output::WlOutput = registry.bind(name, v, qh, name);
            state.outputs.insert(name, DisplayInfo::default());
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for DisplaysState {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        registry_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(entry) = state.outputs.get_mut(registry_name) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { make, model, .. } if entry.description.is_empty() => {
                let combined = format!("{make} {model}");
                entry.description = combined.trim().to_string();
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
                    entry.width = width.max(0) as u32;
                    entry.height = height.max(0) as u32;
                }
            }
            wl_output::Event::Name { name } => {
                entry.id = name;
            }
            wl_output::Event::Description { description } => {
                entry.description = description;
            }
            _ => {}
        }
    }
}
