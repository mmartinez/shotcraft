//! The host-visible contract of `shotcraft mcp`: what hosts store (tool and parameter names) and how
//! bad arguments come back (as tool results the model can read, not JSON-RPC errors).

use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(20);

fn send(stdin: &mut ChildStdin, msg: Value) {
    writeln!(stdin, "{msg}").unwrap();
    stdin.flush().unwrap();
}

fn request(stdin: &mut ChildStdin, lines: &Receiver<String>, id: u64, method: &str, params: Value) -> Value {
    send(stdin, json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    loop {
        let line = lines.recv_timeout(TIMEOUT).unwrap_or_else(|e| panic!("no reply to {method}: {e}"));
        let msg: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("stdout carried a non-JSON line {line:?}: {e}"));
        if msg["id"] == id {
            assert!(msg.get("error").is_none(), "{method} failed at the protocol level: {msg}");
            return msg["result"].clone();
        }
    }
}

#[test]
fn hosts_see_a_stable_contract() {
    let dir = std::env::temp_dir().join(format!("shotcraft-mcp-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("shotcraft")).unwrap();
    // Nothing should be saved, but a regression must not write into the real screenshots folder.
    std::fs::write(dir.join("shotcraft/settings.json"), json!({"save_dir": dir}).to_string()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_shotcraft"))
        .arg("mcp")
        .env("XDG_CONFIG_HOME", &dir)
        .env("XDG_DATA_HOME", &dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || BufReader::new(stdout).lines().map_while(Result::ok).try_for_each(|line| tx.send(line)));

    let init = json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "shotcraft-test", "version": "0"}});
    let server = request(&mut stdin, &lines, 1, "initialize", init)["serverInfo"].clone();
    assert_eq!((server["name"].as_str(), server["version"].as_str()), (Some("shotcraft"), Some(env!("CARGO_PKG_VERSION"))));
    send(&mut stdin, json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let tools = request(&mut stdin, &lines, 2, "tools/list", json!({}));
    let mut contract: Vec<(String, Vec<String>)> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            let mut params: Vec<String> = tool["inputSchema"]["properties"].as_object().unwrap().keys().cloned().collect();
            params.sort();
            (tool["name"].as_str().unwrap().to_owned(), params)
        })
        .collect();
    contract.sort();
    let expected: [(&str, &[&str]); 10] = [
        ("capture_screen", &["area", "display"]),
        ("capture_window", &["window_id"]),
        ("copy_capture", &["id"]),
        ("get_capture", &["area", "id"]),
        ("list_captures", &["limit"]),
        ("list_windows", &[]),
        ("read_text", &["id", "linebreaks"]),
        ("save_capture", &["id"]),
        ("show_capture", &["id", "view"]),
        ("upload_capture", &["id"]),
    ];
    let expected: Vec<(String, Vec<String>)> =
        expected.iter().map(|(name, params)| (name.to_string(), params.iter().map(|p| p.to_string()).collect())).collect();
    assert_eq!(contract, expected, "a rename breaks the names hosts store and the approvals users granted");

    let bad_id = request(&mut stdin, &lines, 3, "tools/call", json!({"name": "get_capture", "arguments": {"id": "../x"}}));
    assert_eq!(bad_id["isError"], true, "{bad_id}");
    let bad_type = request(&mut stdin, &lines, 4, "tools/call", json!({"name": "capture_screen", "arguments": {"display": "one"}}));
    assert_eq!(bad_type["isError"], true, "{bad_type}");

    for (id, tool) in [(5, "list_captures"), (6, "list_windows")] {
        let listed = request(&mut stdin, &lines, id, "tools/call", json!({"name": tool, "arguments": {}}));
        let key = tool.trim_start_matches("list_");
        assert!(listed["isError"] == false && listed["structuredContent"][key].is_array(), "{listed}");
    }
    let unknown = "19700101-000000000-00000";
    let calls = [
        ("get_capture", json!({"id": unknown})),
        ("read_text", json!({"id": unknown})),
        ("save_capture", json!({"id": unknown})),
        ("copy_capture", json!({"id": unknown})),
        ("upload_capture", json!({"id": unknown})),
        ("show_capture", json!({"id": unknown, "view": "editor"})),
    ];
    for (id, (tool, args)) in (7..).zip(calls) {
        let reply = request(&mut stdin, &lines, id, "tools/call", json!({"name": tool, "arguments": args}));
        let text = reply["content"][0]["text"].as_str().unwrap_or_default();
        assert!(reply["isError"] == true && text.contains(unknown), "{tool} must name the id it refused: {reply}");
    }
    let gone = request(&mut stdin, &lines, 20, "tools/call", json!({"name": "capture_window", "arguments": {"window_id": 0}}));
    assert_eq!(gone["isError"], true, "{gone}");

    drop(stdin);
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().ok();
            panic!("the server did not exit after stdin closed");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    std::fs::remove_dir_all(&dir).ok();
    assert!(status.success(), "{status}");
}
