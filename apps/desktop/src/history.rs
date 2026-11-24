use std::cell::{Cell, OnceCell, RefCell};
use std::fs;
use std::mem;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use zgx::background::BackgroundPool;
use zgx::compose::prelude::*;
use zgx::reactive::{Runtime, Signal};
use zgx::scene::Color;
use zgx::semantics::Live;
use zgx::theme::{ThemeOverride, tok};
use zgx_desktop::{ScopedTask, TaskSpawner, WindowContext};

use crate::material::{metadata, quiet_action};

const LIMIT: usize = 40;
const ROW: f32 = 48.;
const PANEL: f32 = 5. * ROW;
const RECHECK: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Eq, Hash)]
struct Receipt {
    path: PathBuf,
    bytes: u64,
}

#[derive(Default)]
struct Pending {
    receipts: Vec<Receipt>,
    open: Option<PathBuf>,
    recheck: bool,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.receipts.is_empty() && self.open.is_none() && !self.recheck
    }

    fn run(self, mut rows: Vec<Receipt>) -> (Vec<Receipt>, Option<bool>) {
        let mut added = self.receipts;
        rows.retain(|row| !added.iter().any(|new| new.path == row.path));
        if self.recheck {
            rows.retain(|row| regular_file(&row.path));
        }
        added.retain(|row| regular_file(&row.path));
        added.append(&mut rows);
        added.truncate(LIMIT);

        let opened = self.open.map(|path| {
            if regular_file(&path) {
                open_parent(&path)
            } else {
                added.retain(|row| row.path != path);
                false
            }
        });
        (added, opened)
    }
}

struct State {
    runtime: Runtime,
    tasks: TaskSpawner,
    rows: Signal<Vec<Receipt>>,
    message: Signal<Option<&'static str>>,
    worker: OnceCell<BackgroundPool>,
    pending: RefCell<Pending>,
    busy: Cell<bool>,
    timer: RefCell<Option<ScopedTask>>,
}

#[derive(Clone)]
pub(crate) struct History(Rc<State>);

impl History {
    pub(crate) fn new(cx: &WindowContext) -> Self {
        let runtime = cx.ui.runtime.clone();
        Self(Rc::new(State {
            rows: runtime.signal(Vec::new()),
            message: runtime.signal(None),
            runtime,
            tasks: cx.tasks.clone(),
            worker: OnceCell::new(),
            pending: RefCell::default(),
            busy: Cell::new(false),
            timer: RefCell::default(),
        }))
    }

    pub(crate) fn record(&self, path: PathBuf, bytes: u64) {
        if !path.is_absolute() || path.file_name().is_none() {
            return;
        }
        let mut pending = self.0.pending.borrow_mut();
        pending.receipts.retain(|row| row.path != path);
        pending.receipts.insert(0, Receipt { path, bytes });
        pending.receipts.truncate(LIMIT);
        drop(pending);
        self.kick();
    }

    fn request_open(&self, path: PathBuf) {
        self.0.message.set(None);
        self.0.pending.borrow_mut().open = Some(path);
        self.kick();
    }

    fn kick(&self) {
        let state = &self.0;
        if state.busy.get() || state.pending.borrow().is_empty() {
            return;
        }
        state.busy.set(true);
        let ui = self.clone();
        state.tasks.spawn(async move {
            let state = &ui.0;
            let pending = mem::take(&mut *state.pending.borrow_mut());
            let opening = pending.open.is_some();
            let checking = pending.recheck || !pending.receipts.is_empty();
            if state.worker.get().is_none()
                && let Ok(worker) = BackgroundPool::new(1, 1)
            {
                let _ = state.worker.set(worker);
            }
            let rows = state.rows.get();
            let job = state
                .worker
                .get()
                .and_then(|worker| worker.spawn(move || pending.run(rows)).ok());
            let result = match job {
                Some(job) => job.await.ok(),
                None => None,
            };
            match result {
                Some((rows, opened)) => {
                    if state.rows.with(|current| *current != rows) {
                        state.rows.set(rows);
                    }
                    if let Some(ok) = opened {
                        state.message.set((!ok).then_some("Couldn’t open folder."));
                    } else if state.message.get() == Some("Couldn’t check recent files.") {
                        state.message.set(None);
                    }
                }
                None if opening => {
                    state.message.set(Some("Couldn’t open folder."));
                }
                None if checking => {
                    state.message.set(Some("Couldn’t check recent files."));
                }
                None => {}
            }
            state.busy.set(false);
            ui.schedule_recheck();
            ui.kick();
        });
    }

