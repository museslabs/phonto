//! Runs the actions of a pack. A click on the wallpaper runs the action under
//! the pointer. The pack names its actions, and the `[[action]]` entries of
//! the configuration give their commands and their labels. Commands come
//! only from the configuration of the user, never from a pack.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::ActionCommand;

/// The most characters of a label that a pack gets. A longer label is cut.
pub const MAX_LABEL: usize = 32;

/// The character that replaces a character outside the Basic Multilingual
/// Plane, which the label texture cannot store.
const REPLACEMENT: u32 = '?' as u32;

/// The actions of one pack, with their commands.
#[derive(Debug)]
pub struct ActionRunner {
    /// The name of each action, in the order of the pack, and its command.
    entries: Vec<(String, Option<String>)>,
}

impl ActionRunner {
    pub fn new(names: &[String], commands: &[ActionCommand]) -> Self {
        let entries: Vec<_> = names
            .iter()
            .map(|name| {
                let command = find(commands, name).and_then(|c| c.command.clone());
                (name.clone(), command)
            })
            .collect();
        let missing: Vec<&str> = entries
            .iter()
            .filter(|(_, c)| c.is_none())
            .map(|(n, _)| n.as_str())
            .collect();
        if !missing.is_empty() {
            log::info!(
                "the configuration gives no command to {}, so a click on these does nothing",
                missing.join(", ")
            );
        }
        Self { entries }
    }

    /// The index of the action that `value` names. `value` is the red channel
    /// of the actions buffer: 1 is the first action, and 0 is no action.
    pub fn index(&self, value: u8) -> Option<usize> {
        let index = usize::from(value).checked_sub(1)?;
        (index < self.entries.len()).then_some(index)
    }

    /// Starts the command of the action at `index`. The command runs in a
    /// new session, so it continues when phonto stops.
    pub fn run(&self, index: usize) {
        let Some((name, command)) = self.entries.get(index) else {
            return;
        };
        let Some(command) = command else {
            log::info!(
                "action `{name}` has no command. Add a `command` to the [[action]] named `{name}` in the configuration"
            );
            return;
        };
        log::info!("action `{name}`: run `{command}`");
        let mut child = Command::new("sh");
        child
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // A program that a click opens starts in the home directory, as from
        // a launcher. If HOME is not a directory, as in a Nix build, it starts
        // in the directory of phonto, because sh cannot start in a directory
        // that does not exist.
        if let Some(home) = std::env::var_os("HOME").filter(|home| Path::new(home).is_dir()) {
            child.current_dir(home);
        }
        // SAFETY: setsid is async-signal-safe, so it can run between fork and
        // exec. It makes the command the leader of a new session. So a signal
        // to the terminal or the process group of phonto does not stop it.
        unsafe {
            child.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = match child.spawn() {
            Ok(child) => child,
            Err(err) => {
                log::warn!("action `{name}`: cannot start sh: {err}");
                return;
            }
        };
        // A thread waits for the command, so that it does not stay as a
        // zombie process after it ends.
        let (name, command) = (name.clone(), command.clone());
        let waiter = std::thread::Builder::new()
            .name(format!("action:{name}"))
            .spawn(move || match child.wait() {
                Ok(status) if !status.success() => {
                    log::warn!("action `{name}`: `{command}` stopped with {status}");
                }
                Ok(_) => {}
                Err(err) => log::warn!("action `{name}`: {err}"),
            });
        if let Err(err) = waiter {
            log::warn!("action: cannot start a thread that waits for the command: {err}");
        }
    }
}

fn find<'a>(commands: &'a [ActionCommand], name: &str) -> Option<&'a ActionCommand> {
    commands.iter().find(|c| c.name == name)
}

