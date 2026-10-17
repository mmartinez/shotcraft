//! Operations shared by every window: clipboard, saving, uploading and launching other modes.

use std::borrow::Cow;
use std::io::{BufRead, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::RgbaImage;

use crate::config::{ExportFormat, Settings, run_dir};
use crate::{Error, Result};

/// Carries a notification body to the `notify` helper. Other users can list command lines on most
/// systems, but not another user's environment, and a body can be recognized text or a share link.
pub const NOTIFY_BODY_ENV: &str = "SHOTCRAFT_NOTIFY_BODY";

fn this_app() -> Command {
    Command::new(std::env::current_exe().map(installed_path).unwrap_or_else(|_| PathBuf::from("shotcraft")))
}

/// Linux names a running binary that has since been replaced (an update, a rebuild) `<path> (deleted)`;
/// launch the file now at that path rather than fail until the daemon restarts.
fn installed_path(exe: PathBuf) -> PathBuf {
    match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(path) if cfg!(target_os = "linux") => PathBuf::from(path),
        _ => exe,
    }
}

/// Start a child without waiting for it. It gets none of our standard streams: a child that outlives us
/// (the clipboard owner, a window) would otherwise hold the caller's pipes open, and a caller reading
/// them (an MCP host, `shotcraft … 2>&1 | cat`) would wait until that child exits.
fn launch(cmd: &mut Command) {
    match cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        // Reap the child so long-lived parents (the tray daemon) leave no zombies.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(err) => eprintln!("failed to launch {}: {err}", cmd.get_program().to_string_lossy()),
    }
}

/// Launch another mode of this app (`shotcraft <args>`) without waiting for it.
pub fn spawn<I, S>(args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    launch(this_app().args(args));
}

/// A launched mode whose failure we report. Its stderr goes to a file in the run folder, not a pipe: a
/// child that outlives us would hold a caller's pipe open, and writing into one we closed would fail its `eprintln!`.
struct Watched {
    child: Child,
    log: PathBuf,
}

impl Watched {
    fn start(cmd: &mut Command, stdout: Stdio) -> Result<Self> {
        let dir = run_dir();
        std::fs::create_dir_all(&dir)?;
        let log = dir.join(format!("launch-{}-{}.log", std::process::id(), nanos()));
        let child = cmd.stdin(Stdio::null()).stdout(stdout).stderr(std::fs::File::create(&log)?).spawn()?;
        Ok(Self { child, log })
    }

    /// `what` failed, with the mode's error text (its stderr, else its exit status) once it has exited.
    fn failure(mut self, what: &str) -> Error {
        let status = self.child.wait().map(|s| s.to_string()).unwrap_or_default();
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let _ = std::fs::remove_file(&self.log);
        let text = text.trim();
        Error::Other(format!("{what}: {}", if text.is_empty() { &status } else { text }))
    }

    /// The mode started fine: let it run and reap it. Unix lets us unlink its log now; Windows only once it exits.
    fn detach(mut self) {
        let _ = std::fs::remove_file(&self.log);
        drop(std::thread::spawn(move || {
            let _ = self.child.wait();
            let _ = std::fs::remove_file(&self.log);
        }));
    }
}

/// How long a window mode gets to fail before it counts as open (40 polls 50 ms apart); failures such as
/// no display or a bad file come within milliseconds.
const OPEN_POLLS: u32 = 40;

/// Launch a window mode like `spawn`, but report a launch that fails within the first two seconds, with the
/// mode's error text.
pub fn open<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut mode = Watched::start(this_app().args(args), Stdio::null())?;
    for _ in 0..OPEN_POLLS {
        if let Some(status) = mode.child.try_wait()? {
            if !status.success() {
                return Err(mode.failure("the window did not open"));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    mode.detach();
    Ok(())
}

pub fn notify(title: &str, body: &str) {
    launch(this_app().args(["notify", "--title", title]).env(NOTIFY_BODY_ENV, body));
}

/// A notification for something that went wrong: warning icon instead of a check mark.
pub fn warn(title: &str, body: &str) {
    launch(this_app().args(["notify", "--warning", "--title", title]).env(NOTIFY_BODY_ENV, body));
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default()
}

fn temp_file(ext: &str) -> Result<PathBuf> {
    let dir = run_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("clip-{}-{}.{ext}", std::process::id(), nanos())))
}

/// On X11/Wayland the clipboard lives only as long as the owning process, so a detached
/// helper (`clipboard-serve`) holds it until another app takes over; we return once it owns it.
/// Elsewhere the OS keeps the data after we exit.
pub fn copy_image(img: &RgbaImage) -> Result<()> {
    if cfg!(target_os = "linux") {
        let path = temp_file("png")?;
        img.save(&path)?;
        return serve_from("--image", &path);
    }
    arboard::Clipboard::new()?.set_image(image_data(img))?;
    Ok(())
}

