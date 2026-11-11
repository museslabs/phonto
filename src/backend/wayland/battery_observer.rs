use std::path::Path;

use crate::backend::PauseMode;

const POWER_SUPPLY: &str = "/sys/class/power_supply";

/// The power state of the system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Battery {
    /// The charge in percent. `None` if the system has no battery.
    pub level: Option<u8>,
    pub charging: bool,
    pub on_battery: bool,
}

impl Battery {
    pub fn read() -> Self {
        Self::read_from(Path::new(POWER_SUPPLY))
    }

    fn read_from(root: &Path) -> Self {
        let mut battery = Battery {
            level: None,
            charging: false,
            on_battery: false,
        };
        let Ok(entries) = std::fs::read_dir(root) else {
            return battery;
        };
        let mut found_ac = false;
        let mut ac_online = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            // AC adapter nodes are typically named AC, AC0, ADP0, ADP1, etc.
            if name.starts_with("AC") || name.starts_with("ADP") {
                found_ac = true;
                ac_online |= read_sysfs(&path.join("online")).as_deref() == Some("1");
            } else if name.starts_with("BAT")
                && battery.level.is_none()
                && let Some(level) = read_sysfs(&path.join("capacity")).and_then(|s| s.parse().ok())
            {
                battery.level = Some(u8::min(level, 100));
                battery.charging = read_sysfs(&path.join("status")).as_deref() == Some("Charging");
            }
        }
        // The system is on battery if it has AC adapters and none is online.
        // A system without an AC adapter is probably a desktop, so it counts
        // as AC power.
        battery.on_battery = found_ac && !ac_online;
        battery
    }

    pub fn should_pause(&self, mode: &PauseMode) -> bool {
        match mode {
            PauseMode::Never => false,
            PauseMode::OnBattery => self.on_battery,
            PauseMode::BelowPercent(threshold) => {
                self.on_battery && self.level.is_some_and(|p| p < *threshold)
            }
        }
    }

    /// `iBattery`: the charge from 0 to 1, 1 while charging, 1 on AC power,
    /// and 1 if the system has a battery. Without a battery, the charge is 1.
    pub fn uniform(&self) -> [f32; 4] {
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        [
            self.level.map_or(1.0, |p| f32::from(p) / 100.0),
            flag(self.charging),
            flag(!self.on_battery),
            flag(self.level.is_some()),
        ]
    }
}

fn read_sysfs(path: &Path) -> Option<String> {
    Some(std::fs::read_to_string(path).ok()?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake power_supply folder: each node is a name and its files.
    fn power_supply(tag: &str, nodes: &[(&str, &[(&str, &str)])]) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("phonto-battery-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (node, files) in nodes {
            std::fs::create_dir_all(root.join(node)).unwrap();
            for (file, contents) in *files {
                std::fs::write(root.join(node).join(file), format!("{contents}\n")).unwrap();
            }
        }
        root
    }

    #[test]
    fn laptop_on_battery_and_charging() {
        let root = power_supply(
            "laptop",
            &[
                ("AC", &[("online", "0")]),
                ("BAT0", &[("capacity", "42"), ("status", "Discharging")]),
            ],
        );
        let battery = Battery::read_from(&root);
        assert_eq!(battery.uniform(), [0.42, 0.0, 0.0, 1.0]);
        assert!(battery.should_pause(&PauseMode::OnBattery));
        assert!(battery.should_pause(&PauseMode::BelowPercent(50)));
        assert!(!battery.should_pause(&PauseMode::BelowPercent(40)));

        std::fs::write(root.join("AC/online"), "1\n").unwrap();
        std::fs::write(root.join("BAT0/status"), "Charging\n").unwrap();
        let battery = Battery::read_from(&root);
        assert_eq!(battery.uniform(), [0.42, 1.0, 1.0, 1.0]);
        assert!(!battery.should_pause(&PauseMode::OnBattery));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn desktop_without_battery() {
        let root = power_supply("desktop", &[]);
        let battery = Battery::read_from(&root);
        assert_eq!(battery.uniform(), [1.0, 0.0, 1.0, 0.0]);
        assert!(!battery.should_pause(&PauseMode::OnBattery));
        std::fs::remove_dir_all(root).unwrap();
    }
}
