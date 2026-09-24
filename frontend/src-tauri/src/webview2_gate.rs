// WebView2 缺失时的原生兜底。
//
// 关键点：Tauri 的界面**就是** WebView2 本身。组件缺失时窗口能建出来，但内容
// 渲染不出来 —— 用户看到的就是一个白屏，无从排查。所以必须在创建 WebView
// 之前先检测；若缺失，改用纯 Win32 窗口（不依赖 WebView2）显示说明和按钮。
//
// 该模块只在 Windows 编译，且刻意不引用任何 tauri / wry 的东西。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, FONT_CHARSET, FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION,
    FONT_QUALITY, HFONT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetDlgCtrlID,
    GetMessageW, GetSystemMetrics, LoadCursorW, PostQuitMessage, RegisterClassW, SendMessageW,
    SetWindowPos, TranslateMessage, BS_DEFPUSHBUTTON, HMENU, IDC_ARROW, MSG, SM_CXSCREEN,
    SM_CYSCREEN, SWP_NOZORDER, SWP_SHOWWINDOW, SW_SHOWNORMAL, WINDOW_STYLE, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORSTATIC, WM_DESTROY, WM_SETFONT, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_MINIMIZEBOX,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

/// WebView2 Evergreen Runtime 在 EdgeUpdate 下的 client GUID
const WV2_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
/// 官方在线引导程序文件名；构建脚本会尝试下载一份放到 exe 同目录，
/// 这样离线机器上点“立即安装”也能直接装
const BOOTSTRAPPER: &str = "MicrosoftEdgeWebview2Setup.exe";
const DOWNLOAD_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

const ID_INSTALL: i32 = 1001;
const ID_EXIT: i32 = 1002;
const ID_HINT: i32 = 1003;

/// 用户点了哪个按钮（0=未点/关窗，1=安装，2=退出）
static CHOICE: AtomicU32 = AtomicU32::new(0);
/// 背景画刷（在窗口过程里要作为 LRESULT 返回，只能是原始句柄值）
static BG_BRUSH: AtomicIsize = AtomicIsize::new(0);

// ---------- 检测 ----------

fn reg_value_present(root: HKEY, subkey: &str, value: &str) -> bool {
    unsafe {
        let mut key = HKEY::default();
        let sk = HSTRING::from(subkey);
        if RegOpenKeyExW(root, PCWSTR(sk.as_ptr()), None, KEY_READ, &mut key).0 != 0 {
            return false;
        }
        let vn = HSTRING::from(value);
        let mut len: u32 = 0;
        let ok = RegQueryValueExW(
            key,
            PCWSTR(vn.as_ptr()),
            None,
            None,
            None,
            Some(&mut len),
        )
        .0 == 0
            && len > 0;
        let _ = RegCloseKey(key);
        ok
    }
}

/// EdgeWebView\Application\<version>\msedgewebview2.exe
fn dir_has_runtime(base: &Path) -> bool {
    let app = base.join("Microsoft").join("EdgeWebView").join("Application");
    let Ok(rd) = std::fs::read_dir(&app) else { return false };
    rd.flatten()
        .any(|e| e.path().join("msedgewebview2.exe").exists())
}

/// 调试/验证开关：`TERRY_SIMULATE_NO_WEBVIEW2=1` 可强制走缺失分支，
/// 用来在装有 WebView2 的机器上预览这个提示窗口（不会真的改动系统）。
pub fn force_missing() -> bool {
    matches!(
        std::env::var("TERRY_SIMULATE_NO_WEBVIEW2").ok().as_deref(),
        Some("1" | "true" | "yes")
    )
}