pub fn copy_text(text: &str) -> Result<()> {
    if cfg!(target_os = "linux") {
        let path = temp_file("txt")?;
        std::fs::write(&path, text)?;
        return serve_from("--text", &path);
    }
    arboard::Clipboard::new()?.set_text(text)?;
    Ok(())
}

fn image_data(img: &RgbaImage) -> arboard::ImageData<'_> {
    arboard::ImageData { width: img.width() as usize, height: img.height() as usize, bytes: Cow::Borrowed(img.as_raw()) }
}

/// The line `clipboard-serve` prints once it owns the clipboard.
const CLIPBOARD_READY: &str = "clipboard-ready";

/// Start `clipboard-serve` on `path` and wait until it owns the clipboard, or return its error.
fn serve_from(kind: &str, path: &Path) -> Result<()> {
    let mut helper = Watched::start(this_app().args(["clipboard-serve", kind]).arg(path), Stdio::piped())?;
    let mut line = String::new();
    if let Some(out) = helper.child.stdout.take() {
        // The ready line, or nothing when the helper exits first.
        let _ = std::io::BufReader::new(out).read_line(&mut line);
    }
    if line.trim_end() != CLIPBOARD_READY {
        let _ = std::fs::remove_file(path);
        return Err(helper.failure("the clipboard was not set"));
    }
    helper.detach();
    Ok(())
}

/// Body of the `clipboard-serve` helper: take the clipboard, tell `serve_from`, then own it until someone else does.
pub fn serve_clipboard(image: Option<&Path>, text: Option<&Path>) -> Result<()> {
    if let Some(path) = image {
        let img = image::open(path)?.into_rgba8();
        let _ = std::fs::remove_file(path);
        return hold(Clip::Image(img));
    }
    if let Some(path) = text {
        let s = std::fs::read_to_string(path)?;
        let _ = std::fs::remove_file(path);
        return hold(Clip::Text(s));
    }
    Err(Error::Other("nothing to serve".into()))
}

enum Clip {
    Image(RgbaImage),
    Text(String),
}

impl Clip {
    fn write(&self, set: arboard::Set<'_>) -> std::result::Result<(), arboard::Error> {
        match self {
            Clip::Image(img) => set.image(image_data(img)),
            Clip::Text(s) => set.text(s.as_str()),
        }
    }
}

/// How long the X server gets to show this helper as the clipboard owner (500 polls 10 ms apart); encoding a large image comes first.
#[cfg(target_os = "linux")]
const OWNER_POLLS: u32 = 500;

/// Write the clipboard once, waiting in a thread until another app takes it, and print the ready line once
/// this helper owns it. Writing again after the ready line could put this copy back over a newer one.
#[cfg(target_os = "linux")]
fn hold(clip: Clip) -> Result<()> {
    use arboard::SetExtLinux;
    use x11rb::protocol::xproto::ConnectionExt as _;
    let x11 = |e: &dyn std::fmt::Display| Error::Other(format!("clipboard: {e}"));
    let (conn, _) = x11rb::connect(None).map_err(|e| x11(&e))?;
    let selection = conn.intern_atom(false, b"CLIPBOARD").map_err(|e| x11(&e))?.reply().map_err(|e| x11(&e))?.atom;
    let owner = || conn.get_selection_owner(selection).ok().and_then(|cookie| cookie.reply().ok()).map(|reply| reply.owner);
    // arboard shares one context per process and answers `get` from memory while it owns the clipboard,
    // so a probe that reads back this helper's content proves the owner is this helper, not another app.
    let mut probe = arboard::Clipboard::new()?;
    let clip = std::sync::Arc::new(clip);
    let mut seen = owner();
    let writer = std::thread::spawn({
        let clip = std::sync::Arc::clone(&clip);
        move || -> Result<()> {
            let mut cb = arboard::Clipboard::new()?;
            clip.write(cb.set().wait())?;
            Ok(())
        }
    });
    let mut owned = false;
    for _ in 0..OWNER_POLLS {
        let now = owner();
        if now != seen {
            seen = now;
            owned = match &*clip {
                Clip::Image(img) => probe.get_image().is_ok_and(|got| *got.bytes == **img.as_raw()),
                Clip::Text(s) => probe.get_text().is_ok_and(|got| got == *s),
            };
        }
        if owned || writer.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if owned {
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{CLIPBOARD_READY}").and_then(|()| out.flush());
    } else if !writer.is_finished() {
        return Err(Error::Other("this helper never became the clipboard owner".into()));
    }
    writer.join().map_err(|_| Error::Other("the clipboard thread panicked".into()))?
}

/// Elsewhere the OS keeps the clipboard after we exit, so one write is all.
#[cfg(not(target_os = "linux"))]
fn hold(clip: Clip) -> Result<()> {
    clip.write(arboard::Clipboard::new()?.set())?;
    Ok(())
}

pub fn clipboard_image() -> Result<RgbaImage> {
    let data = arboard::Clipboard::new()?.get_image().map_err(|_| Error::Other("the clipboard has no image".into()))?;
    RgbaImage::from_raw(data.width as u32, data.height as u32, data.bytes.into_owned()).ok_or_else(|| Error::Other("bad clipboard image".into()))
}

/// macOS-style default name: "Screenshot 2026-10-15 at 18.25.31.png".
pub fn default_file_name(format: ExportFormat) -> String {
    format!("Screenshot {}.{}", chrono::Local::now().format("%Y-%m-%d at %H.%M.%S"), format.ext())
}

/// Create `path`, or "name (2).ext", "name (3).ext"… when taken. Creation is exclusive, so two
/// processes saving in the same second never write to the same file.
fn create_unique(path: PathBuf) -> Result<(PathBuf, std::fs::File)> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Screenshot").to_owned();
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("png").to_owned();
    let mut candidate = path.clone();
    for n in 2..10_000 {
        match std::fs::File::options().write(true).create_new(true).open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => candidate = path.with_file_name(format!("{stem} ({n}).{ext}")),
            Err(e) => return Err(Error::Other(format!("cannot write {}: {e}", candidate.display()))),
        }
    }
    Err(Error::Other(format!("no free file name next to {}", path.display())))
}

