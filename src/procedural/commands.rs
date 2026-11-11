//! Runs the `[[input]]` commands of the configuration for the inputs that a
//! pack reads. Each command prints one to four numbers. Commands come only
//! from the configuration of the user, never from a pack.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use super::pack::{MAX_INPUTS, NamedInput};
use crate::config::InputCommand;

/// The values of the inputs of a pack, in the order of `Pack::inputs`.
pub type InputValues = [[f32; 4]; MAX_INPUTS];

/// The shortest time between two runs of a command.
const MIN_EVERY: Duration = Duration::from_millis(100);
/// A command that runs longer than this stops, and keeps its last value.
const MAX_RUN: Duration = Duration::from_secs(10);

/// The inputs of a pack, which threads keep current.
pub struct CommandInputs {
    values: Arc<Mutex<InputValues>>,
    paused: Arc<AtomicBool>,
}

impl CommandInputs {
    /// Starts one thread for each input of the pack that has a command.
    pub fn start(inputs: &[NamedInput], commands: &[InputCommand]) -> Self {
        let values = Arc::new(Mutex::new(defaults(inputs)));
        let paused = Arc::new(AtomicBool::new(false));
        for (slot, input) in inputs.iter().enumerate() {
            let Some(command) = find(commands, &input.name) else {
                log::info!(
                    "no [[input]] named `{}` in the configuration, so it keeps its default",
                    input.name
                );
                continue;
            };
            // The configuration checks `every`. A value that does not fit a
            // Duration runs as rarely as possible.
            let every = Duration::try_from_secs_f64(command.every)
                .unwrap_or(Duration::MAX)
                .max(MIN_EVERY);
            let limit = every.clamp(Duration::from_secs(1), MAX_RUN);
            let (values, paused) = (Arc::clone(&values), Arc::clone(&paused));
            let thread_name = format!("input:{}", input.name);
            let (input, command) = (input.clone(), command.command.clone());
            let spawned = std::thread::Builder::new()
                .name(thread_name.clone())
                .spawn(move || {
                    let mut failing = false;
                    loop {
                        if !paused.load(Ordering::Relaxed) {
                            match run(&command, limit) {
                                Ok(numbers) => {
                                    if let Ok(mut values) = values.lock() {
                                        apply(&mut values[slot], &input, &numbers);
                                    }
                                    failing = false;
                                }
                                // Warn once until the command works again.
                                Err(err) if !failing => {
                                    log::warn!("[[input]] `{}`: {err:#}", input.name);
                                    failing = true;
                                }
                                Err(_) => {}
                            }
                        }
                        std::thread::sleep(every);
                    }
                });
            if let Err(err) = spawned {
                log::warn!("cannot start {thread_name}: {err}");
            }
        }
        Self { values, paused }
    }

    pub fn values(&self) -> InputValues {
        self.values.lock().map_or([[0.0; 4]; MAX_INPUTS], |v| *v)
    }

    /// Stops running the commands while the wallpaper is paused.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
}

/// Runs each command once and returns the values, for `dump`.
pub fn read_once(inputs: &[NamedInput], commands: &[InputCommand]) -> InputValues {
    let mut values = defaults(inputs);
    for (slot, input) in inputs.iter().enumerate() {
        if let Some(command) = find(commands, &input.name) {
            match run(&command.command, MAX_RUN) {
                Ok(numbers) => apply(&mut values[slot], input, &numbers),
                Err(err) => log::warn!("[[input]] `{}`: {err:#}", input.name),
            }
        }
    }
    values
}

fn defaults(inputs: &[NamedInput]) -> InputValues {
    let mut values = [[0.0; 4]; MAX_INPUTS];
    for (value, input) in values.iter_mut().zip(inputs) {
        *value = input.default;
    }
    values
}

fn find<'a>(commands: &'a [InputCommand], name: &str) -> Option<&'a InputCommand> {
    commands.iter().find(|c| c.name == name)
}

/// Copies the printed numbers into the value. If the command printed fewer
/// numbers than the input has, the others keep their values.
fn apply(value: &mut [f32; 4], input: &NamedInput, numbers: &[f32]) {
    for (v, n) in value.iter_mut().take(input.size).zip(numbers) {
        *v = *n;
    }
}

/// Runs `command` with `sh -c` and reads the numbers that it prints. After
/// `limit`, it stops the command and all the programs that it started.
fn run(command: &str, limit: Duration) -> anyhow::Result<Vec<f32>> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .context("start sh")?;
    // One thread reads each pipe while the command runs, so a command that
    // fills one pipe does not wait for the other. A program that the command
    // leaves running can keep a pipe open, so the reading has the same time
    // limit.
    let stdout = read_in_thread(child.stdout.take());
    let stderr = read_in_thread(child.stderr.take());
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait().context("wait for the command")? {
            break status;
        }
        if Instant::now() >= deadline {
            stop(&mut child);
            bail!("stopped after {} s", limit.as_secs_f32());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let wait = || {
        deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(100))
    };
    let (Ok(out), Ok(err)) = (stdout.recv_timeout(wait()), stderr.recv_timeout(wait())) else {
        stop(&mut child);
        bail!("the command left a program running that keeps its output open");
    };
    if !status.success() {
        bail!("{status}: {}", err.trim());
    }
    parse(&out)
}

