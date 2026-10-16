//! Background process: tray menu and global shortcuts. Each action runs as a child process
//! (`shotcraft <command>`), so a crashing capture never takes the tray down.

use std::collections::HashMap;
use std::fs::File;
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime};

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use shotcraft_engine::config::{Settings, run_dir};
use shotcraft_engine::share;
use shotcraft_ui_egui::theme;

enum UserEvent {
    Hotkey(GlobalHotKeyEvent),
    Menu(MenuEvent),
}

struct Daemon {
    tray: Option<TrayIcon>,
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<HotKey>,
    hotkey_cmds: HashMap<u32, &'static str>,
    menu_cmds: HashMap<MenuId, &'static str>,
    settings_mtime: Option<SystemTime>,
    next_check: Instant,
    _lock: File,
}

pub(crate) const MENU: [Option<(&str, &str)>; 18] = [
    Some(("All-In-One", "all-in-one")),
    Some(("Capture Area", "capture-area")),
    Some(("Capture Previous Area", "capture-previous-area")),
    Some(("Capture Fullscreen", "capture-fullscreen")),
    Some(("Capture Window", "capture-window")),
    Some(("Self-Timer", "self-timer")),
    Some(("Scrolling Capture", "scrolling-capture")),
    Some(("Capture Text (OCR)", "capture-text")),
    None,
    Some(("Annotate Image…", "open-annotate")),
    Some(("Annotate from Clipboard", "open-from-clipboard")),
    Some(("Pin Image…", "pin")),
    None,
    Some(("Capture History", "open-history")),
    Some(("Restore Recently Closed", "restore-recently-closed")),
    None,
    Some(("Settings…", "open-settings")),
    Some(("Quit ShotCraft", "quit")),
];

pub fn run() -> anyhow::Result<()> {
    std::fs::create_dir_all(run_dir())?;
    let lock = File::options().create(true).truncate(false).write(true).open(run_dir().join("daemon.lock"))?;
    if lock.try_lock().is_err() {
        share::notify("ShotCraft is already running", "Use the tray icon or your shortcuts.");
        return Ok(());
    }
    #[allow(unused_mut)]
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder.build()?;
    let proxy = event_loop.create_proxy();
    GlobalHotKeyEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Hotkey(e));
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));
    let mut daemon = Daemon {
        tray: None,
        manager: None,
        registered: Vec::new(),
        hotkey_cmds: HashMap::new(),
        menu_cmds: HashMap::new(),
        settings_mtime: None,
        next_check: Instant::now(),
        _lock: lock,
    };
    event_loop.run_app(&mut daemon)?;
    Ok(())
}

impl Daemon {
    fn init(&mut self) {
        let settings = Settings::load();
        let menu = Menu::new();
        let shortcuts: HashMap<&str, String> = settings.hotkeys.entries().into_iter().collect();
        for entry in MENU {
            let added = match entry {
                Some((label, cmd)) => {
                    let accel = shortcuts.get(cmd).and_then(|k| k.parse().ok());
                    let item = MenuItem::new(label, true, accel);
                    self.menu_cmds.insert(item.id().clone(), cmd);
                    menu.append(&item)
                }
                None => menu.append(&PredefinedMenuItem::separator()),
            };
            if let Err(e) = added {
                eprintln!("tray menu: {e}");
            }
        }
        let mut tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("ShotCraft");
        if let Some(icon) = theme::app_icon(64).and_then(|(rgba, size)| tray_icon::Icon::from_rgba(rgba, size, size).ok()) {
            tray = tray.with_icon(icon);
        }
        match tray.build() {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("tray icon unavailable ({e}); shortcuts still work"),
        }
        match GlobalHotKeyManager::new() {
            Ok(m) => self.manager = Some(m),
            Err(e) => eprintln!("global shortcuts unavailable: {e}"),
        }
        self.apply_hotkeys(&settings);
        self.settings_mtime = Settings::modified();
    }

    fn apply_hotkeys(&mut self, settings: &Settings) {
        let Some(manager) = &self.manager else { return };
        let _ = manager.unregister_all(&self.registered);
        self.registered.clear();
        self.hotkey_cmds.clear();
        let mut failed = Vec::new();
        for (cmd, key) in settings.hotkeys.entries() {
            if key.is_empty() {
                continue;
            }
            match HotKey::from_str(&key) {
                Ok(hk) => match manager.register(hk) {
                    Ok(()) => {
                        self.hotkey_cmds.insert(hk.id(), cmd);
                        self.registered.push(hk);
                    }
                    Err(e) => failed.push(format!("{key} ({e})")),
                },
                Err(e) => failed.push(format!("{key} ({e})")),
            }
        }
        if !failed.is_empty() {
            share::warn("Some shortcuts are unavailable", &failed.join(", "));
        }
    }

    fn reload_if_changed(&mut self) {
        let mtime = Settings::modified();
        if mtime != self.settings_mtime {
            self.settings_mtime = mtime;
            self.apply_hotkeys(&Settings::load());
        }
    }
}

impl ApplicationHandler<UserEvent> for Daemon {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        // macOS requires the tray to be created once the loop is running.
        if cause == StartCause::Init {
            self.init();
        }
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, _event: WindowEvent) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Hotkey(e) if e.state == HotKeyState::Pressed => {
                if let Some(cmd) = self.hotkey_cmds.get(&e.id) {
                    share::spawn([cmd]);
                }
            }
            UserEvent::Hotkey(_) => {}
            UserEvent::Menu(e) => match self.menu_cmds.get(&e.id).copied() {
                Some("quit") => event_loop.exit(),
                Some(cmd) => share::spawn([cmd]),
                None => {}
            },
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if Instant::now() >= self.next_check {
            self.reload_if_changed();
            self.next_check = Instant::now() + Duration::from_secs(2);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_check));
    }
}
