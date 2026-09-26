//! Windows 窗口外形：圆角 + 抗锯齿描边。
//!
//! 为什么不直接把主窗做成 transparent：那样虽然能拿到 WebView 的 alpha 圆角，
//! 但会废掉「不透明窗口 + set_background_color」这一套去白闪方案，而且中文字体的
//! ClearType 子像素抗锯齿有可能退化成灰度。所以这里沿用不透明主窗。
//!
//! Win11：`DWMWA_WINDOW_CORNER_PREFERENCE`，DWM 原生抗锯齿圆角，直接交给系统。
//!
//! Win10：该属性不被支持，只能 `SetWindowRgn` 硬裁剪 —— 区域是 **1-bit 掩码**，
//! 圆角必然是锯齿台阶。region 与 layered 伴随窗使用同一套像素中心几何，
//! 用 8-bit alpha 沿同一条圆角路径描一圈边：不透明带完全包住 region 的台阶，
//! 外侧淡出到桌面。轮廓从此由这一层定义，锯齿被完全盖在下面。
//!
//! 关键约束：主窗不能用 `UpdateLayeredWindow`（它要求整个窗口是一张位图，WebView
//! 是子窗口，一进去就没了）；而伴随窗没有子窗口，可以放心用 layered。
//!
//! ## 描边样式来自 CSS
//! Rust 读不到 CSS，所以由前端读 `getComputedStyle` 拿到 `--win-stroke` /
//! `--win-stroke-size`，再通过 `frame_set_style` 命令下发（见 styles.css 的 :root）。
//! 前端没下发 / 解析失败时用下面的常量兜底，所以窗口轮廓永远有值。
//!
//! frame_geometry 的回归测试验证各 DPI 下裁剪不会进入内外羽化带，
//! 并将 4×4 子像素覆盖率与 32×32 面积采样参考值比较。

#![cfg(target_os = "windows")]

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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

// 裁剪和描边必须共用圆角几何，不能分别采用 GDI 和浮点圆弧。
#[path = "frame_geometry.rs"]
mod frame_geometry;
use frame_geometry::FrameGeometry;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;

/// 伴随窗比主窗外扩的逻辑像素数，给外侧淡出留出空间
const PAD: f64 = 4.0;
/// 兜底样式：前端没下发时的值，与 styles.css 的 :root 保持一致
const STROKE_DEFAULT: (u8, u8, u8) = (114, 123, 157);
const STROKE_WIDTH_DEFAULT: f64 = 1.0;
/// 线粗的合理区间（逻辑像素）
const WIDTH_MIN: f64 = 0.5;
const WIDTH_MAX: f64 = 6.0;

/// 自定义线程消息：让伴随窗线程立刻同步一次（不必等下一拍 timer）
const WM_SYNC: u32 = WM_APP + 1;
/// 主窗 subclass 的 id
const SUBCLASS_ID: usize = 1;
const TIMER_ID: usize = 1;
/// 兜底校正间隔（毫秒）。移动/缩放有 subclass 实时通知，这个只是防漏。
const TIMER_MS: u32 = 80;
/// 拖动/缩放过程中的校正间隔：这时没有别的兜底，跟紧一点才不会甩尾
const TIMER_MS_DRAG: u32 = 16;

/// 描边样式。必须是**不透明**的颜色 —— 只有不透明像素才能盖住 region 漏出来的桌面。
#[derive(Clone, Copy, PartialEq)]
struct Style {
    r: u8,
    g: u8,
    b: u8,
    /// 逻辑像素：内侧不透明带的厚度，也就是肉眼看到的线粗
    width: f64,
}

impl Style {
    const fn fallback() -> Self {
        let (r, g, b) = STROKE_DEFAULT;
        Self { r, g, b, width: STROKE_WIDTH_DEFAULT }
    }
}

static STYLE: Mutex<Style> = Mutex::new(Style::fallback());
/// 每次改样式自增，用来让伴随窗知道位图该重算了
static STYLE_REV: AtomicU32 = AtomicU32::new(0);

fn current_style() -> (Style, u32) {
    let s = STYLE.lock().map(|s| *s).unwrap_or_else(|_| Style::fallback());
    let rev = STYLE_REV.load(Ordering::Relaxed);
    (s, rev)
}

fn hex2(s: &str) -> Option<u8> {
    u8::from_str_radix(s, 16).ok()
}

fn num(s: &str) -> Option<f64> {
    s.trim().trim_end_matches('%').trim().parse::<f64>().ok()
}

