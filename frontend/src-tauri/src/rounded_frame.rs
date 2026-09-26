//! Windows 窗口外形：圆角 + 抗锯齿描边。
//!
//! 为什么不直接把主窗做成 transparent：那样虽然能拿到 WebView 的 alpha 圆角，
//! 但会废掉「不透明窗口 + set_background_color」这一套去白闪方案，而且中文字体的
//! ClearType 子像素抗锯齿有可能退化成灰度。所以这里沿用不透明主窗。
//!
//! Win11：`DWMWA_WINDOW_CORNER_PREFERENCE`，DWM 原生抗锯齿圆角，直接交给系统。
//!
//! Win10：该属性不被支持，只能 `SetWindowRgn` 硬裁剪 —— 区域是 **1-bit 掩码**，
//! 圆角必然是锯齿台阶。对策是**保留** region 不动，额外叠一个独立的 layered 伴随窗，
//! 用 8-bit alpha 沿同一条圆角路径描一圈边：内侧不透明带正好压住 region 的台阶，
//! 外侧淡出到桌面。轮廓从此由这一层定义，锯齿被完全盖在下面。
//!
//! 关键约束：主窗不能用 `UpdateLayeredWindow`（它要求整个窗口是一张位图，WebView
//! 是子窗口，一进去就没了）；而伴随窗没有子窗口，可以放心用 layered。
//!
//! 参数经 `G:\AIGC\round-probe` 探针像素级验证：
//!   现状 region：角落 2 种颜色、44/44 行硬切；
//!   叠加描边后：21+ 种颜色、0/43 行硬切。

#![cfg(target_os = "windows")]

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;

use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWINDOWATTRIBUTE,
    DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

/// 逻辑圆角半径（CSS 像素）。改这里的同时要改 `styles.css` 的 `--window-radius`。
/// Win11 的 DWM 圆角是系统固定的约 8px；想两边视觉接近就把它调到 8。
const CORNER_RADIUS: i32 = 16;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;

/// 伴随窗比主窗外扩的逻辑像素数，给外侧淡出留出空间
const PAD: f64 = 4.0;
/// 描边剖面（逻辑像素）：内侧不透明带厚度 / 内侧羽化 / 外侧淡出厚度
const INNER_BAND: f64 = 1.5;
const INNER_RAMP: f64 = 0.5;
const OUTER_FADE: f64 = 1.2;
/// 描边颜色：取代 CSS 里那条 `inset 0 0 0 1px` 边框（区域裁剪下圆角段画不出来）。
/// 必须是**不透明**的 —— 只有不透明像素才能盖住 region 漏出来的桌面颜色。
const STROKE: (u8, u8, u8) = (198, 204, 218);

/// 自定义线程消息：让伴随窗线程立刻同步一次（不必等下一拍 timer）
const WM_SYNC: u32 = WM_APP + 1;
/// 主窗 subclass 的 id
const SUBCLASS_ID: usize = 1;
const TIMER_ID: usize = 1;
/// 兜底校正间隔（毫秒）。移动/缩放有 subclass 实时通知，这个只是防漏。
const TIMER_MS: u32 = 80;

struct Shared {
    owner: AtomicIsize,
    thread: AtomicU32,
    /// 主窗自己是否允许显示描边（拖动中 / 刚失焦时置 false）
    want_visible: AtomicBool,
    /// 伴随窗当前是否真的显示着
    shown: AtomicBool,
    subclassed: AtomicBool,
    quit: AtomicBool,
}

