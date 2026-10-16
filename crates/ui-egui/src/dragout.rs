//! Drag a file out of our window into other apps.
//!
//! Windows and macOS use the `drag` crate. On Linux it only supports GTK windows, so X11 is
//! handled here with a minimal XDND source: we own `XdndSelection`, track the pointer with
//! QueryPointer while the button is held, and answer the drop target's selection request
//! with a `text/uri-list`.

use std::path::Path;

/// Start dragging `file`; `preview` is a small image shown under the cursor where supported.
/// `on_done(dropped)` runs once the drag ends: synchronously on Linux (this call blocks until
/// the drop finishes), from the platform's drag callback elsewhere.
pub fn start(frame: &eframe::Frame, file: &Path, preview: &Path, on_done: impl Fn(bool) + Send + 'static) -> anyhow::Result<()> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let item = drag::DragItem::Files(vec![file.to_path_buf()]);
        let done = move |result: drag::DragResult, _| on_done(matches!(result, drag::DragResult::Dropped));
        drag::start_drag(frame, item, drag::Image::File(preview.to_path_buf()), done, drag::Options::default())?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let _ = preview;
        let dropped = x11::drag_file(own_x11_window(frame), file)?;
        on_done(dropped);
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn own_x11_window(frame: &eframe::Frame) -> Option<u32> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(h.window as u32),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
mod x11 {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use anyhow::{Context, anyhow};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, KeyButMask, PropMode, SelectionNotifyEvent,
        SelectionRequestEvent, Window, WindowClass,
    };
    use x11rb::wrapper::ConnectionExt as _;
    use x11rb::{CURRENT_TIME, NONE};

    x11rb::atom_manager! {
        Atoms: AtomsCookie {
            XdndAware,
            XdndSelection,
            XdndEnter,
            XdndPosition,
            XdndStatus,
            XdndLeave,
            XdndDrop,
            XdndFinished,
            XdndActionCopy,
            TARGETS,
            UTF8_STRING,
            URI_LIST: b"text/uri-list",
            TEXT_PLAIN: b"text/plain;charset=utf-8",
        }
    }

    struct Source<C: Connection> {
        conn: C,
        win: Window,
        atoms: Atoms,
        uri: Vec<u8>,
    }

    /// Returns whether a target accepted the drop.
    pub fn drag_file(own: Option<u32>, file: &Path) -> anyhow::Result<bool> {
        let (conn, screen) = x11rb::connect(None).context("X11 is required for drag and drop on Linux")?;
        let root = conn.setup().roots.get(screen).ok_or_else(|| anyhow!("X11 screen {screen} not found"))?.root;
        let atoms = Atoms::new(&conn)?.reply()?;
        let win = conn.generate_id()?;
        conn.create_window(0, win, root, -1, -1, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())?;
        conn.set_selection_owner(win, atoms.XdndSelection, CURRENT_TIME)?;
        let path = file.canonicalize()?;
        let uri = format!("file://{}\r\n", percent_encode(&path.to_string_lossy())).into_bytes();
        let src = Source { conn, win, atoms, uri };
        let result = src.run(root, own);
        let _ = src.conn.destroy_window(src.win);
        let _ = src.conn.flush();
        result
    }

    /// What the current drop target has told us.
    #[derive(Default)]
    struct Replies {
        accepted: bool,
        /// `Some(success)` once the target sent `XdndFinished`.
        finished: Option<bool>,
    }

    impl<C: Connection> Source<C> {
        fn run(&self, root: Window, own: Option<u32>) -> anyhow::Result<bool> {
            let mut target: Option<(Window, u32)> = None;
            let mut replies = Replies::default();
            let mut last_pos = (i16::MIN, i16::MIN);
            let started = Instant::now();
            loop {
                self.pump(target, &mut replies)?;
                let p = self.conn.query_pointer(root)?.reply()?;
                let held = p.mask.contains(KeyButMask::BUTTON1);
                let found = self.find_target(root, p.root_x, p.root_y)?.filter(|(w, _)| Some(*w) != own);
                if found.map(|f| f.0) != target.map(|t| t.0) {
                    if let Some((t, _)) = target {
                        self.send(t, self.atoms.XdndLeave, [self.win, 0, 0, 0, 0])?;
                    }
                    replies = Replies::default();
                    if let Some((t, version)) = found {
                        let v = version.min(5);
                        self.send(t, self.atoms.XdndEnter, [self.win, v << 24, self.atoms.URI_LIST, self.atoms.TEXT_PLAIN, 0])?;
                    }
                    target = found;
                    last_pos = (i16::MIN, i16::MIN);
                }
                if let Some((t, _)) = target
                    && (p.root_x, p.root_y) != last_pos
                {
                    let packed = ((p.root_x as u32) << 16) | (p.root_y as u16 as u32);
                    self.send(t, self.atoms.XdndPosition, [self.win, 0, packed, CURRENT_TIME, self.atoms.XdndActionCopy])?;
                    last_pos = (p.root_x, p.root_y);
                }
                if !held {
                    return match target {
                        Some(t) if replies.accepted => self.drop_on(t),
                        Some((t, _)) => {
                            self.send(t, self.atoms.XdndLeave, [self.win, 0, 0, 0, 0])?;
                            Ok(false)
                        }
                        None => Ok(false),
                    };
                }
                if started.elapsed() > Duration::from_secs(120) {
                    return Err(anyhow!("drag timed out"));
                }
                std::thread::sleep(Duration::from_millis(12));
            }
        }

        /// Drop and wait for the target's verdict; silence for 5 s counts as a failed drop.
        fn drop_on(&self, target: (Window, u32)) -> anyhow::Result<bool> {
            self.send(target.0, self.atoms.XdndDrop, [self.win, 0, CURRENT_TIME, 0, 0])?;
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut replies = Replies { accepted: true, finished: None };
            while replies.finished.is_none() && Instant::now() < deadline {
                self.pump(Some(target), &mut replies)?;
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(replies.finished == Some(true))
        }

        /// Handle pending events: the target's status and finish replies, and selection requests.
        fn pump(&self, target: Option<(Window, u32)>, replies: &mut Replies) -> anyhow::Result<()> {
            while let Some(ev) = self.conn.poll_for_event()? {
                match ev {
                    Event::ClientMessage(m) if m.type_ == self.atoms.XdndStatus || m.type_ == self.atoms.XdndFinished => {
                        let [from, flags, ..] = m.data.as_data32();
                        // Replies from a window we already left are stale.
                        let Some((_, version)) = target.filter(|t| t.0 == from) else { continue };
                        if m.type_ == self.atoms.XdndStatus {
                            replies.accepted = flags & 1 == 1;
                        } else {
                            // Version 5 added the success flag; older targets only say they are done.
                            replies.finished = Some(version < 5 || flags & 1 == 1);
                        }
                    }
                    Event::SelectionRequest(r) => self.answer(r)?,
                    _ => {}
                }
            }
            Ok(())
        }

        fn answer(&self, r: SelectionRequestEvent) -> anyhow::Result<()> {
            let property = if r.property == NONE { r.target } else { r.property };
            let a = &self.atoms;
            let ok = if r.target == a.TARGETS {
                self.conn.change_property32(PropMode::REPLACE, r.requestor, property, AtomEnum::ATOM, &[a.TARGETS, a.URI_LIST, a.TEXT_PLAIN, a.UTF8_STRING])?;
                true
            } else if [a.URI_LIST, a.TEXT_PLAIN, a.UTF8_STRING].contains(&r.target) {
                self.conn.change_property8(PropMode::REPLACE, r.requestor, property, r.target, &self.uri)?;
                true
            } else {
                false
            };
            let notify = SelectionNotifyEvent {
                response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: r.time,
                requestor: r.requestor,
                selection: r.selection,
                target: r.target,
                property: if ok { property } else { NONE },
            };
            self.conn.send_event(false, r.requestor, EventMask::NO_EVENT, notify)?;
            self.conn.flush()?;
            Ok(())
        }

        /// Deepest window under the pointer advertising XdndAware, with its version.
        fn find_target(&self, root: Window, x: i16, y: i16) -> anyhow::Result<Option<(Window, u32)>> {
            let mut w = root;
            for _ in 0..16 {
                if w != root
                    && let Some(v) = self.aware_version(w)?
                {
                    return Ok(Some((w, v)));
                }
                let child = self.conn.translate_coordinates(root, w, x, y)?.reply()?.child;
                if child == NONE {
                    return Ok(None);
                }
                w = child;
            }
            Ok(None)
        }

        fn aware_version(&self, w: Window) -> anyhow::Result<Option<u32>> {
            let reply = self.conn.get_property(false, w, self.atoms.XdndAware, AtomEnum::ATOM, 0, 1)?.reply()?;
            Ok(reply.value32().and_then(|mut v| v.next()))
        }

        fn send(&self, to: Window, kind: Atom, data: [u32; 5]) -> anyhow::Result<()> {
            let ev = ClientMessageEvent::new(32, to, kind, ClientMessageData::from(data));
            self.conn.send_event(false, to, EventMask::NO_EVENT, ev)?;
            self.conn.flush()?;
            Ok(())
        }
    }

    fn percent_encode(path: &str) -> String {
        let mut out = String::with_capacity(path.len());
        for b in path.bytes() {
            if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn uri_escapes_spaces_and_unicode() {
            assert_eq!(super::percent_encode("/tmp/Screenshot 1é.png"), "/tmp/Screenshot%201%C3%A9.png");
        }
    }
}