/// Encode for saving or uploading. JPEG has no alpha, so transparency is flattened on white.
pub fn encode(img: &RgbaImage, format: ExportFormat, jpeg_quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match format {
        ExportFormat::Png => img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)?,
        ExportFormat::Webp => img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::WebP)?,
        ExportFormat::Jpeg => {
            let rgb = image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
                let [r, g, b, a] = img.get_pixel(x, y).0;
                let mix = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
                image::Rgb([mix(r), mix(g), mix(b)])
            });
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, jpeg_quality.clamp(1, 100));
            rgb.write_with_encoder(enc)?;
        }
    }
    Ok(out)
}

pub fn write_image(img: &RgbaImage, path: &Path, format: ExportFormat, settings: &Settings) -> Result<()> {
    let bytes = encode(img, format, settings.jpeg_quality)?;
    std::fs::write(path, bytes).map_err(|e| Error::Other(format!("cannot write {}: {e}", path.display())))
}

/// Save into the configured folder with the default name.
pub fn save_default(img: &RgbaImage, settings: &Settings) -> Result<PathBuf> {
    std::fs::create_dir_all(&settings.save_dir)?;
    let bytes = encode(img, settings.export_format, settings.jpeg_quality)?;
    let (path, mut file) = create_unique(settings.save_dir.join(default_file_name(settings.export_format)))?;
    if let Err(e) = file.write_all(&bytes) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(Error::Other(format!("cannot write {}: {e}", path.display())));
    }
    Ok(path)
}

