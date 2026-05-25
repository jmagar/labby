use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn read_protocol_line(reader: &mut BufReader<impl Read>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read runner output");
    assert!(!line.is_empty(), "runner closed stdout");
    serde_json::from_str(&line).expect("runner output must be JSON")
}

#[test]
fn code_mode_runner_evaluates_js_in_a_minimal_host_environment() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stderr = child.stderr.take().expect("runner stderr");
    let mut stdout = BufReader::new(stdout);
    let code = r#"
        if (typeof process !== "undefined" || typeof require !== "undefined" ||
            typeof fetch !== "undefined" || typeof Deno !== "undefined" ||
            typeof Bun !== "undefined") {
          throw new Error("ambient host API exposed");
        }
        console.log("runner console check");
        const first = await callTool("lab::gateway.first", {"x": 1});
        if (first.ok) {
          await callTool("lab::gateway.second", {"from": first.value});
        }
        if (false) {
          await callTool("lab::gateway.never", {});
        }
    "#;

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "start",
            "code": code
        })
    )
    .expect("write start");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 0,
            "id": "lab::gateway.first",
            "params": {"x": 1}
        })
    );
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 0,
            "result": {"ok": true, "value": 42}
        })
    )
    .expect("write first result");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 1,
            "id": "lab::gateway.second",
            "params": {"from": 42}
        })
    );
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 1,
            "result": {"ok": true}
        })
    )
    .expect("write second result");

    assert_eq!(read_protocol_line(&mut stdout), json!({"type": "done"}));
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
    let mut stderr_text = String::new();
    stderr
        .read_to_string(&mut stderr_text)
        .expect("read runner stderr");
    assert!(stderr_text.contains("runner console check"));
}

#[test]
fn code_mode_runner_fans_out_promise_all_tool_calls() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);
    let code = r#"
        const [first, second] = await Promise.all([
          callTool("lab::gateway.first", {"x": 1}),
          callTool("lab::gateway.second", {"x": 2})
        ]);
        await callTool("lab::gateway.after", {"sum": first.value + second.value});
    "#;

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "start",
            "code": code
        })
    )
    .expect("write start");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 0,
            "id": "lab::gateway.first",
            "params": {"x": 1}
        })
    );
    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 1,
            "id": "lab::gateway.second",
            "params": {"x": 2}
        })
    );

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 1,
            "result": {"value": 20}
        })
    )
    .expect("write second result");
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 0,
            "result": {"value": 10}
        })
    )
    .expect("write first result");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 2,
            "id": "lab::gateway.after",
            "params": {"sum": 30}
        })
    );
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 2,
            "result": {"ok": true}
        })
    )
    .expect("write after result");

    assert_eq!(read_protocol_line(&mut stdout), json!({"type": "done"}));
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}