/// 注册表 + 安装目录双路检测，任一命中即认为可用。
/// 只查注册表在部分机器上会漏（用户级安装 / 注册表被清理），
/// 只查目录又会漏掉非标准路径，所以两条都走。
pub fn webview2_installed() -> bool {
    let g = WV2_GUID;
    for (root, sub) in [
        (HKEY_LOCAL_MACHINE, format!("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{g}")),
        (HKEY_LOCAL_MACHINE, format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{g}")),
        (HKEY_CURRENT_USER, format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{g}")),
    ] {
        if reg_value_present(root, &sub, "pv") {
            return true;
        }
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Ok(p) = std::env::var(var) {
            if dir_has_runtime(Path::new(&p)) {
                return true;
            }
        }
    }
    false
}

// ---------- 原生窗口 ----------

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_default()
}

unsafe fn make_font(px: i32, weight: i32) -> HFONT {
    CreateFontW(
        -px,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        FONT_CHARSET(0),          // DEFAULT_CHARSET
        FONT_OUTPUT_PRECISION(0), // OUT_DEFAULT_PRECIS
        FONT_CLIP_PRECISION(0),   // CLIP_DEFAULT_PRECIS
        FONT_QUALITY(0),          // DEFAULT_QUALITY
        0,                        // DEFAULT_PITCH | FF_DONTCARE
        w!("Microsoft YaHei UI"),
    )
}

unsafe fn add_label(
    parent: HWND,
    text: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: HFONT,
) -> HWND {
    let cls = w!("STATIC");
    let cap = HSTRING::from(text);
    // 子控件的 ID 就是 CreateWindowExW 的 hMenu 参数
    let hwnd = CreateWindowExW(
        Default::default(),
        PCWSTR(cls.as_ptr()),
        PCWSTR(cap.as_ptr()),
        WS_CHILD | WS_VISIBLE,
        x,
        y,
        w,
        h,
        Some(parent),
        Some(HMENU(id as *mut core::ffi::c_void)),
        None,
        None,
    )
    .unwrap_or_default();
    if hwnd.is_invalid() {
        return hwnd;
    }
    let _ = SendMessageW(
        hwnd,
        WM_SETFONT,
        Some(WPARAM(font.0 as usize)),
        Some(LPARAM(1)),
    );
    hwnd
}

unsafe fn add_button(
    parent: HWND,
    text: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: HFONT,
    default: bool,
) -> HWND {
    let cls = w!("BUTTON");
    let cap = HSTRING::from(text);
    let mut style = (WS_CHILD | WS_VISIBLE | WS_TABSTOP).0;
    if default {
        style |= BS_DEFPUSHBUTTON as u32;
    }
    let hwnd = CreateWindowExW(
        Default::default(),
        PCWSTR(cls.as_ptr()),
        PCWSTR(cap.as_ptr()),
        WINDOW_STYLE(style),
        x,
        y,
        w,
        h,
        Some(parent),
        Some(HMENU(id as *mut core::ffi::c_void)),
        None,
        None,
    )
    .unwrap_or_default();
    if hwnd.is_invalid() {
        return hwnd;
    }
    let _ = SendMessageW(
        hwnd,
        WM_SETFONT,
        Some(WPARAM(font.0 as usize)),
        Some(LPARAM(1)),
    );
    hwnd
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            if id == ID_INSTALL || id == ID_EXIT {
                CHOICE.store(if id == ID_INSTALL { 1 } else { 2 }, Ordering::SeqCst);
                PostQuitMessage(0);
                return LRESULT(0);
            }
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
            windows::Win32::Graphics::Gdi::SetBkMode(
                hdc,
                windows::Win32::Graphics::Gdi::TRANSPARENT,
            );
            // 底部提示行用灰色，其余保持默认黑字
            if GetDlgCtrlID(HWND(lparam.0 as *mut core::ffi::c_void)) == ID_HINT {
                windows::Win32::Graphics::Gdi::SetTextColor(hdc, COLORREF(0x00808080));
            }
            LRESULT(BG_BRUSH.load(Ordering::SeqCst))
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_CLOSE => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn open_url_or_file(file: Option<&str>, url: Option<&str>) -> bool {
    unsafe {
        let target = HSTRING::from(file.unwrap_or_else(|| url.unwrap_or("")));
        let r = ShellExecuteW(
            Some(HWND(std::ptr::null_mut())),
            w!("open"),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        r.0 as usize as i32 > 32
    }
}

/// 启动安装：优先用 exe 同目录自带的引导程序（离线也能装），
/// 没有则打开官方下载页。
fn start_install() {
    let local = exe_dir().join(BOOTSTRAPPER);
    if local.exists() {
        if let Some(s) = local.to_str() {
            if open_url_or_file(Some(s), None) {
                return;
            }
        }
    }
    let _ = open_url_or_file(None, Some(DOWNLOAD_URL));
}

/// 显示说明窗口并阻塞到用户做出选择。返回 true 表示用户可以重试（已触发安装）。
pub fn show_missing_gate() -> bool {
    unsafe {
        // 让窗口在高 DPI 下不被拉虚（失败也无所谓，Win7 上不支持）
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let dpi = GetDpiForSystem();
        let s = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
        let px = |v: i32| -> i32 { (v as f32 * s).round() as i32 };

        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let bg = CreateSolidBrush(COLORREF(0x00F7F3F2)); // #F2F3F7，与主界面同色
        BG_BRUSH.store(bg.0 as isize, Ordering::SeqCst);

        let cls_name = w!("TerryComfyGate");
        let mut wc = WNDCLASSW::default();
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinst.into();
        wc.lpszClassName = PCWSTR(cls_name.as_ptr());
        wc.hbrBackground = bg;
        wc.hCursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let _ = RegisterClassW(&wc);

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE;
        let mut rc = RECT { left: 0, top: 0, right: px(540), bottom: px(252) };
        let _ = AdjustWindowRect(&mut rc, style, false);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        let x = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - h) / 2;

        let title = HSTRING::from("TerryComfy启动器");
        let hwnd = CreateWindowExW(
            Default::default(),
            PCWSTR(cls_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            style,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default();
        if hwnd.is_invalid() {
            return false;
        }

        let f_title = make_font(px(16), 600);
        let f_body = make_font(px(13), 400);
        let f_hint = make_font(px(12), 400);
        let f_btn = make_font(px(13), 400);

        add_label(
            hwnd,
            "缺少 WebView2 运行时",
            2001,
            px(28),
            px(22),
            px(484),
            px(26),
            f_title,
        );
        add_label(
            hwnd,
            "启动器的界面需要 Windows 的 WebView2 组件才能显示。",
            2002,
            px(28),
            px(58),
            px(484),
            px(22),
            f_body,
        );
        add_label(
            hwnd,
            "你的电脑上没有安装它，常见于精简版系统、LTSC 或长期离线的机器。",
            2003,
            px(28),
            px(80),
            px(484),
            px(22),
            f_body,
        );
        add_label(
            hwnd,
            "点击下方按钮安装，装好后重新打开本程序即可。",
            2004,
            px(28),
            px(102),
            px(484),
            px(22),
            f_body,
        );
        add_label(
            hwnd,
            "手动下载地址：go.microsoft.com/fwlink/p/?LinkId=2124703",
            ID_HINT,
            px(28),
            px(134),
            px(484),
            px(20),
            f_hint,
        );
        add_button(
            hwnd,
            "立即安装",
            ID_INSTALL,
            px(346),
            px(190),
            px(96),
            px(32),
            f_btn,
            true,
        );
        add_button(
            hwnd,
            "退出",
            ID_EXIT,
            px(450),
            px(190),
            px(64),
            px(32),
            f_btn,
            false,
        );

        // 确保窗口可见（主窗口已在 style 里带 WS_VISIBLE，这里再兜一次）
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            w,
            h,
            SWP_NOZORDER | SWP_SHOWWINDOW,
        );

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let install = CHOICE.load(Ordering::SeqCst) == 1;
        if install {
            start_install();
        }
        install
    }
}
