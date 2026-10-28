//! Presentation of Codex's JSONL stream. Tool output is diagnostic, not progress.

use serde_json::Value;
use std::io::{self, Write};

const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

pub(crate) struct Renderer<W: Write, E: Write> {
    stdout: W,
    stderr: E,
    pending: Option<String>,
    completed: bool,
    failed: bool,
}

fn protocol_error(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn string<'a>(value: &'a Value, field: &str) -> io::Result<&'a str> {
    value[field]
        .as_str()
        .ok_or_else(|| protocol_error(format!("Codex event is missing string field {field}")))
}

impl<W: Write, E: Write> Renderer<W, E> {
    pub(crate) fn new(stdout: W, stderr: E) -> Self {
        Self {
            stdout,
            stderr,
            pending: None,
            completed: false,
            failed: false,
        }
    }

    fn progress(&mut self) -> io::Result<()> {
        if let Some(message) = self.pending.take() {
            writeln!(self.stderr, "{message}")?;
            self.stderr.flush()?;
        }
        Ok(())
    }

    /// Consume one complete JSONL record. Invalid known records fail explicitly.
    pub(crate) fn event(&mut self, line: &str) -> io::Result<()> {
        let event: Value = serde_json::from_str(line)
            .map_err(|error| protocol_error(format!("invalid Codex JSON event: {error}")))?;
        match string(&event, "type")? {
            "thread.started" => {
                writeln!(self.stderr, "Session: {}", string(&event, "thread_id")?)?;
            }
            "turn.started" => {
                self.completed = false;
                writeln!(self.stderr, "Codex is working (Ctrl-C to interrupt).")?;
            }
            "turn.completed" => self.completed = true,
            "turn.failed" => {
                self.progress()?;
                self.failed = true;
                writeln!(
                    self.stderr,
                    "Codex failed: {}",
                    string(&event["error"], "message")?
                )?;
            }
            "error" => {
                self.progress()?;
                // Diagnostics can precede a recovered turn. Completion and the
                // process status determine success; turn.failed remains decisive.
                writeln!(
                    self.stderr,
                    "Codex diagnostic: {}",
                    string(&event, "message")?
                )?;
            }
            "item.started" | "item.updated" | "item.completed" => {
                let item = &event["item"];
                let kind = string(item, "type")?;
                let completed = event["type"] == "item.completed";
                match kind {
                    "agent_message" if completed => {
                        let message = string(item, "text")?;
                        if message.len() > MAX_MESSAGE_BYTES {
                            return Err(protocol_error("Codex message exceeds 1 MiB"));
                        }
                        self.progress()?;
                        self.pending = Some(message.to_owned());
                    }
                    "error" if completed => {
                        self.progress()?;
                        writeln!(
                            self.stderr,
                            "Codex diagnostic: {}",
                            string(item, "message")?
                        )?;
                    }
                    "command_execution" => {
                        let command = string(item, "command")?;
                        let status = string(item, "status")?;
                        if !item["exit_code"].is_null() && !item["exit_code"].is_i64() {
                            return Err(protocol_error(
                                "Codex command exit_code must be an integer or null",
                            ));
                        }
                        if event["type"] == "item.started" {
                            self.progress()?;
                        }
                        if completed
                            && (status == "failed"
                                || item["exit_code"].as_i64().is_some_and(|code| code != 0))
                        {
                            self.progress()?;
                            writeln!(self.stderr, "Command failed: {command}")?;
                            let output = string(item, "aggregated_output")?;
                            write!(self.stderr, "{output}")?;
                            if !output.ends_with('\n') {
                                writeln!(self.stderr)?;
                            }
                        }
                    }
                    _ => {
                        if event["type"] == "item.started"
                            && matches!(
                                kind,
                                "mcp_tool_call" | "web_search" | "collab_tool_call" | "file_change"
                            )
                        {
                            self.progress()?;
                        }
                        if completed && item["status"] == "failed" {
                            self.progress()?;
                            // Preserve a diagnostic for newer item schemas too.
                            writeln!(self.stderr, "Codex {kind} failed: {item}")?;
                        }
                    }
                }
            }
            // New event types must not break an otherwise supported stream.
            _ => {}
        }
        self.stderr.flush()
    }