/// 解析 CSS 颜色：`#rgb` / `#rrggbb` / `#rrggbbaa` / `rgb(...)` / `rgba(...)`。
/// 自定义属性经 getComputedStyle 取出来通常是原始写法（`#727b9d`），
/// 但 `color-mix()` 之类会被算成 `rgb(...)`，所以两种都要认。
fn parse_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(h) = s.strip_prefix('#') {
        let h: String = h.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        return match h.len() {
            3 => Some((
                hex2(&h[0..1].repeat(2))?,
                hex2(&h[1..2].repeat(2))?,
                hex2(&h[2..3].repeat(2))?,
            )),
            6 | 8 => Some((hex2(&h[0..2])?, hex2(&h[2..4])?, hex2(&h[4..6])?)),
            _ => None,
        };
    }
    let open = s.find('(')? + 1;
    let close = s.find(')')?;
    let parts: Vec<&str> = s[open..close].split(',').collect();
    if parts.len() < 3 {
        return None;
    }
    let clamp = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    Some((clamp(num(parts[0])?), clamp(num(parts[1])?), clamp(num(parts[2])?)))
}

/// 解析长度：`1px` / `1.5px` / `2`（单位可省略，缺单位按 px 算）
fn parse_len(s: &str) -> Option<f64> {
    let t = s.trim().trim_end_matches("px").trim().trim_end_matches("pt").trim();
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// 前端下发 CSS 变量里的描边样式（见 styles.css 的 `--win-stroke`）。
/// 颜色和宽度都可以只给一个，没给的保持原值。
pub fn set_style(color: &str, size: &str) {
    let Ok(mut cur) = STYLE.lock() else { return };
    let mut changed = false;
    if let Some((r, g, b)) = parse_color(color) {
        if (r, g, b) != (cur.r, cur.g, cur.b) {
            cur.r = r;
            cur.g = g;
            cur.b = b;
            changed = true;
        }
    }
    if let Some(w) = parse_len(size) {
        let w = w.clamp(WIDTH_MIN, WIDTH_MAX);
        if (w - cur.width).abs() > 0.001 {
            cur.width = w;
            changed = true;
        }
    }
    drop(cur);
    if changed {
        STYLE_REV.fetch_add(1, Ordering::Relaxed);
        if let Some(st) = SHARED.get() {
            wake(st);
        }
    }
}

struct Shared {
    owner: AtomicIsize,
    thread: AtomicU32,
    /// 主窗自己是否允许显示描边（拖动中 / 刚失焦时置 false）
    want_visible: AtomicBool,
    /// 伴随窗当前是否真的显示着
    shown: AtomicBool,
    subclassed: AtomicBool,
    /// 正在拖动/缩放窗口
    moving: AtomicBool,
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
            moving: AtomicBool::new(false),
            quit: AtomicBool::new(false),
        }
    }
    fn owner(&self) -> HWND {
        HWND(self.owner.load(Ordering::Relaxed) as *mut c_void)
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// 伴随窗的位图。只在主窗尺寸、DPI 或描边样式变化时才重新计算像素，移动时复用。
struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    previous: HGDIOBJ,
    w: i32,
    h: i32,
    scale: f64,
    pad: i32,
    rev: u32,
}

impl Surface {
    unsafe fn free(self) {
        SelectObject(self.dc, self.previous);
        let _ = DeleteObject(self.bmp.into());
        let _ = DeleteDC(self.dc);
    }
}

unsafe fn build_surface(w: i32, h: i32, scale: f64, style: Style, rev: u32) -> Option<Surface> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let pad = (PAD * scale).round() as i32;
    let rw = w + pad * 2;
    let rh = h + pad * 2;
    let geometry = FrameGeometry::new(w, h, scale, style.width);
    let (sr, sg, sb) = (style.r, style.g, style.b);

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
    buf.fill(0);
    // 只处理边缘条带。移动仍复用位图，超采样只发生在尺寸/DPI/样式变化时。
    let edge = (geometry.radius + WIDTH_MAX * scale + 2.0).ceil() as i32 + pad;

    for py in 0..rh {
        let mut px = 0;
        while px < rw {
            if py >= edge && py < rh - edge && px >= edge && px < rw - edge {
                px = rw - edge;
            }
            let a = geometry.pixel_alpha(px - pad, py - pad);
            let off = (py * stride + px * 4) as usize;
            px += 1;
            if a <= 0.0 {
                continue;
            }
            let a256 = (a * 255.0).round() as u32;
            // UpdateLayeredWindow 要求**预乘 alpha** 的 BGRA
            buf[off] = ((sb as u32 * a256) / 255) as u8;
            buf[off + 1] = ((sg as u32 * a256) / 255) as u8;
            buf[off + 2] = ((sr as u32 * a256) / 255) as u8;
            buf[off + 3] = a256 as u8;
        }
    }

    let previous = SelectObject(dc, bmp.into());
    ReleaseDC(None, screen_dc);
    Some(Surface { dc, bmp, previous, w, h, scale, pad, rev })
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

