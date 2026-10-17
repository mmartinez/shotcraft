//! `shotcraft mcp`: Model Context Protocol over stdin/stdout. Thin: decode, run the engine op on the
//! blocking pool, encode. Never writes to stdout itself. Bad arguments and failures are tool errors the
//! model can read and fix; only an unknown tool name is a protocol error (rmcp's router answers that).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::handler::server::common::schema_for_type;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router, transport::stdio};
use serde::Serialize;
use serde::de::DeserializeOwned;
use shotcraft_engine::ops::{self, View};
use shotcraft_engine::{Error, history, share};

#[derive(Clone)]
pub struct Server;

#[tool_router]
impl Server {
    #[tool(
        description = "List the visible windows of other apps, topmost first, with the ids capture_window takes.",
        annotations(title = "List windows", read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_windows(&self) -> CallToolResult {
        reply(blocking(|| Ok(ops::list_windows())).await)
    }

    #[tool(
        description = "Capture a display, or an area of it, into ShotCraft history. Returns the history id, the full-resolution size and a downscaled preview image. Shows nothing to the user.",
        input_schema = schema_for_type::<ops::CaptureScreen>(),
        annotations(title = "Capture screen", read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn capture_screen(&self, Parameters(args): Parameters<JsonObject>) -> CallToolResult {
        shot(call(args, ops::capture_screen).await)
    }

    #[tool(
        description = "Capture one window, by its list_windows id, into ShotCraft history. Returns the history id, the full-resolution size and a downscaled preview image. Shows nothing to the user.",
        input_schema = schema_for_type::<ops::CaptureWindow>(),
        annotations(title = "Capture window", read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn capture_window(&self, Parameters(args): Parameters<JsonObject>) -> CallToolResult {
        shot(call(args, ops::capture_window).await)
    }

    #[tool(
        description = "List captures in ShotCraft history, newest first, including the ones the user took.",
        input_schema = schema_for_type::<ops::ListCaptures>(),
        annotations(title = "List captures", read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_captures(&self, Parameters(args): Parameters<JsonObject>) -> CallToolResult {
        reply(call(args, |req| Ok(ops::list_captures(req))).await)
    }

    #[tool(
        description = "Return a stored capture as a downscaled preview image. With area, zoom into that region, at full resolution when it is small enough.",
        input_schema = schema_for_type::<ops::GetCapture>(),
        annotations(title = "Get capture", read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn get_capture(&self, Parameters(args): Parameters<JsonObject>) -> CallToolResult {
        shot(call(args, ops::get_capture).await)
    }

    #[tool(
        description = "Read the text in a stored capture: QR code contents if there are any, else OCR. Returns an empty string when there is no text. The first call downloads the pinned, checksummed OCR models (about 12 MB).",
        input_schema = schema_for_type::<ops::ReadText>(),
        annotations(title = "Read text", read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn read_text(&self, Parameters(args): Parameters<JsonObject>) -> CallToolResult {
        reply(call(args, ops::read_text).await)
    }

    #[tool(
        description = "Save a stored capture as a new file in the user's screenshot folder and return its path. Never overwrites a file.",
        input_schema = schema_for_type::<ops::CaptureRef>(),
        annotations(title = "Save capture", read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn save_capture(&self, Parameters(args): Parameters<JsonObject>) -> CallToolResult {
        reply(call(args, ops::save_capture).await)
    }

    #[tool(
        description = "Copy a stored capture to the clipboard, replacing whatever the user had there.",
        input_schema = schema_for_type::<ops::CaptureRef>(),
        annotations(title = "Copy capture", read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn copy_capture(&self, Parameters(args): Parameters<JsonObject>) -> CallToolResult {
        reply(call(args, ops::copy_capture).await)
    }

    #[tool(
        description = "Upload a stored capture to the user's configured upload endpoint and return the share link. Sends the image and the configured credentials over the network.",
        input_schema = schema_for_type::<ops::CaptureRef>(),
        annotations(title = "Upload capture", read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    async fn upload_capture(&self, Parameters(args): Parameters<JsonObject>) -> CallToolResult {
        reply(call(args, ops::upload_capture).await)
    }

    #[tool(
        description = "Hand a stored capture to the user in a ShotCraft window: the editor, a pinned window or Quick Access.",
        input_schema = schema_for_type::<ops::ShowCapture>(),
        annotations(title = "Show capture", read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn show_capture(&self, Parameters(args): Parameters<JsonObject>) -> CallToolResult {
        let shown = call(args, |req: ops::ShowCapture| {
            history::find(&req.id)?;
            share::open(show_args(req.view, req.id.as_str()))?;
            Ok(match req.view {
                View::Editor => "Opened in the editor.",
                View::Pin => "Pinned above all windows.",
                View::QuickAccess => "Opened in Quick Access.",
            })
        });
        match shown.await {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(msg) => CallToolResult::error(vec![ContentBlock::text(msg)]),
        }
    }
}

#[tool_handler(
    name = "shotcraft",
    instructions = "Captures land in ShotCraft history; pass the returned id to the other tools. Area coordinates are full-resolution pixels of the captured image. Nothing appears on the user's screen or clipboard unless you call copy_capture or show_capture."
)]
impl ServerHandler for Server {}

/// Serve until the client closes stdin.
pub fn run() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        Server.serve(stdio()).await?.waiting().await?;
        Ok(())
    })
}

/// Decode the arguments into the op's request, then run it like `blocking`.
async fn call<P, T>(args: JsonObject, op: impl FnOnce(P) -> shotcraft_engine::Result<T> + Send + 'static) -> Result<T, String>
where
    P: DeserializeOwned + Send + 'static,
    T: Send + 'static,
{
    let req = serde_json::from_value(serde_json::Value::Object(args)).map_err(|e| format!("invalid arguments: {e}"))?;
    blocking(move || op(req)).await
}

/// Run an engine op off the async thread, so the server keeps answering. A panic arrives as a `JoinError`
/// and becomes a message like an engine error, leaving the server running.
async fn blocking<T: Send + 'static>(op: impl FnOnce() -> shotcraft_engine::Result<T> + Send + 'static) -> Result<T, String> {
    match tokio::task::spawn_blocking(op).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err @ Error::ScreenRecording(_))) => Err(format!("{err:#} The permission belongs to the app that started `shotcraft mcp`.")),
        Ok(Err(err)) => Err(format!("{err:#}")),
        Err(join) => Err(match join.try_into_panic() {
            Ok(panic) => format!("internal error: {}", crate::panic_message(&*panic)),
            Err(join) => format!("internal error: {join}"),
        }),
    }
}

fn reply<T: Serialize>(result: Result<T, String>) -> CallToolResult {
    match result.and_then(|value| serde_json::to_value(value).map_err(|e| format!("internal error: {e}"))) {
        Ok(json) => CallToolResult::structured(json),
        Err(msg) => CallToolResult::error(vec![ContentBlock::text(msg)]),
    }
}

/// `reply` plus the preview image.
fn shot(result: Result<ops::Shot, String>) -> CallToolResult {
    let (captured, preview) = match result {
        Ok(shot) => (Ok(shot.captured), Some(shot.preview)),
        Err(msg) => (Err(msg), None),
    };
    let mut reply = reply(captured);
    if let Some(preview) = preview.filter(|_| reply.is_error != Some(true)) {
        reply.content.push(ContentBlock::image(BASE64.encode(preview.bytes), preview.mime));
    }
    reply
}

fn show_args(view: View, id: &str) -> [&str; 3] {
    let mode = match view {
        View::Editor => "open-annotate",
        View::Pin => "pin",
        View::QuickAccess => "add-quick-access-overlay",
    };
    [mode, "--history", id]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cmd;
    use crate::tests::parse;

    #[test]
    fn show_args_round_trip() {
        let id = "20261017-101502117-04211";
        assert!(matches!(parse(show_args(View::Editor, id)), Some(Cmd::OpenAnnotate { filepath: None, history: Some(h) }) if h == id));
        assert!(matches!(parse(show_args(View::Pin, id)), Some(Cmd::Pin { filepath: None, history: Some(h) }) if h == id));
        assert!(matches!(parse(show_args(View::QuickAccess, id)), Some(Cmd::AddQuickAccessOverlay { filepath: None, history: Some(h) }) if h == id));
    }

    #[test]
    fn a_capture_carries_its_preview_and_a_failure_does_not() {
        let entry = history::Entry { id: "x".into(), created: 0, kind: "area".into(), width: 1, height: 1 };
        let captured = ops::Captured { entry, shown_width: 1, shown_height: 1, path: "x.png".into(), display: None, displays: None };
        let preview = ops::Preview { mime: "image/png", bytes: vec![1, 2, 3], width: 1, height: 1 };
        let ok = serde_json::to_value(shot(Ok(ops::Shot { captured, preview }))).unwrap();
        assert_eq!(ok["isError"], false);
        assert_eq!(ok["structuredContent"]["id"], "x");
        assert_eq!(ok["content"][1], serde_json::json!({"type": "image", "data": "AQID", "mimeType": "image/png"}));
        let failed = serde_json::to_value(shot(Err("no display".into()))).unwrap();
        assert_eq!((failed["isError"].clone(), failed["content"].as_array().map(Vec::len)), (serde_json::json!(true), Some(1)));
    }
}
