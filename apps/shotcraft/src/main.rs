#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! ShotCraft: cross-platform screenshot tool.
//! Every capture mode is a subcommand, so any of them can be bound to an OS shortcut.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod daemon;
mod mcp;

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use anyhow::anyhow;
use clap::{Args, Parser, Subcommand, ValueEnum};
use shotcraft_engine::config::{self, LastArea, Settings, State, WindowCaptureStyle};
use shotcraft_engine::doc::effects::IRect;
use shotcraft_engine::doc::model::{Background, BgFill, Doc};
use shotcraft_engine::{capture, history, ocr, share};
use shotcraft_ui_egui::overlay::{self, Mode, Outcome};
use shotcraft_ui_egui::pin::{self, ImageSource};
use shotcraft_ui_egui::{dialogs, editor, history_view, notify, quick, scrolling, settings_view};

#[derive(Parser)]
#[command(name = "shotcraft", version, about = "Capture, annotate, redact and share screenshots.")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// Region in pixels of the chosen display, origin at its top-left corner.
#[derive(Args, Clone, Copy, Debug, Default)]
struct AreaArgs {
    #[arg(long)]
    x: Option<u32>,
    #[arg(long)]
    y: Option<u32>,
    #[arg(long)]
    width: Option<u32>,
    #[arg(long)]
    height: Option<u32>,
    /// 1 is the main display, 2 the secondary, etc. Defaults to the display under the cursor.
    #[arg(long)]
    display: Option<usize>,
}

impl AreaArgs {
    fn rect(&self) -> Option<IRect> {
        Some(IRect { x: self.x?, y: self.y?, w: self.width?, h: self.height? })
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Copy,
    Save,
    Annotate,
    Upload,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run in the background with a tray icon and global shortcuts (default).
    Daemon,
    /// All capture modes from one bar.
    AllInOne(AreaArgs),
    /// Select an area; with x/y/width/height captures it immediately.
    CaptureArea {
        #[command(flatten)]
        area: AreaArgs,
        #[arg(long)]
        action: Option<Action>,
    },
    /// Capture the last selected area again.
    CapturePreviousArea {
        #[arg(long)]
        action: Option<Action>,
    },
    CaptureFullscreen {
        #[arg(long)]
        display: Option<usize>,
        #[arg(long)]
        action: Option<Action>,
    },
    CaptureWindow {
        #[arg(long)]
        action: Option<Action>,
    },
    /// Select an area, then capture it after a countdown.
    SelfTimer {
        #[arg(long)]
        action: Option<Action>,
    },
    /// Capture content taller than the screen while it scrolls.
    ScrollingCapture {
        #[command(flatten)]
        area: AreaArgs,
        #[arg(long, hide = true)]
        monitor_id: Option<u32>,
        /// Start capturing immediately.
        #[arg(long)]
        start: bool,
        /// Scroll automatically until the content ends.
        #[arg(long)]
        autoscroll: bool,
    },
    /// Float an image above all windows.
    Pin {
        #[arg(long)]
        filepath: Option<PathBuf>,
        #[arg(long, hide = true)]
        history: Option<String>,
    },
    /// Copy text (or QR code contents) from a selection or an image file.
    CaptureText {
        #[arg(long)]
        filepath: Option<PathBuf>,
        #[command(flatten)]
        area: AreaArgs,
        #[arg(long)]
        linebreaks: Option<bool>,
    },
    /// Open an image or .shotcraft project in the editor.
    OpenAnnotate {
        #[arg(long)]
        filepath: Option<PathBuf>,
        #[arg(long, hide = true)]
        history: Option<String>,
    },
    OpenFromClipboard,
    /// Show the Quick Access Overlay for an image.
    AddQuickAccessOverlay {
        #[arg(long)]
        filepath: Option<PathBuf>,
        #[arg(long, hide = true)]
        history: Option<String>,
    },
    OpenHistory,
    RestoreRecentlyClosed,
    OpenSettings {
        #[arg(long)]
        tab: Option<String>,
    },
    /// Serve the Model Context Protocol on stdin/stdout so AI agents can capture, read and share screenshots.
    Mcp,
    #[command(hide = true)]
    Notify {
        #[arg(long)]
        title: String,
        /// Defaults to `SHOTCRAFT_NOTIFY_BODY`, which is how the app passes it.
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        warning: bool,
    },
    #[command(hide = true)]
    ClipboardServe {
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(long)]
        text: Option<PathBuf>,
    },
    #[command(hide = true)]
    Countdown {
        #[arg(long)]
        monitor_id: u32,
        #[command(flatten)]
        area: AreaArgs,
        #[arg(long)]
        action: Option<Action>,
    },
}