    /// Returns false for a failed or truncated turn, even if Codex exited zero.
    pub(crate) fn finish(mut self, child_success: bool) -> io::Result<bool> {
        if !child_success || !self.completed || self.failed {
            self.progress()?;
            if !self.completed && !self.failed {
                writeln!(
                    self.stderr,
                    "Codex event stream ended before turn.completed; inspect the session or rerun with --verbose."
                )?;
            }
            self.stderr.flush()?;
            return Ok(false);
        }
        if let Some(message) = self.pending.take() {
            writeln!(self.stdout, "{message}")?;
        }
        self.stdout.flush()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render(events: &[Value]) -> (bool, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut renderer = Renderer::new(&mut out, &mut err);
        for event in events {
            renderer.event(&event.to_string()).unwrap();
        }
        let success = renderer.finish(true).unwrap();
        (
            success,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn message(text: &str) -> Value {
        json!({"type":"item.completed","item":{"type":"agent_message","text":text}})
    }

    #[test]
    fn progress_streams_before_exit_and_final_is_stdout_only() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        {
            let mut renderer = Renderer::new(&mut out, &mut err);
            renderer
                .event(&message("Inspecting changes").to_string())
                .unwrap();
            renderer.event(&json!({"type":"item.started","item":{"type":"command_execution","command":"git diff","status":"in_progress"}}).to_string()).unwrap();
            assert!(String::from_utf8_lossy(renderer.stderr).contains("Inspecting changes"));
            renderer
                .event(&message("Committed abc123").to_string())
                .unwrap();
            renderer.event(r#"{"type":"turn.completed"}"#).unwrap();
            assert!(renderer.finish(true).unwrap());
        }
        assert_eq!(String::from_utf8(out).unwrap(), "Committed abc123\n");
        assert_eq!(String::from_utf8(err).unwrap(), "Inspecting changes\n");
    }

    #[test]
    fn turn_start_shows_activity_before_any_message_or_tool() {
        let mut renderer = Renderer::new(Vec::new(), Vec::new());
        renderer.event(r#"{"type":"turn.started"}"#).unwrap();
        assert_eq!(
            renderer.stderr,
            b"Codex is working (Ctrl-C to interrupt).\n"
        );
        assert!(renderer.stdout.is_empty());
    }

    #[test]
    fn successful_tool_output_and_reasoning_are_hidden() {
        let (success, out, err) = render(&[
            json!({"type":"thread.started","thread_id":"session-1"}),
            json!({"type":"item.completed","item":{"type":"reasoning","text":"private"}}),
            json!({"type":"item.completed","item":{"type":"command_execution","command":"git diff","status":"completed","exit_code":0,"aggregated_output":"huge diff"}}),
            message("Done"),
            json!({"type":"turn.completed"}),
        ]);
        assert!(success);
        assert_eq!(out, "Done\n");
        assert_eq!(err, "Session: session-1\n");
    }

    #[test]
    fn failed_commands_retain_exact_diagnostics_even_when_turn_recovers() {
        let (success, _, err) = render(&[
            json!({"type":"item.completed","item":{"type":"command_execution","command":"git push","status":"completed","exit_code":128,"aggregated_output":"fatal: authentication failed\n"}}),
            message("Push failed"),
            json!({"type":"turn.completed"}),
        ]);
        assert!(success);
        assert_eq!(
            err,
            "Command failed: git push\nfatal: authentication failed\n"
        );
    }

    #[test]
    fn failures_are_visible_and_failed_turn_cannot_publish_success() {
        let (success, out, err) = render(&[
            message("Working"),
            json!({"type":"item.completed","item":{"type":"mcp_tool_call","status":"failed","error":{"message":"tool denied"}}}),
            json!({"type":"turn.failed","error":{"message":"model unavailable"}}),
        ]);
        assert!(!success);
        assert!(out.is_empty());
        assert!(err.contains("tool denied"));
        assert!(err.contains("model unavailable"));
    }

    #[test]
    fn malformed_or_invalid_known_events_are_errors() {
        for line in [
            "not json",
            "{}",
            r#"{"type":"item.completed","item":{"type":"agent_message"}}"#,
            r#"{"type":"thread.started"}"#,
        ] {
            let mut renderer = Renderer::new(Vec::new(), Vec::new());
            assert_eq!(
                renderer.event(line).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn unknown_events_are_ignored_but_truncation_is_detected() {
        let (success, out, err) = render(&[
            json!({"type":"future.event"}),
            json!({"type":"item.completed","item":{"type":"future_item"}}),
            message("Still working"),
        ]);
        assert!(!success);
        assert!(out.is_empty());
        assert!(err.contains("before turn.completed"));
    }

    #[test]
    fn diagnostic_errors_can_recover_when_turn_completes() {
        let (success, out, err) = render(&[
            json!({"type":"error","message":"retrying transport"}),
            json!({"type":"item.completed","item":{"type":"error","message":"experimental feature warning"}}),
            message("Done"),
            json!({"type":"turn.completed"}),
        ]);
        assert!(success);
        assert_eq!(out, "Done\n");
        assert_eq!(
            err,
            "Codex diagnostic: retrying transport\nCodex diagnostic: experimental feature warning\n"
        );
    }

    #[test]
    fn chronicle_warning_remains_diagnostic_when_turn_succeeds_or_fails() {
        let warning = "Under-development features enabled: chronicle. Under-development features are incomplete and may behave unpredictably. To suppress this warning, set `suppress_unstable_features_warning = true` in /home/user/.codex/config.toml.";
        for diagnostic in [
            json!({"type":"error","message":warning}),
            json!({"type":"item.completed","item":{"type":"error","message":warning}}),
        ] {
            let (success, out, err) = render(&[
                diagnostic.clone(),
                message("Done"),
                json!({"type":"turn.completed"}),
            ]);
            assert!(success);
            assert_eq!(out, "Done\n");
            assert_eq!(err, format!("Codex diagnostic: {warning}\n"));

            let (success, out, err) = render(&[
                diagnostic,
                json!({"type":"turn.failed","error":{"message":"model unavailable"}}),
            ]);
            assert!(!success);
            assert!(out.is_empty());
            assert_eq!(
                err,
                format!("Codex diagnostic: {warning}\nCodex failed: model unavailable\n")
            );
        }
    }

    #[test]
    fn error_without_completion_is_failure() {
        let (success, out, err) =
            render(&[json!({"type":"error","message":"authentication failed"})]);
        assert!(!success);
        assert!(out.is_empty());
        assert!(err.contains("authentication failed"));
    }

    #[test]
    fn nonzero_process_flushes_pending_explanation_to_stderr() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut renderer = Renderer::new(&mut out, &mut err);
        renderer
            .event(&message("Push was rejected").to_string())
            .unwrap();
        renderer.event(r#"{"type":"turn.completed"}"#).unwrap();
        assert!(!renderer.finish(false).unwrap());
        assert!(out.is_empty());
        assert_eq!(String::from_utf8(err).unwrap(), "Push was rejected\n");
    }
}