impl Shared {
    fn new(owner: HWND) -> Self {
        Self {
            owner: AtomicIsize::new(owner.0 as isize),
            thread: AtomicU32::new(0),
            want_visible: AtomicBool::new(true),
            shown: AtomicBool::new(false),
            subclassed: AtomicBool::new(false),
            quit: AtomicBool::new(false),
        }
    }
    fn owner(&self) -> HWND {
        HWND(self.owner.load(Ordering::Relaxed) as *mut c_void)
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// 圆角矩形的有向距离：<0 在内部，0 在边界上，>0 在外部。
fn sd_rounded_box(px: f64, py: f64, half_w: f64, half_h: f64, r: f64) -> f64 {
    let qx = px.abs() - half_w + r;
    let qy = py.abs() - half_h + r;
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    qx.max(qy).min(0.0) + outside - r
}

/// 描边覆盖率：内侧用 INNER_RAMP 从 0 升到 1，外侧用 OUTER_FADE 降到 0，取小者。
/// 等价于 D2D / GDI+ 的抗锯齿结果，但不必拖进整套图形栈。
fn coverage(t: f64, inner_band: f64, inner_ramp: f64, outer_fade: f64) -> f64 {
    let a_in = ((t + inner_band) / inner_ramp).clamp(0.0, 1.0);
    let a_out = (1.0 - t / outer_fade).clamp(0.0, 1.0);
    a_in.min(a_out)
}

/// 伴随窗的位图。只在主窗尺寸或 DPI 变化时才重新计算像素，移动时复用。
struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    w: i32,
    h: i32,
    scale: f64,
    pad: i32,
}

impl Surface {
    unsafe fn free(self) {
        let _ = DeleteObject(self.bmp.into());
        let _ = DeleteDC(self.dc);
    }
}

unsafe fn build_surface(w: i32, h: i32, scale: f64) -> Option<Surface> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let pad = (PAD * scale).round() as i32;
    let rw = w + pad * 2;
    let rh = h + pad * 2;
    let radius = (CORNER_RADIUS as f64 * scale).round();
    let inner_band = INNER_BAND * scale;
    let inner_ramp = INNER_RAMP * scale;
    let outer_fade = OUTER_FADE * scale;
    let (sr, sg, sb) = STROKE;

    let screen_dc = GetDC(None);
    let dc = CreateCompatibleDC(Some(screen_dc));

    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = rw;
    bmi.bmiHeader.biHeight = -rh; // 负值 = 自上而下的行序
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB.0;

    let mut bits: *mut c_void = ptr::null_mut();
    let bmp = match CreateDIBSection(Some(screen_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
        Ok(b) => b,
        Err(_) => {
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen_dc);
            return None;
        }
    };
    if bits.is_null() {
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen_dc);
        return None;
    }

    let stride = rw * 4;
    let buf = std::slice::from_raw_parts_mut(bits as *mut u8, (stride * rh) as usize);
    let half_w = w as f64 / 2.0;
    let half_h = h as f64 / 2.0;

    for py in 0..rh {
        for px in 0..rw {
            // 伴随窗坐标 -> 主窗中心坐标
            let ix = px as f64 - pad as f64 - half_w + 0.5;
            let iy = py as f64 - pad as f64 - half_h + 0.5;
            let t = sd_rounded_box(ix, iy, half_w, half_h, radius);
            let a = coverage(t, inner_band, inner_ramp, outer_fade);
            if a <= 0.0 {
                continue;
            }
            let a256 = (a * 255.0).round() as u32;
            let off = (py * stride + px * 4) as usize;
            // UpdateLayeredWindow 要求**预乘 alpha** 的 BGRA
            buf[off] = ((sb as u32 * a256) / 255) as u8;
            buf[off + 1] = ((sg as u32 * a256) / 255) as u8;
            buf[off + 2] = ((sr as u32 * a256) / 255) as u8;
            buf[off + 3] = a256 as u8;
        }
    }

    SelectObject(dc, bmp.into());
    ReleaseDC(None, screen_dc);
    Some(Surface { dc, bmp, w, h, scale, pad })
}

/// 主窗可见矩形（屏幕坐标）。无边框窗口外面有一圈看不见的缩放热区，
/// 直接用 GetWindowRect 会算错，所以优先取 DWMWA_EXTENDED_FRAME_BOUNDS。
unsafe fn visible_rect(hwnd: HWND) -> Option<RECT> {
    let mut fb = RECT::default();
    let ok = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut fb as *mut RECT as *mut c_void,
        size_of::<RECT>() as u32,
    )
    .is_ok();
    if ok && fb.right > fb.left && fb.bottom > fb.top {
        return Some(fb);
    }
    let mut r = RECT::default();
    if GetWindowRect(hwnd, &mut r).is_ok() {
        Some(r)
    } else {
        None
    }
}

