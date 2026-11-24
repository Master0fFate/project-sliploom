use zgx::reactive::Signal;
use zgx_desktop::WindowContext;

pub(crate) fn transparent_window() -> bool {
    #[cfg(windows)]
    {
        windows::supported()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub(crate) fn available(cx: &WindowContext) -> Signal<bool> {
    #[cfg(windows)]
    {
        windows::watch(cx)
    }
    #[cfg(not(windows))]
    {
        cx.ui.runtime.signal(false)
    }
}

#[cfg(windows)]
pub(crate) use windows::is_maximized;

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::{c_char, c_void};
    use std::sync::OnceLock;
    use zgx::reactive::Effect;

    type Hwnd = *mut c_void;
    type SetComposition = unsafe extern "system" fn(Hwnd, *mut CompositionData) -> i32;

    const ACRYLIC_BUILD: u32 = 17763;
    const SYSTEM_BACKDROP_BUILD: u32 = 22621;
    const GA_ROOT: u32 = 2;
    const GW_OWNER: u32 = 4;
    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
    const GRAPHITE_ABGR: u32 = 0xA0202020;
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMSBT_NONE: i32 = 1;
    const DWMSBT_TRANSIENTWINDOW: i32 = 3;

    #[repr(C)]
    struct Version {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
    }

    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        color: u32,
        animation: u32,
    }

    #[repr(C)]
    struct CompositionData {
        attribute: u32,
        data: *mut AccentPolicy,
        size: usize,
    }

    #[derive(Default)]
    #[repr(C)]
    struct Margins {
        left: i32,
        right: i32,
        top: i32,
        bottom: i32,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(version: *mut Version) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn GetModuleHandleA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(
            module: *mut c_void,
            name: *const c_char,
        ) -> Option<unsafe extern "system" fn() -> isize>;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumThreadWindows(
            thread: u32,
            callback: unsafe extern "system" fn(Hwnd, isize) -> i32,
            context: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(hwnd: Hwnd, process: *mut u32) -> u32;
        fn GetAncestor(hwnd: Hwnd, flags: u32) -> Hwnd;
        fn GetWindow(hwnd: Hwnd, command: u32) -> Hwnd;
        fn GetClassNameW(hwnd: Hwnd, class: *mut u16, capacity: i32) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, capacity: i32) -> i32;
        fn IsZoomed(hwnd: Hwnd) -> i32;
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmExtendFrameIntoClientArea(hwnd: Hwnd, margins: *const Margins) -> i32;
        fn DwmSetWindowAttribute(
            hwnd: Hwnd,
            attribute: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }

    #[derive(Clone, Copy)]
    enum Backdrop {
        Accent(SetComposition),
        System,
    }

    pub(super) fn supported() -> bool {
        backdrop().is_some()
    }

    fn backdrop() -> Option<Backdrop> {
        static BACKDROP: OnceLock<Option<Backdrop>> = OnceLock::new();
        *BACKDROP.get_or_init(|| {
            let mut version = Version {
                size: size_of::<Version>() as u32,
                major: 0,
                minor: 0,
                build: 0,
                platform: 0,
                service_pack: [0; 128],
            };
            if unsafe { RtlGetVersion(&mut version) } != 0 || version.major < 10 {
                return None;
            }
            if version.build >= SYSTEM_BACKDROP_BUILD {
                return Some(Backdrop::System);
            }
            if version.build < ACRYLIC_BUILD {
                return None;
            }
            let user32 = unsafe { GetModuleHandleA(c"user32.dll".as_ptr()) };
            if user32.is_null() {
                return None;
            }
            let address =
                unsafe { GetProcAddress(user32, c"SetWindowCompositionAttribute".as_ptr()) }?;
            Some(Backdrop::Accent(unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, SetComposition>(address)
            }))
        })
    }

    struct Search {
        thread: u32,
        process: u32,
        found: Option<Hwnd>,
    }

    unsafe extern "system" fn find_app_window(hwnd: Hwnd, context: isize) -> i32 {
        let search = unsafe { &mut *(context as *mut Search) };
        let mut process = 0;
        if unsafe { GetWindowThreadProcessId(hwnd, &mut process) } != search.thread
            || process != search.process
            || unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd
            || !unsafe { GetWindow(hwnd, GW_OWNER) }.is_null()
        {
            return 1;
        }

        let mut class = [0; 14];
        let mut title = [0; 64];
        let title_len =
            unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) } as usize;
        if unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } == 12
            && class[..12]
                .iter()
                .copied()
                .eq("Window Class".encode_utf16())
            && title[..title_len]
                .iter()
                .copied()
                .eq(crate::APP_NAME.encode_utf16())
        {
            if search.found.is_some() {
                return 0;
            }
            search.found = Some(hwnd);
        }
        1
    }

    fn seam_window() -> Option<Hwnd> {
        let mut search = Search {
            thread: unsafe { GetCurrentThreadId() },
            process: std::process::id(),
            found: None,
        };
        let completed = unsafe {
            EnumThreadWindows(
                search.thread,
                find_app_window,
                (&mut search as *mut Search) as isize,
            )
        };
        if completed == 0 { None } else { search.found }
    }

    pub(crate) fn is_maximized() -> Option<bool> {
        seam_window().map(|hwnd| unsafe { IsZoomed(hwnd) != 0 })
    }

    impl Backdrop {
        unsafe fn set(self, hwnd: Hwnd, enabled: bool) -> bool {
            match self {
                Self::Accent(set) => {
                    let mut policy = AccentPolicy {
                        state: if enabled {
                            ACCENT_ENABLE_ACRYLICBLURBEHIND
                        } else {
                            ACCENT_DISABLED
                        },
                        flags: 0,
                        color: if enabled { GRAPHITE_ABGR } else { 0 },
                        animation: 0,
                    };
                    let mut data = CompositionData {
                        attribute: WCA_ACCENT_POLICY,
                        data: &mut policy,
                        size: size_of::<AccentPolicy>(),
                    };
                    unsafe { set(hwnd, &mut data) != 0 }
                }
                Self::System => {
                    let kind = if enabled {
                        DWMSBT_TRANSIENTWINDOW
                    } else {
                        DWMSBT_NONE
                    };
                    let margins = Margins {
                        bottom: if enabled { i32::from(u16::MAX) } else { 0 },
                        ..Margins::default()
                    };
                    let frame = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
                    let backdrop = unsafe {
                        DwmSetWindowAttribute(
                            hwnd,
                            DWMWA_SYSTEMBACKDROP_TYPE,
                            (&kind as *const i32).cast(),
                            size_of::<i32>() as u32,
                        )
                    };
                    frame >= 0 && backdrop >= 0
                }
            }
        }
    }

    struct Acrylic {
        available: Signal<bool>,
        _effect: Effect,
    }

    pub(super) fn watch(cx: &WindowContext) -> Signal<bool> {
        let Some(backdrop) = backdrop() else {
            return cx.ui.runtime.signal(false);
        };
        cx.ui
            .runtime
            .extension(|runtime| {
                let available = runtime.signal(false);
                let output = available.clone();
                let ready = cx.window_info.ready.clone();
                let themes = cx.ui.theme().clone();
                let mut needs_reset = None;
                let _effect = runtime.effect(move || {
                    let ready = ready.get();
                    let preferences = themes.system_preferences();
                    let enabled = ready
                        && !preferences.reduced_transparency
                        && !preferences.increased_contrast;
                    let mut applied = false;
                    if (enabled || needs_reset.is_some())
                        && let Some(hwnd) = seam_window()
                    {
                        if enabled {
                            needs_reset = Some(hwnd);
                            applied = unsafe { backdrop.set(hwnd, true) };
                        }
                        if !applied
                            && needs_reset == Some(hwnd)
                            && unsafe { backdrop.set(hwnd, false) }
                        {
                            needs_reset = None;
                        }
                    }
                    if !ready {
                        needs_reset = None;
                    }
                    output.set(applied);
                });
                Acrylic { available, _effect }
            })
            .available
            .clone()
    }
}