/// HTTPS only, except to this machine (a local server or proxy): the request carries the screenshot and any credentials.
fn check_upload_url(url: &str) -> Result<()> {
    let uri: ureq::http::Uri = url.parse().map_err(|e| Error::Other(format!("invalid upload URL: {e}")))?;
    let local = matches!(uri.host(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"));
    match uri.scheme_str() {
        Some("https") => Ok(()),
        Some("http") if local => Ok(()),
        _ => Err(Error::Other("the upload URL must start with https:// (http:// only for localhost)".into())),
    }
}

/// POST the image as `multipart/form-data` and extract the share link from the response.
pub fn upload(img: &RgbaImage, settings: &Settings) -> Result<String> {
    let cfg = &settings.upload;
    let url = cfg.url.trim();
    if url.is_empty() {
        return Err(Error::Other("set an upload endpoint in Settings → Upload first".into()));
    }
    check_upload_url(url)?;
    let png = encode(img, ExportFormat::Png, 100)?;
    let boundary = format!("----shotcraft{:x}", nanos());
    let name = default_file_name(ExportFormat::Png);
    let mut body = Vec::with_capacity(png.len() + 512);
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{name}\"\r\nContent-Type: image/png\r\n\r\n", cfg.file_field)
            .as_bytes(),
    );
    body.extend_from_slice(&png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut req = ureq::post(url)
        .config()
        // A redirect would carry the configured headers (API keys) to whatever host the server names.
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"));
    for h in &cfg.headers {
        if let Some((k, v)) = h.split_once(':') {
            req = req.header(k.trim(), v.trim());
        }
    }
    let mut resp = req.send(&body[..])?;
    if resp.status().is_redirection() {
        return Err(Error::Other(format!("the upload endpoint redirected (HTTP {}); enter the final URL in Settings → Upload", resp.status().as_u16())));
    }
    let text = resp.body_mut().read_to_string()?;
    extract_link(&text, &cfg.response_path)
}

/// The link in an upload response: the trimmed body, or the value at a dotted JSON path.
pub fn extract_link(body: &str, path: &str) -> Result<String> {
    let link = if path.trim().is_empty() {
        body.trim().to_owned()
    } else {
        let json: serde_json::Value = serde_json::from_str(body).map_err(|_| Error::Other("upload response is not JSON".into()))?;
        let mut v = &json;
        for key in path.trim().split('.') {
            v = match v {
                serde_json::Value::Array(a) => key.parse::<usize>().ok().and_then(|i| a.get(i)),
                _ => v.get(key),
            }
            .ok_or_else(|| Error::Other(format!("no `{path}` in upload response")))?;
        }
        v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())
    };
    if !(link.starts_with("http://") || link.starts_with("https://")) {
        return Err(Error::Other(format!("upload response has no link: {link:.120}")));
    }
    Ok(link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_binary_is_launched_from_its_path() {
        let replaced = PathBuf::from("/opt/shot craft/shotcraft (deleted)");
        let expected = if cfg!(target_os = "linux") { PathBuf::from("/opt/shot craft/shotcraft") } else { replaced.clone() };
        assert_eq!(installed_path(replaced), expected);
        let current = PathBuf::from("/opt/shot craft/shotcraft");
        assert_eq!(installed_path(current.clone()), current);
    }

    #[test]
    fn extracts_link_from_plain_and_json_bodies() {
        assert_eq!(extract_link("https://0x0.st/abc.png\n", "").unwrap(), "https://0x0.st/abc.png");
        let json = r#"{"data":{"files":[{"url":"https://x.test/1"}]}}"#;
        assert_eq!(extract_link(json, "data.files.0.url").unwrap(), "https://x.test/1");
        assert!(extract_link(json, "data.missing").is_err());
        assert!(extract_link("<html>error</html>", "").is_err());
    }

    #[test]
    fn save_names_never_collide() {
        let dir = std::env::temp_dir().join(format!("shotcraft-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, _) = create_unique(dir.join("a.png")).unwrap();
        let (b, _) = create_unique(dir.join("a.png")).unwrap();
        assert_eq!((a, b), (dir.join("a.png"), dir.join("a (2).png")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upload_needs_https_except_to_this_machine() {
        assert!(check_upload_url("https://x.test/up").is_ok());
        assert!(check_upload_url("http://localhost:8080/up").is_ok());
        assert!(check_upload_url("http://127.0.0.1/up").is_ok());
        assert!(check_upload_url("http://[::1]:8080/up").is_ok());
        assert!(check_upload_url("http://x.test/up").is_err());
        assert!(check_upload_url("ftp://x.test/up").is_err());
    }

    #[test]
    fn upload_does_not_follow_redirects() {
        use std::io::{BufRead, BufReader, Read};
        use std::net::TcpListener;
        let elsewhere = TcpListener::bind("127.0.0.1:0").unwrap();
        elsewhere.set_nonblocking(true).unwrap();
        let target = format!("http://127.0.0.1:{}/", elsewhere.local_addr().unwrap().port());
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/upload", server.local_addr().unwrap().port());
        let handle = std::thread::spawn(move || {
            let mut reader = BufReader::new(server.accept().unwrap().0);
            let mut len = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            reader.read_exact(&mut vec![0; len]).unwrap();
            let resp = format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            reader.get_mut().write_all(resp.as_bytes()).unwrap();
        });
        let mut settings = Settings::default();
        settings.upload.url = url;
        settings.upload.headers = vec!["X-Api-Key: secret".into()];
        let err = upload(&RgbaImage::new(2, 2), &settings).unwrap_err();
        handle.join().unwrap();
        assert!(err.to_string().contains("redirected"), "{err}");
        assert!(elsewhere.accept().is_err(), "the redirect target was contacted");
    }

    #[test]
    fn jpeg_flattens_alpha_on_white() {
        let img = RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 0]));
        let bytes = encode(&img, ExportFormat::Jpeg, 95).unwrap();
        let back = image::load_from_memory(&bytes).unwrap().into_rgb8();
        assert!(back.get_pixel(4, 4).0.iter().all(|&c| c > 245));
    }
}
