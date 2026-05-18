use std::path::PathBuf;

use crate::backend::PauseMode;

pub fn should_pause(mode: &PauseMode) -> bool {
    match mode {
        PauseMode::Never => false,
        PauseMode::OnBattery => on_battery(),
        PauseMode::BelowPercent(threshold) => {
            on_battery() && battery_percent().is_some_and(|p| p < *threshold)
        }
    }
}

fn on_battery() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    let mut found_ac = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // AC adapter nodes are typically named AC, AC0, ADP0, ADP1, etc.
        if name_str.starts_with("AC") || name_str.starts_with("ADP") {
            found_ac = true;
            if read_sysfs_u8(entry.path().join("online")) == Some(1) {
                return false;
            }
        }
    }
    // If we found AC adapters and none are online, we're on battery.
    // If no AC node exists at all this is probably a desktop — treat as AC.
    found_ac
}

fn battery_percent() -> Option<u8> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("BAT")
            && let Some(pct) = read_sysfs_u8(entry.path().join("capacity"))
        {
            return Some(pct.min(100));
        }
    }
    None
}

fn read_sysfs_u8(path: PathBuf) -> Option<u8> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}
