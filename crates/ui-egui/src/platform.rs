//! Small per-OS shims winit/egui do not cover.

/// Lift every window of this process above the menu bar and Dock (macOS). The capture
/// overlay must cover the whole display; winit's "always on top" level stays below them.
#[cfg(target_os = "macos")]
pub fn raise_all_windows() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    // NSScreenSaverWindowLevel
    const LEVEL: isize = 1000;
    let Some(mtm) = MainThreadMarker::new() else { return };
    for window in NSApplication::sharedApplication(mtm).windows().iter() {
        window.setLevel(LEVEL);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn raise_all_windows() {}
