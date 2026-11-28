//! Reads the light or dark preference of the desktop from the XDG desktop
//! portal, and follows its changes. GNOME, KDE, and the GTK portal that other
//! desktops use all give it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use gio::glib::{self, Variant, VariantTy};
use gio::prelude::*;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";
/// The portal value for "prefer dark". 0 means no preference, and 2 means
/// light.
const PREFER_DARK: u32 = 1;

/// True while the desktop prefers dark colors. A thread keeps it current.
pub fn watch() -> Arc<AtomicBool> {
    let dark = Arc::new(AtomicBool::new(false));
    let shared = Arc::clone(&dark);
    let spawned = std::thread::Builder::new()
        .name("color-scheme".into())
        .spawn(move || {
            if let Err(err) = follow(&shared) {
                log::info!("no system color scheme, so shaders get light: {err:#}");
            }
        });
    if let Err(err) = spawned {
        log::warn!("cannot follow the system color scheme: {err}");
    }
    dark
}

/// Reads the preference once. False if the portal does not answer.
pub fn read_once() -> bool {
    gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .ok()
        .and_then(|bus| read(&bus).ok())
        .unwrap_or(false)
}

/// Reads the preference, then waits for changes on a main loop of this
/// thread. It returns only on an error.
fn follow(dark: &Arc<AtomicBool>) -> anyhow::Result<()> {
    let context = glib::MainContext::new();
    context.with_thread_default(|| {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .context("connect to the session bus")?;
        dark.store(read(&bus)?, Ordering::Relaxed);
        let shared = Arc::clone(dark);
        let _subscription = bus.subscribe_to_signal(
            Some(PORTAL),
            Some(SETTINGS),
            Some("SettingChanged"),
            Some(PATH),
            Some(NAMESPACE),
            gio::DBusSignalFlags::NONE,
            move |signal| {
                // The arguments are the namespace, the key, and the value.
                let args = signal.parameters;
                if args.is_type(VariantTy::new("(ssv)").expect("valid type"))
                    && args.child_value(1).str() == Some(KEY)
                {
                    shared.store(is_dark(&args.child_value(2)), Ordering::Relaxed);
                }
            },
        );
        glib::MainLoop::new(Some(&context), false).run();
        Ok(())
    })?
}

fn read(bus: &gio::DBusConnection) -> anyhow::Result<bool> {
    let args = (NAMESPACE, KEY).to_variant();
    let reply_type = VariantTy::new("(v)").expect("valid type");
    // Portals before version 2 have only Read, which wraps the value once
    // more.
    let reply = ["ReadOne", "Read"]
        .into_iter()
        .find_map(|method| {
            bus.call_sync(
                Some(PORTAL),
                PATH,
                SETTINGS,
                method,
                Some(&args),
                Some(reply_type),
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
            )
            .ok()
        })
        .context("the desktop portal has no color scheme")?;
    Ok(is_dark(&reply.child_value(0)))
}

fn is_dark(value: &Variant) -> bool {
    let mut value = value.clone();
    while value.is_type(VariantTy::VARIANT) {
        let Some(inner) = value.as_variant() else {
            return false;
        };
        value = inner;
    }
    value.get::<u32>() == Some(PREFER_DARK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plain_and_wrapped_values() {
        assert!(is_dark(&PREFER_DARK.to_variant()));
        assert!(is_dark(&Variant::from_variant(&PREFER_DARK.to_variant())));
        assert!(!is_dark(&2u32.to_variant()));
        assert!(!is_dark(&"dark".to_variant()));
    }
}
