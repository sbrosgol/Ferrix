//! Running converter processes with a deadline.

use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::ConvertError;

pub(crate) struct Finished {
    pub(crate) success: bool,
    /// Combined stdout and stderr, for error messages.
    pub(crate) output: String,
}

/// Runs `command`, killing it if it outlives `timeout`.
pub(crate) fn run(
    mut command: Command,
    tool: &'static str,
    timeout: Duration,
) -> Result<Finished, ConvertError> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ConvertError::Failed {
            tool,
            message: format!("could not start it: {e}"),
        })?;

    // Drain both pipes on threads so a chatty process can never block on a full buffer.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as _));

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ConvertError::TimedOut(tool));
        }
        thread::sleep(Duration::from_millis(50));
    };
    let output = [stdout, stderr]
        .into_iter()
        .filter_map(|t| t.join().ok())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Finished {
        success: status.success(),
        output,
    })
}

/// The last meaningful lines of a tool's output, for an error message.
pub(crate) fn summary(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Fontconfig"))
        .collect();
    let tail = &lines[lines.len().saturating_sub(3)..];
    if tail.is_empty() {
        "no output was produced".into()
    } else {
        tail.join(" ")
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn kills_processes_that_overrun() {
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = Instant::now();
        let result = run(command, "sleep", Duration::from_millis(200));
        assert!(matches!(result, Err(ConvertError::TimedOut("sleep"))));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn captures_output_and_exit_status() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo hello; echo oops >&2; exit 3"]);
        let finished = run(command, "sh", Duration::from_secs(5)).expect("runs");
        assert!(!finished.success);
        assert_eq!(summary(&finished.output), "hello oops");
    }
}