    fn schedule_recheck(&self) {
        let state = &self.0;
        let mut timer = state.timer.borrow_mut();
        if state.rows.with(Vec::is_empty) {
            timer.take();
        } else if timer.is_none() && !state.pending.borrow().recheck {
            let weak = Rc::downgrade(state);
            *timer = Some(state.tasks.spawn_scoped(async move {
                zgx::timer::sleep(RECHECK).await;
                if let Some(state) = weak.upgrade() {
                    state.timer.borrow_mut().take();
                    if !state.rows.with(Vec::is_empty) {
                        state.pending.borrow_mut().recheck = true;
                        Self(state).kick();
                    }
                }
            }));
        }
    }

    pub(crate) fn view(&self) -> View {
        let offset = self.0.runtime.signal(0.);
        let empty = self.0.rows.clone();
        let rows = self.0.rows.clone();
        let ui = self.clone();
        let message = self.0.message.clone();
        let scrollbar = ThemeOverride::new()
            .with(tok::SCROLLBAR_TRACK, Color(255, 255, 255, 24))
            .with(tok::SCROLLBAR_THUMB, Color(170, 170, 170, 255))
            .with(tok::SCROLLBAR_THUMB_ACTIVE, Color(235, 235, 235, 255));
        column()
            .w_full()
            .gap(12.)
            .child(
                text("Recent files")
                    .heading(2)
                    .text_size(20.)
                    .font_weight(500)
                    .text_color(Color(250, 250, 250, 255)),
            )
            .child(theme_scope(
                scrollbar,
                scroll(offset)
                    .w_full()
                    .h(PANEL)
                    .shrink_0()
                    .scrollbar(true)
                    .overscroll_contain()
                    .child(
                        column()
                            .w_full()
                            .child_when(
                                move || empty.with(Vec::is_empty).then_some(()),
                                |(), _| metadata(text("Files you receive appear here")),
                            )
                            .child(keyed(move || rows.get(), move |receipt, _| ui.row(receipt))),
                    ),
            ))
            .child_when(
                move || message.get(),
                |message, _| metadata(text(message)).live(Live::Polite),
            )
    }

    fn row(&self, receipt: Receipt) -> View {
        let name = receipt
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let visible_name: String = name
            .chars()
            .map(|c| {
                if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        let ui = self.clone();
        let path = receipt.path.clone();
        row()
            .w_full()
            .h(ROW)
            .shrink_0()
            .pr(8.)
            .gap(8.)
            .items_center()
            .child(
                column()
                    .grow()
                    .min_w(0.)
                    .child(
                        text(visible_name)
                            .w_full()
                            .text_color(Color(250, 250, 250, 255))
                            .text_wrap(false)
                            .line_clamp(1)
                            .text_overflow(TextOverflow::Ellipsis)
                            .accessible_label(name.to_string())
                            .tooltip(name.to_string()),
                    )
                    .child(
                        metadata(text(file_size(receipt.bytes)))
                            .accessible_label(format!("{} bytes", receipt.bytes))
                            .font_features(zgx::text_layout::FontFeatures::new([(*b"tnum", 1)]))
                            .text_wrap(false)
                            .text_overflow(TextOverflow::Ellipsis),
                    ),
            )
            .child(
                quiet_action(text("Open folder").text_wrap(false))
                    .accessible_label(format!("Open folder for {name}"))
                    .on_click(move || ui.request_open(path.clone())),
            )
    }
}

fn file_size(bytes: u64) -> String {
    if bytes < 1_000 {
        return format!("{bytes} B");
    }
    let (unit, base) = if bytes >= 1_000_000 {
        ("MB", 1_000_000.)
    } else {
        ("KB", 1_000.)
    };
    format!("{:.1} {unit}", bytes as f64 / base)
}

fn regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;

            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return false;
            }
        }
        metadata.is_file()
    })
}