unsafe fn hide(st: &Shared, ring: HWND) {
    if st.shown.swap(false, Ordering::Relaxed) {
        let _ = ShowWindow(ring, SW_HIDE);
    }
}

unsafe fn sync(st: &Shared, ring: HWND, surf: &mut Option<Surface>) {
    let owner = st.owner();
    if !IsWindow(Some(owner)).as_bool() {
        hide(st, ring);
        return;
    }
    // owned 窗永远压在 owner 之上，主窗一失焦描边就会浮到别的窗口上面 —— 所以只在
    // 主窗是前台窗口时才显示。比监听 WM_NCACTIVATE 更省心：能自愈，不依赖消息送达。
    let fg = GetForegroundWindow();
    let active = fg == owner || IsChild(fg, owner).as_bool();
    let ok = active
        && st.want_visible.load(Ordering::Relaxed)
        && IsWindowVisible(owner).as_bool()
        && !IsIconic(owner).as_bool()
        && !IsZoomed(owner).as_bool(); // 最大化时是直角，不需要描边
    if !ok {
        hide(st, ring);
        return;
    }
    let Some(rc) = visible_rect(owner) else {
        hide(st, ring);
        return;
    };
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    if w <= 0 || h <= 0 {
        hide(st, ring);
        return;
    }
    let scale = (GetDpiForWindow(owner) as f64 / 96.0).max(1.0);
    let stale = match surf {
        Some(s) => s.w != w || s.h != h || (s.scale - scale).abs() > 0.001,
        None => true,
    };
    if stale {
        if let Some(old) = surf.take() {
            old.free();
        }
        match build_surface(w, h, scale) {
            Some(s) => *surf = Some(s),
            None => {
                hide(st, ring);
                return;
            }
        }
    }
    let s = match surf.as_ref() {
        Some(s) => s,
        None => {
            hide(st, ring);
            return;
        }
    };

    let top_left = POINT { x: rc.left - s.pad, y: rc.top - s.pad };
    let size = SIZE { cx: s.w + s.pad * 2, cy: s.h + s.pad * 2 };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let screen_dc = GetDC(None);
    let _ = UpdateLayeredWindow(
        ring,
        Some(screen_dc),
        Some(&top_left),
        Some(&size),
        Some(s.dc),
        Some(&src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );
    ReleaseDC(None, screen_dc);

    // 必须先 UpdateLayeredWindow 再 Show，否则第一帧是空白
    if !st.shown.swap(true, Ordering::Relaxed) {
        let _ = ShowWindow(ring, SW_SHOWNOACTIVATE);
    }
}

fn wake(st: &Shared) {
    let tid = st.thread.load(Ordering::Relaxed);
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, WM_SYNC, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe extern "system" fn owner_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if let Some(st) = SHARED.get() {
        match msg {
            // 拖动/缩放由 DWM 合成，伴随窗只能事后追，会甩出一帧 —— 拖动中直接藏起来
            WM_ENTERSIZEMOVE => {
                st.want_visible.store(false, Ordering::Relaxed);
                wake(st);
            }
            WM_EXITSIZEMOVE => {
                st.want_visible.store(true, Ordering::Relaxed);
                wake(st);
            }
            WM_WINDOWPOSCHANGED | WM_SIZE | WM_DPICHANGED => wake(st),
            WM_DESTROY => {
                st.quit.store(true, Ordering::Relaxed);
                wake(st);
            }
            _ => {}
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn frame_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn register_frame_class(hinstance: HINSTANCE) {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(frame_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: HICON::default(),
        hCursor: HCURSOR::default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: windows::core::PCWSTR::null(),
        lpszClassName: w!("TerryComfyRoundFrame"),
        hIconSm: HICON::default(),
    };
    let _ = RegisterClassExW(&class);
}

fn frame_thread(st: Arc<Shared>) {
    unsafe {
        let hinstance = HINSTANCE(GetModuleHandleW(None).unwrap_or_default().0);
        register_frame_class(hinstance);

        let ring = match CreateWindowExW(
            // TOPMOST：保证压在主窗之上；TRANSPARENT+NOACTIVATE：不吃鼠标、不抢焦点
            WS_EX_TOPMOST
                | WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE,
            w!("TerryComfyRoundFrame"),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            Some(st.owner()),
            None,
            Some(hinstance),
            None,
        ) {
            Ok(h) => h,
            Err(_) => return,
        };

        st.thread.store(GetCurrentThreadId(), Ordering::Relaxed);
        SetTimer(Some(ring), TIMER_ID, TIMER_MS, None);

        let mut surf: Option<Surface> = None;
        sync(&st, ring, &mut surf);

        let mut msg = MSG::default();
        loop {
            if st.quit.load(Ordering::Relaxed) {
                break;
            }
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                break;
            }
            if msg.message == WM_TIMER || msg.message == WM_SYNC {
                sync(&st, ring, &mut surf);
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        st.shown.store(false, Ordering::Relaxed);
        if let Some(s) = surf.take() {
            s.free();
        }
        let _ = DestroyWindow(ring);
    }
}

/// Win10 的 region 硬裁剪（保留原方案，锯齿交给描边窗去盖）。
/// 半径按 DPI 换算，与 CSS 的逻辑像素保持一致。
unsafe fn apply_region(hwnd: HWND) {
    if IsZoomed(hwnd).as_bool() {
        let _ = SetWindowRgn(hwnd, None, true);
        return;
    }
    let mut outer = RECT::default();
    if GetWindowRect(hwnd, &mut outer).is_err() {
        return;
    }
    let Some(fb) = visible_rect(hwnd) else { return };
    let w = fb.right - fb.left;
    let h = fb.bottom - fb.top;
    if w <= 0 || h <= 0 {
        return;
    }
    // region 用窗口自身坐标系；right/bottom 是开区间，(l+w, t+h) 刚好覆盖可见矩形
    let l = fb.left - outer.left;
    let t = fb.top - outer.top;
    let scale = (GetDpiForWindow(hwnd) as f64 / 96.0).max(1.0);
    let radius = (CORNER_RADIUS as f64 * scale).round() as i32;
    let rgn = CreateRoundRectRgn(l, t, l + w, t + h, radius * 2, radius * 2);
    if !rgn.is_invalid() {
        // 成功后系统接管 region，不可再 DeleteObject
        if SetWindowRgn(hwnd, Some(rgn), true) == 0 {
            let _ = DeleteObject(rgn.into());
        }
    }
}

/// 调试开关：设 `TERRY_NO_ROUND_FRAME=1` 就只保留 region（退回原来的锯齿），
/// 用来在同一台机器上做 A/B 像素对比，不用重新编译。
fn frame_enabled() -> bool {
    std::env::var("TERRY_NO_ROUND_FRAME").map(|v| v != "1").unwrap_or(true)
}

/// 对外入口：在 setup 和每次 Resized / 焦点变化时调用。
pub fn apply_rounded_corners(window: &tauri::WebviewWindow) {
    let Ok(h) = window.hwnd() else { return };
    let hwnd = HWND(h.0 as *mut c_void);

    unsafe {
        // Win11：DWM 原生圆角，成功即由系统接管，不需要 region 也不需要描边窗
        let pref: u32 = DWMWCP_ROUND;
        let hr = DwmSetWindowAttribute(
            hwnd,
            DWMWINDOWATTRIBUTE(DWMWA_WINDOW_CORNER_PREFERENCE as i32),
            &pref as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        );
        if hr.is_ok() {
            if let Some(st) = SHARED.get() {
                st.quit.store(true, Ordering::Relaxed);
                wake(st);
            }
            return;
        }

        apply_region(hwnd);
        if !frame_enabled() {
            return;
        }

        let st = SHARED.get_or_init(|| {
            let s = Arc::new(Shared::new(hwnd));
            let s2 = Arc::clone(&s);
            thread::spawn(move || frame_thread(s2));
            s
        });
        st.owner.store(hwnd.0 as isize, Ordering::Relaxed);
        if !st.subclassed.swap(true, Ordering::Relaxed) {
            let _ = SetWindowSubclass(hwnd, Some(owner_subclass), SUBCLASS_ID, 0);
        }
        wake(st);
    }
}