/// Reads `pipe` to its end on a new thread, and sends the text.
fn read_in_thread(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (sender, text) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    text
}

/// Stops the process group of `child`.
fn stop(child: &mut Child) {
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill only sends a signal. The negative pid names the
        // process group that `process_group(0)` made for the command.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    let _ = child.wait();
}

/// The numbers in `text`. Spaces, commas, and brackets separate them, so a
/// JSON list also works. jq prints `true` and `false`, which count as 1 and
/// 0.
fn parse(text: &str) -> anyhow::Result<Vec<f32>> {
    let numbers = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '[' | ']'))
        .filter(|t| !t.is_empty())
        .map(|t| match t {
            "true" => Ok(1.0),
            "false" => Ok(0.0),
            _ => t
                .parse::<f32>()
                .ok()
                .filter(|n| n.is_finite())
                .with_context(|| format!("`{t}` is not a number")),
        })
        .collect::<anyhow::Result<Vec<f32>>>()?;
    if numbers.is_empty() {
        bail!("the command printed no number");
    }
    Ok(numbers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, size: usize) -> NamedInput {
        NamedInput {
            name: name.into(),
            size,
            default: [0.5; 4],
        }
    }

    fn command(name: &str, command: &str) -> InputCommand {
        InputCommand {
            name: name.into(),
            command: command.into(),
            every: 1.0,
        }
    }

    #[test]
    fn parses_numbers_lists_and_booleans() {
        assert_eq!(parse("3\n").unwrap(), [3.0]);
        assert_eq!(parse("0.25 1e2").unwrap(), [0.25, 100.0]);
        assert_eq!(parse("[1, 2,3]").unwrap(), [1.0, 2.0, 3.0]);
        assert_eq!(parse("true false").unwrap(), [1.0, 0.0]);
        assert!(parse("").is_err());
        assert!(parse("null").is_err());
        assert!(parse("NaN").is_err());
    }

    #[test]
    fn runs_commands_once_for_dump() {
        let inputs = [input("cpu", 1), input("wind", 2), input("missing", 1)];
        let commands = [
            command("cpu", "echo 0.75"),
            command("wind", "printf '3 4 5'"),
            command("unused", "echo 9"),
        ];
        let values = read_once(&inputs, &commands);
        assert_eq!(values[0], [0.75, 0.5, 0.5, 0.5]);
        assert_eq!(values[1], [3.0, 4.0, 0.5, 0.5], "extra numbers are ignored");
        assert_eq!(values[2], [0.5; 4], "no command keeps the default");
    }

    #[test]
    fn failing_commands_keep_the_value() {
        let inputs = [input("a", 1), input("b", 1)];
        let commands = [command("a", "exit 3"), command("b", "echo none")];
        let values = read_once(&inputs, &commands);
        assert_eq!(values[0], [0.5; 4]);
        assert_eq!(values[1], [0.5; 4]);
    }

    #[test]
    fn slow_commands_stop_with_their_children() {
        let start = Instant::now();
        let err = run("sleep 5 | cat; echo 1", Duration::from_millis(200)).unwrap_err();
        assert!(err.to_string().contains("stopped"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_full_error_pipe_does_not_block_the_output() {
        // 200 KB of errors is more than a pipe holds, and stdout stays open
        // until the command ends.
        let numbers = run(
            "head -c 200000 /dev/zero >&2; echo 1",
            Duration::from_secs(5),
        );
        assert_eq!(numbers.unwrap(), [1.0]);
    }

    #[test]
    fn programs_left_running_must_close_the_output() {
        let err = run("sleep 5 & echo 1", Duration::from_millis(200)).unwrap_err();
        assert!(err.to_string().contains("keeps its output open"), "{err}");
        let quiet = run(
            "sleep 5 > /dev/null 2>&1 & echo 1",
            Duration::from_millis(200),
        );
        assert_eq!(quiet.unwrap(), [1.0]);
    }

    #[test]
    fn threads_update_the_values() {
        let inputs = [input("n", 1)];
        let inputs = CommandInputs::start(&inputs, &[command("n", "echo 7")]);
        let deadline = Instant::now() + Duration::from_secs(2);
        while inputs.values()[0][0] != 7.0 {
            assert!(
                Instant::now() < deadline,
                "the thread did not run the command"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
