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
    let stored = "20261017-101502117-04211";
    std::fs::create_dir_all(dir.join("history").join(stored)).unwrap();
    std::fs::write(
        dir.join("history").join(stored).join("meta.json"),
        json!({"id": stored, "created": 0, "kind": "area", "width": 2, "height": 1}).to_string(),
    )
    .unwrap();
    image::RgbaImage::new(2, 1).save(dir.join("history").join(stored).join("capture.png")).unwrap();
    std::fs::write(dir.join("settings.json"), json!({"save_dir": dir}).to_string()).unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_shotcraft"));
    // No display: a window the server launches must fail, not open on the machine running the tests.
    server.arg("mcp").env("SHOTCRAFT_HOME", &dir).env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
    let mut child = server.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
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

    let listed = request(&mut stdin, &lines, 5, "tools/call", json!({"name": "list_captures", "arguments": {}}));
    assert_eq!(listed["structuredContent"]["captures"][0]["id"], stored, "the history in SHOTCRAFT_HOME, and only it: {listed}");
    assert_eq!(listed["structuredContent"]["captures"].as_array().map(Vec::len), Some(1), "{listed}");
    let windows = request(&mut stdin, &lines, 6, "tools/call", json!({"name": "list_windows", "arguments": {}}));
    assert!(windows["isError"] == false && windows["structuredContent"]["windows"].is_array(), "{windows}");
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

    let got = request(&mut stdin, &lines, 21, "tools/call", json!({"name": "get_capture", "arguments": {"id": stored}}));
    assert!(got["structuredContent"]["id"] == stored && got["content"][1]["type"] == "image", "{got}");
    let saved = request(&mut stdin, &lines, 22, "tools/call", json!({"name": "save_capture", "arguments": {"id": stored}}));
    let path = saved["structuredContent"]["path"].as_str().map(std::path::PathBuf::from).unwrap_or_default();
    assert!(path.starts_with(&dir) && path.is_file(), "saves go to the configured folder: {saved}");
    if cfg!(target_os = "linux") {
        let shown = request(&mut stdin, &lines, 23, "tools/call", json!({"name": "show_capture", "arguments": {"id": stored, "view": "pin"}}));
        assert_eq!(shown["isError"], true, "a window that cannot open is not reported as shown: {shown}");
        let copied = request(&mut stdin, &lines, 24, "tools/call", json!({"name": "copy_capture", "arguments": {"id": stored}}));
        assert_eq!(copied["isError"], true, "a clipboard that cannot be reached is not reported as copied: {copied}");
    }

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