/// The labels of the actions, as RGBA8 texels for the `iActionLabels` texture:
/// one row of `MAX_LABEL` texels for each action, in the order of `names`. Red
/// and green are the low and the high byte of the Unicode code point of each
/// character. They are 0 after the end of the label. Blue is 255 in the whole
/// row if the action has a command.
pub fn label_texels(names: &[String], commands: &[ActionCommand]) -> Vec<u8> {
    let mut texels = vec![0u8; names.len() * MAX_LABEL * 4];
    let (rows, _) = texels.as_chunks_mut::<{ MAX_LABEL * 4 }>();
    for (row, name) in rows.iter_mut().zip(names) {
        let entry = find(commands, name);
        let has_command = entry.is_some_and(|e| e.command.is_some());
        let label = entry.and_then(|e| e.label.as_deref()).unwrap_or("");
        if label.chars().count() > MAX_LABEL {
            log::warn!(
                "the label of action `{name}` has more than {MAX_LABEL} characters, so the wallpaper shows only the first {MAX_LABEL}"
            );
        }
        let mut chars = label.chars().take(MAX_LABEL);
        let blue = if has_command { 255 } else { 0 };
        for texel in row.as_chunks_mut::<4>().0 {
            let code = match chars.next() {
                None => 0,
                Some(c) if u32::from(c) > 0xFFFF => REPLACEMENT,
                Some(c) => u32::from(c),
            };
            *texel = [(code & 0xFF) as u8, (code >> 8) as u8, blue, 255];
        }
    }
    texels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_code_points_in_rows() {
        let names = ["a".to_string(), "b".to_string(), "c".to_string()];
        let commands = [
            ActionCommand {
                name: "a".into(),
                command: Some("firefox".into()),
                label: Some("Fé λ".into()),
            },
            ActionCommand {
                name: "b".into(),
                label: Some("x".repeat(40)),
                ..ActionCommand::default()
            },
        ];
        let texels = label_texels(&names, &commands);
        assert_eq!(texels.len(), 3 * MAX_LABEL * 4);
        let row = |r: usize| &texels[r * MAX_LABEL * 4..(r + 1) * MAX_LABEL * 4];
        let code =
            |r: usize, i: usize| u32::from(row(r)[i * 4]) | u32::from(row(r)[i * 4 + 1]) << 8;
        assert_eq!(
            [code(0, 0), code(0, 1), code(0, 2), code(0, 3), code(0, 4)],
            ['F' as u32, 'é' as u32, ' ' as u32, 'λ' as u32, 0]
        );
        // The blue channel tells that the action has a command.
        assert_eq!(row(0)[2], 255);
        assert_eq!(row(1)[2], 0);
        // A long label is cut, and an action without an entry is empty.
        assert_eq!(code(1, MAX_LABEL - 1), 'x' as u32);
        assert!((0..MAX_LABEL).all(|i| code(2, i) == 0));
    }

    #[test]
    fn values_name_actions_from_one() {
        let names = ["tape1".to_string(), "tape2".to_string()];
        let commands = [ActionCommand {
            name: "tape2".into(),
            command: Some("true".into()),
            label: None,
        }];
        let actions = ActionRunner::new(&names, &commands);
        assert_eq!(actions.index(0), None);
        assert_eq!(actions.index(1), Some(0));
        assert_eq!(actions.index(2), Some(1));
        assert_eq!(actions.index(3), None);
        assert_eq!(actions.entries[0].1, None);
        assert_eq!(actions.entries[1].1.as_deref(), Some("true"));
    }

    #[test]
    fn runs_the_command_in_a_new_session() {
        let dir = std::env::temp_dir().join(format!("phonto-action-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("pid");
        let hold = dir.join("hold");
        std::fs::write(&hold, "").unwrap();
        // The command writes its process id. Then it stays while `hold` is
        // there, at most 10 s, so that the test can look at it.
        let command = format!(
            "echo $$ > '{}'; i=0; while [ -e '{}' ] && [ $i -lt 10 ]; do sleep 1; i=$((i + 1)); done",
            out.display(),
            hold.display()
        );
        let names = ["a".to_string()];
        let commands = [ActionCommand {
            name: "a".into(),
            command: Some(command),
            label: None,
        }];
        ActionRunner::new(&names, &commands).run(0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let text = loop {
            if let Ok(text) = std::fs::read_to_string(&out)
                && text.ends_with('\n')
            {
                break text;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the command did not run"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let pid: libc::pid_t = text.trim().parse().expect("a process id");
        // The session id of the leader of a new session is its own process
        // id. getsid works the same on Linux and macOS, unlike `ps`.
        // SAFETY: getsid only reads the session id of a process.
        let session = unsafe { libc::getsid(pid) };
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(session, pid, "the command is not a session leader");
    }
}
