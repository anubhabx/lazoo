use std::{
    fs::OpenOptions,
    io::{self, BufRead as _, Write as _},
    path::PathBuf,
};

use serde_json::{Value, json};

fn main() {
    if std::env::args_os().any(|argument| argument == "--version") {
        println!("codex-cli 0.0.0-lazoo-turn-transport-fixture");
        return;
    }
    if let Err(error) = run_server() {
        eprintln!("turn transport fixture failed: {error}");
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_lines)] // One small loop mirrors the fixture's finite wire protocol.
fn run_server() -> Result<(), Box<dyn std::error::Error>> {
    let thread_id = std::env::var("LAZOO_TURN_FIXTURE_THREAD_ID")
        .unwrap_or_else(|_| format!("fixture-thread-{}", std::process::id()));
    let log_path = std::env::var_os("LAZOO_TURN_FIXTURE_LOG").map(PathBuf::from);
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut turn_number = 0_u64;
    let mut retry_omitted = false;
    let mut active_turn: Option<String> = None;
    append_log(log_path.as_ref(), "server-start")?;

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(&line)?;
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = message.get("id");
        append_log(log_path.as_ref(), method)?;
        match method {
            "initialize" => write_response(
                &mut stdout,
                id,
                &json!({"userAgent": "lazoo-turn-transport-fixture"}),
            )?,
            "initialized" => {}
            "account/read" => write_response(
                &mut stdout,
                id,
                &json!({
                    "account": {"type": "fixture"},
                    "requiresOpenaiAuth": true
                }),
            )?,
            "thread/start" | "thread/resume" => {
                write_response(&mut stdout, id, &json!({"thread": {"id": thread_id}}))?;
            }
            "turn/start" => {
                let prompt = message
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if prompt.contains("RETRY_ONCE") && !retry_omitted {
                    retry_omitted = true;
                    append_log(log_path.as_ref(), "retry-response-omitted")?;
                    continue;
                }
                turn_number += 1;
                let turn_id = format!("fixture-turn-{turn_number}");
                active_turn = Some(turn_id.clone());
                write_response(&mut stdout, id, &json!({"turn": {"id": turn_id}}))?;
                write_notification(
                    &mut stdout,
                    "thread/tokenUsage/updated",
                    &json!({
                        "threadId": thread_id,
                        "tokenUsage": {
                            "total": {"inputTokens": 10, "outputTokens": 2, "totalTokens": 12}
                        }
                    }),
                )?;
                if prompt.contains("HOLD_FOR_RESTART") || prompt.contains("WAIT_FOR_CANCEL") {
                    continue;
                }
                if prompt.contains("FAIL_AFTER_ACCEPTANCE") {
                    write_notification(
                        &mut stdout,
                        "error",
                        &json!({
                            "threadId": thread_id,
                            "turnId": active_turn,
                            "willRetry": false,
                            "error": {
                                "message": "fixture terminal failure",
                                "codexErrorInfo": "other"
                            }
                        }),
                    )?;
                    write_notification(
                        &mut stdout,
                        "turn/completed",
                        &json!({
                            "threadId": thread_id,
                            "turn": {"id": active_turn, "status": "failed"}
                        }),
                    )?;
                    active_turn = None;
                    continue;
                }
                let output = if prompt.contains("LARGE_TRANSCRIPT") {
                    "x".repeat(70 * 1024)
                } else {
                    "FIXTURE_TURN_OK".to_owned()
                };
                write_notification(
                    &mut stdout,
                    "item/agentMessage/delta",
                    &json!({
                        "threadId": thread_id,
                        "turnId": active_turn,
                        "itemId": format!("fixture-item-{turn_number}"),
                        "delta": output
                    }),
                )?;
                write_notification(
                    &mut stdout,
                    "turn/completed",
                    &json!({
                        "threadId": thread_id,
                        "turn": {"id": active_turn, "status": "completed"}
                    }),
                )?;
                active_turn = None;
            }
            "turn/interrupt" => {
                let interrupted = message
                    .pointer("/params/turnId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| active_turn.clone())
                    .unwrap_or_else(|| "fixture-turn-unknown".to_owned());
                write_response(&mut stdout, id, &json!({}))?;
                write_notification(
                    &mut stdout,
                    "turn/completed",
                    &json!({
                        "threadId": thread_id,
                        "turn": {"id": interrupted, "status": "interrupted"}
                    }),
                )?;
                active_turn = None;
            }
            _ if id.is_some() => write_error(&mut stdout, id, -32601)?,
            _ => {}
        }
    }
    append_log(log_path.as_ref(), "server-stop")?;
    Ok(())
}

fn write_response(
    output: &mut impl io::Write,
    id: Option<&Value>,
    result: &Value,
) -> io::Result<()> {
    write_message(output, &json!({"id": id, "result": result}))
}

fn write_error(output: &mut impl io::Write, id: Option<&Value>, code: i64) -> io::Result<()> {
    write_message(
        output,
        &json!({"id": id, "error": {"code": code, "message": "fixture method not found"}}),
    )
}

fn write_notification(output: &mut impl io::Write, method: &str, params: &Value) -> io::Result<()> {
    write_message(output, &json!({"method": method, "params": params}))
}

fn write_message(output: &mut impl io::Write, message: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn append_log(path: Option<&PathBuf>, entry: &str) -> io::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{entry}")?;
    file.flush()
}