fn main() {
    let cli = Cli::parse();
    let cmd = cli.cmd.unwrap_or(Cmd::Daemon);
    // A toast that fails must not launch another toast: without a display that would never end.
    // The MCP host reports the server's failures; a toast would surprise the human.
    let toast_on_error = !matches!(cmd, Cmd::Notify { .. } | Cmd::Mcp);
    // A bug in one mode must end with a message, not a silent crash; the panic hook has already printed details.
    let result =
        std::panic::catch_unwind(AssertUnwindSafe(|| dispatch(cmd))).unwrap_or_else(|panic| Err(anyhow!("internal error: {}", panic_message(&*panic))));
    if let Err(err) = result {
        eprintln!("shotcraft: {err:#}");
        if toast_on_error {
            share::warn("Something went wrong", &format!("{err:#}"));
        }
        std::process::exit(1);
    }
}

/// The text a `panic!` carried, if any.
fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic.downcast_ref::<&str>().copied().or_else(|| panic.downcast_ref::<String>().map(String::as_str)).unwrap_or_default()
}

fn dispatch(cmd: Cmd) -> anyhow::Result<()> {
    config::ensure_private_dirs()?;
    let settings = Settings::load();
    match cmd {
        Cmd::Daemon => daemon::run(),
        Cmd::AllInOne(area) => {
            let preset = area.rect().map(|rect| overlay::Preset { display: area.display, rect });
            overlay_flow(Mode::AllInOne, preset, None, &settings)
        }
        Cmd::CaptureArea { area, action } => match area.rect() {
            Some(rect) => {
                let shot = capture::capture_monitor(None, area.display)?;
                let r = capture::clip(rect, &shot.image)?;
                remember_area(shot.id, r);
                finish_capture(Doc::new(capture::crop(&shot.image, r)), "area", action, &settings)
            }
            None => overlay_flow(Mode::Area, None, action, &settings),
        },
        Cmd::CapturePreviousArea { action } => {
            let Some(last) = State::load().last_area else {
                return overlay_flow(Mode::Area, None, action, &settings);
            };
            let shot = capture::capture_monitor(Some(last.monitor_id), None)?;
            let r = capture::clip(IRect { x: last.x, y: last.y, w: last.width, h: last.height }, &shot.image)?;
            finish_capture(Doc::new(capture::crop(&shot.image, r)), "area", action, &settings)
        }
        Cmd::CaptureFullscreen { display, action } => {
            let shot = capture::capture_monitor(None, display)?;
            finish_capture(Doc::new(shot.image), "fullscreen", action, &settings)
        }
        Cmd::CaptureWindow { action } => overlay_flow(Mode::Window, None, action, &settings),
        Cmd::SelfTimer { action } => overlay_flow(Mode::Timer, None, action, &settings),
        Cmd::ScrollingCapture { area, monitor_id, start, autoscroll } => match area.rect() {
            Some(rect) => {
                let Some(image) = scrolling::run(monitor_id, area.display, rect, start, autoscroll)? else { return Ok(()) };
                finish_capture(Doc::new(image), "scrolling", None, &settings)
            }
            None => overlay_flow(Mode::Scrolling, None, None, &settings),
        },
        Cmd::Pin { filepath, history } => {
            let source = image_source(filepath, history)?;
            pin::run(source)
        }
        Cmd::CaptureText { filepath, area, linebreaks } => {
            let keep = linebreaks.unwrap_or(settings.ocr_keep_linebreaks);
            if let Some(path) = filepath {
                return copy_text_from(&image::open(path)?.into_rgba8(), keep);
            }
            match area.rect() {
                Some(rect) => {
                    let shot = capture::capture_monitor(None, area.display)?;
                    copy_text_from(&capture::crop(&shot.image, capture::clip(rect, &shot.image)?), keep)
                }
                None => overlay_flow(Mode::Text, None, None, &settings),
            }
        }
        Cmd::OpenAnnotate { filepath, history } => {
            if let Some(id) = history {
                return editor::run(history::load_doc(&id)?, Some(id), None);
            }
            let Some(path) = filepath.or_else(dialogs::pick_image) else { return Ok(()) };
            editor::run(Doc::open(&path)?, None, Some(path))
        }
        Cmd::OpenFromClipboard => editor::run(Doc::new(share::clipboard_image()?), None, None),
        Cmd::AddQuickAccessOverlay { filepath, history } => {
            let id = match (history, filepath) {
                (Some(id), _) => id,
                (None, Some(path)) => history::add(&Doc::open(&path)?, "file")?.id,
                (None, None) => anyhow::bail!("pass --filepath"),
            };
            quick::run(id)
        }
        Cmd::OpenHistory => history_view::run(),
        Cmd::RestoreRecentlyClosed => {
            let mut id = None;
            State::update(|s| id = s.recently_closed.pop());
            let id = id.or_else(|| history::list().first().map(|e| e.id.clone())).ok_or_else(|| anyhow!("history is empty"))?;
            quick::run(id)
        }
        Cmd::OpenSettings { tab } => settings_view::run(tab),
        Cmd::Mcp => mcp::run(),
        Cmd::Notify { title, body, warning } => notify::run(title, body.or_else(|| std::env::var(share::NOTIFY_BODY_ENV).ok()).unwrap_or_default(), warning),
        Cmd::ClipboardServe { image, text } => Ok(share::serve_clipboard(image.as_deref(), text.as_deref())?),
        Cmd::Countdown { monitor_id, area, action } => {
            let rect = area.rect().ok_or_else(|| anyhow!("countdown needs an area"))?;
            if !scrolling::countdown(monitor_id, rect, settings.timer_seconds)? {
                return Ok(());
            }
            // Give the compositor a moment to remove the countdown window before shooting.
            std::thread::sleep(std::time::Duration::from_millis(250));
            let shot = capture::capture_monitor(Some(monitor_id), None)?;
            finish_capture(Doc::new(capture::crop(&shot.image, capture::clip(rect, &shot.image)?)), "area", action, &settings)
        }
    }
}