/// 拖动窗口时系统是否实时绘制窗口内容。为 false 时系统只画一个橡皮筋框、
/// 窗口到松手才真的移动，那时描边跟无可跟，只能藏起来。
unsafe fn drag_shows_content() -> bool {
    let mut v = windows::core::BOOL(0);
    let ok = SystemParametersInfoW(
        SPI_GETDRAGFULLWINDOWS,
        0,
        Some(&mut v as *mut windows::core::BOOL as *mut c_void),
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    )
    .is_ok();
    !ok || v.as_bool() // 查不到就按「实时绘制」处理
}

unsafe fn sync(
    st: &Shared,
    ring: HWND,
    surf: &mut Option<Surface>,
    last: &mut Option<(POINT, SIZE)>,
) {
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
    let (style, rev) = current_style();
    let stale = match surf {
        Some(s) => s.w != w || s.h != h || (s.scale - scale).abs() > 0.001 || s.rev != rev,
        None => true,
    };
    if stale {
        if let Some(old) = surf.take() {
            old.free();
        }
        match build_surface(w, h, scale, style, rev) {
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
    let (first, moved, resized) = match last {
        Some((p, z)) => (
            false,
            p.x != top_left.x || p.y != top_left.y,
            z.cx != size.cx || z.cy != size.cy,
        ),
        None => (true, true, true),
    };

    // 注意：`stale` 也要算进来。改了描边样式时几何完全没变，只是位图内容换了新的
    // —— 那一轮必须重新提交，否则窗口上贴的还是旧位图（改颜色看不到效果）。
    let shown = st.shown.load(Ordering::Relaxed);
    if first || resized || stale || !shown {
        // 位图变了（或从隐藏恢复）：走 UpdateLayeredWindow 重传整张位图
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
    } else if moved {
        // 只是跟着主窗挪：位图没变，用 SetWindowPos 搬过去即可。
        // 走 UpdateLayeredWindow 每帧要重传几 MB，拖动会卡。
        let _ = SetWindowPos(
            ring,
            None,
            top_left.x,
            top_left.y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
        );
    }
    *last = Some((top_left, size));
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
            // 拖动/缩放期间的三种情况：
            //  - 系统实时绘制窗口内容（默认）→ 跟着走，全程保持抗锯齿
            //  - 系统只画橡皮筋框 → 窗口根本没动，描边会钉在原地，只能藏
            //  - TERRY_FRAME_DRAG=hide → 强制退回「拖动时隐藏」
            WM_ENTERSIZEMOVE => {
                st.moving.store(true, Ordering::Relaxed);
                let follow = match std::env::var("TERRY_FRAME_DRAG").as_deref() {
                    Ok("hide") => false,
                    Ok("follow") => true,
                    _ => drag_shows_content(),
                };
                st.want_visible.store(follow, Ordering::Relaxed);
                wake(st);
            }
            WM_EXITSIZEMOVE => {
                st.moving.store(false, Ordering::Relaxed);
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
        let mut last: Option<(POINT, SIZE)> = None;
        let mut period = TIMER_MS;
        sync(&st, ring, &mut surf, &mut last);

        let mut msg = MSG::default();
        loop {
            if st.quit.load(Ordering::Relaxed) {
                break;
            }
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                break;
            }
            if msg.message == WM_TIMER || msg.message == WM_SYNC {
                sync(&st, ring, &mut surf, &mut last);
                let want = if st.moving.load(Ordering::Relaxed) {
                    TIMER_MS_DRAG
                } else {
                    TIMER_MS
                };
                if want != period {
                    period = want;
                    SetTimer(Some(ring), TIMER_ID, period, None);
                }
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

/// Win10 的 region 硬裁剪，边界落在描边的全不透明带中。
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
    let geometry = FrameGeometry::new(w, h, scale, STROKE_WIDTH_DEFAULT);
    let rgn = CreateRectRgn(0, 0, 0, 0);
    if !rgn.is_invalid() {
        // 合并连续且相同的扫描行，直边只需一个矩形。
        let mut y = 0;
        while y < h {
            let span = geometry.row_span(y);
            let start = y;
            y += 1;
            while y < h && geometry.row_span(y) == span {
                y += 1;
            }
            if let Some((left, right)) = span {
                let strip = CreateRectRgn(l + left, t + start, l + right, t + y);
                if strip.is_invalid() {
                    let _ = DeleteObject(rgn.into());
                    return;
                }
                let result = CombineRgn(Some(rgn), Some(rgn), Some(strip), RGN_OR);
                let _ = DeleteObject(strip.into());
                if result == RGN_ERROR {
                    let _ = DeleteObject(rgn.into());
                    return;
                }
            }
        }
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