fn open_parent(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(parent) = parent.canonicalize() else {
        return false;
    };
    parent.is_absolute()
        && fs::symlink_metadata(&parent).is_ok_and(|metadata| metadata.is_dir())
        && explore(&parent)
}

#[cfg(windows)]
fn explore(parent: &Path) -> bool {
    use std::ffi::{OsStr, OsString, c_void};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Prefix};
    use std::ptr::null_mut;

    const SEE_MASK_NOASYNC: u32 = 0x100;
    const SEE_MASK_FLAG_NO_UI: u32 = 0x400;
    const COINIT_APARTMENTTHREADED: u32 = 2;
    const COINIT_DISABLE_OLE1DDE: u32 = 4;

    #[repr(C)]
    struct ShellExecuteInfo {
        size: u32,
        mask: u32,
        window: *mut c_void,
        verb: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
        instance: *mut c_void,
        id_list: *mut c_void,
        class: *const u16,
        class_key: *mut c_void,
        hot_key: u32,
        icon_or_monitor: *mut c_void,
        process: *mut c_void,
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfo) -> i32;
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
        fn CoUninitialize();
    }

    let Some(Component::Prefix(prefix)) = parent.components().next() else {
        return false;
    };
    let wide: Vec<_> = parent.as_os_str().encode_wide().collect();
    let slash = u16::from(b'\\');
    let mut shell_path = match prefix.kind() {
        Prefix::VerbatimDisk(_) => wide[4..].to_vec(),
        Prefix::VerbatimUNC(_, _) => [vec![slash, slash], wide[8..].to_vec()].concat(),
        Prefix::Disk(_) | Prefix::UNC(_, _) => wide,
        _ => return false,
    };
    if shell_path.contains(&0) {
        return false;
    }
    if shell_path.last() != Some(&slash) {
        shell_path.push(slash);
    }
    let normal = PathBuf::from(OsString::from_wide(&shell_path));
    if !normal.is_absolute()
        || normal.components().any(|component| {
            matches!(component, Component::Normal(name) if matches!(name.encode_wide().last(), Some(32 | 46)))
        })
        || normal.canonicalize().ok().as_deref() != Some(parent)
    {
        return false;
    }
    shell_path.push(0);
    let verb: Vec<_> = OsStr::new("explore").encode_wide().chain([0]).collect();
    unsafe {
        if CoInitializeEx(
            null_mut(),
            COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE,
        ) < 0
        {
            return false;
        }
        let mut info: ShellExecuteInfo = mem::zeroed();
        info.size = size_of::<ShellExecuteInfo>() as u32;
        info.mask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.verb = verb.as_ptr();
        info.file = shell_path.as_ptr();
        info.show = 1;
        let opened = ShellExecuteExW(&mut info) != 0;
        CoUninitialize();
        opened
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn explore(parent: &Path) -> bool {
    use std::process::{Command, Stdio};

    #[cfg(target_os = "macos")]
    let tool = "/usr/bin/open";
    #[cfg(target_os = "linux")]
    let tool = "xdg-open";
    let mut directory = parent.as_os_str().to_os_string();
    if parent != Path::new("/") {
        directory.push("/");
    }
    Command::new(tool)
        .arg(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn explore(_: &Path) -> bool {
    false
}