fn remember_area(monitor_id: u32, r: IRect) {
    State::update(|s| s.last_area = Some(LastArea { monitor_id, x: r.x, y: r.y, width: r.w, height: r.h }));
}

fn image_source(filepath: Option<PathBuf>, history: Option<String>) -> anyhow::Result<ImageSource> {
    if let Some(id) = history {
        return Ok(ImageSource::History(id));
    }
    let path = filepath.or_else(dialogs::pick_image).ok_or_else(|| anyhow!("no image selected"))?;
    Ok(ImageSource::File(path))
}

fn overlay_flow(mode: Mode, preset: Option<overlay::Preset>, action: Option<Action>, settings: &Settings) -> anyhow::Result<()> {
    let Some(outcome) = overlay::run(mode, preset)? else { return Ok(()) };
    match outcome {
        Outcome::Image { image, kind, area } => {
            if let Some((id, r)) = area {
                remember_area(id, r);
            }
            finish_capture(Doc::new(image), kind, action, settings)
        }
        Outcome::Window { id, fallback } => {
            // The overlay is gone; grab the window's own pixels (unobscured where the OS allows).
            std::thread::sleep(std::time::Duration::from_millis(150));
            let image = capture::capture_window(id).unwrap_or(fallback);
            finish_capture(window_doc(image, settings.window_style), "window", action, settings)
        }
        Outcome::Scrolling { monitor_id, rect } => {
            remember_area(monitor_id, rect);
            share::spawn(scrolling_args(monitor_id, rect));
            Ok(())
        }
        Outcome::Timer { monitor_id, rect } => {
            remember_area(monitor_id, rect);
            share::spawn(countdown_args(monitor_id, rect, action));
            Ok(())
        }
        Outcome::Text { image } => copy_text_from(&image, settings.ocr_keep_linebreaks),
    }
}

fn scrolling_args(monitor_id: u32, r: IRect) -> Vec<String> {
    let mut args = vec!["scrolling-capture".to_owned(), "--monitor-id".into(), monitor_id.to_string()];
    args.extend(area_args(r));
    args.push("--start".into());
    args
}

fn countdown_args(monitor_id: u32, r: IRect, action: Option<Action>) -> Vec<String> {
    let mut args = vec!["countdown".to_owned(), "--monitor-id".into(), monitor_id.to_string()];
    args.extend(area_args(r));
    if let Some(value) = action.and_then(|a| a.to_possible_value()) {
        args.extend(["--action".into(), value.get_name().to_owned()]);
    }
    args
}

fn area_args(r: IRect) -> [String; 8] {
    ["--x".into(), r.x.to_string(), "--y".into(), r.y.to_string(), "--width".into(), r.w.to_string(), "--height".into(), r.h.to_string()]
}

fn window_doc(image: image::RgbaImage, style: WindowCaptureStyle) -> Doc {
    let mut doc = Doc::new(image);
    let radius = if cfg!(target_os = "macos") { 10.0 } else { 0.0 };
    doc.background = match style {
        WindowCaptureStyle::Plain => None,
        WindowCaptureStyle::Shadow => {
            Some(Background { fill: BgFill::Transparent, padding: 56.0, radius, shadow: 0.55, auto_balance: false, ..Default::default() })
        }
        WindowCaptureStyle::Background => Some(Background { radius, auto_balance: false, ..Default::default() }),
    };
    doc
}

/// Store the capture in history, then copy/save/annotate/upload or show Quick Access.
fn finish_capture(doc: Doc, kind: &str, action: Option<Action>, settings: &Settings) -> anyhow::Result<()> {
    let entry = history::add(&doc, kind)?;
    history::prune(settings.history_days);
    let image = || history::load_image(&entry.id);
    match action {
        Some(Action::Copy) => {
            share::copy_image(&image()?)?;
            share::notify("Copied to clipboard", &format!("{} × {}", entry.width, entry.height));
        }
        Some(Action::Save) => {
            let path = share::save_default(&image()?, settings)?;
            share::notify("Screenshot saved", &path.display().to_string());
        }
        Some(Action::Annotate) => share::spawn(["open-annotate", "--history", &entry.id]),
        Some(Action::Upload) => {
            let link = share::upload(&image()?, settings)?;
            share::copy_text(&link)?;
            share::notify("Link copied", &link);
        }
        None => {
            if settings.copy_after_capture {
                share::copy_image(&image()?)?;
            }
            if settings.save_after_capture {
                share::save_default(&image()?, settings)?;
            }
            if settings.show_quick_access {
                share::spawn(["add-quick-access-overlay", "--history", &entry.id]);
            } else if !settings.copy_after_capture && !settings.save_after_capture {
                share::copy_image(&image()?)?;
                share::notify("Copied to clipboard", &format!("{} × {}", entry.width, entry.height));
            }
        }
    }
    Ok(())
}

fn copy_text_from(img: &image::RgbaImage, keep_linebreaks: bool) -> anyhow::Result<()> {
    let text = ocr::extract(img, keep_linebreaks)?;
    if text.trim().is_empty() {
        share::warn("No text found", "Try a larger or sharper selection.");
        return Ok(());
    }
    share::copy_text(&text)?;
    share::notify("Text copied", &text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    pub(crate) fn parse<S: Into<String>>(args: impl IntoIterator<Item = S>) -> Option<Cmd> {
        Cli::try_parse_from(std::iter::once("shotcraft".to_owned()).chain(args.into_iter().map(Into::into))).ok()?.cmd
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    /// Escaped panics, in any mode and in MCP tool calls, are reported with the text they carried.
    #[test]
    fn panic_messages_keep_their_text() {
        let caught = |f: fn()| std::panic::catch_unwind(f).unwrap_err();
        assert_eq!(panic_message(&*caught(|| panic!("literal"))), "literal");
        assert_eq!(panic_message(&*caught(|| panic!("formatted {}", 3))), "formatted 3");
        assert_eq!(panic_message(&*caught(|| std::panic::panic_any(7))), "");
    }

    /// Shortcuts and the tray menu launch subcommands by name, so a rename would only fail at launch.
    #[test]
    fn shortcut_and_menu_commands_exist() {
        let shortcuts = Settings::default().hotkeys.entries().into_iter().map(|(cmd, _)| cmd);
        let menu = daemon::MENU.into_iter().flatten().map(|(_, cmd)| cmd).filter(|&cmd| cmd != "quit");
        for cmd in shortcuts.chain(menu) {
            assert!(parse([cmd]).is_some(), "`{cmd}` is not a subcommand");
        }
    }

    #[test]
    fn launched_area_commands_round_trip() {
        let rect = IRect { x: 10, y: 20, w: 300, h: 400 };
        let Some(Cmd::ScrollingCapture { area, monitor_id: Some(7), start: true, autoscroll: false }) = parse(scrolling_args(7, rect)) else {
            panic!("scrolling-capture arguments do not parse back");
        };
        assert_eq!(area.rect(), Some(rect));
        let Some(Cmd::Countdown { monitor_id: 7, area, action: Some(Action::Save) }) = parse(countdown_args(7, rect, Some(Action::Save))) else {
            panic!("countdown arguments do not parse back");
        };
        assert_eq!(area.rect(), Some(rect));
    }
}
