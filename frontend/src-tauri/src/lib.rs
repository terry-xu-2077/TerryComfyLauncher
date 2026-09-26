use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Window};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
mod webview2_gate;
#[cfg(target_os = "windows")]
mod rounded_frame;

// ---------- global state ----------
struct LauncherState {
    comfy_child: Mutex<Option<Child>>,
    comfy_state: Mutex<String>, // stopped | starting | running
    busy: Mutex<bool>,          // one long op at a time
    /// set while the user (or app exit) is deliberately stopping ComfyUI,
    /// so the exit watcher does not report it as a crash
    stopping: Mutex<bool>,
    /// Windows job object handle keeping the child tree tied to this process
    job: Mutex<isize>,
    /// increments on every launch; watcher threads of an older launch stop
    /// touching state once a newer launch has taken over
    gen: Mutex<u64>,
}

impl LauncherState {
    fn new() -> Self {
        Self {
            comfy_child: Mutex::new(None),
            comfy_state: Mutex::new("stopped".to_string()),
            busy: Mutex::new(false),
            stopping: Mutex::new(false),
            job: Mutex::new(0),
            gen: Mutex::new(0),
        }
    }
}

// ---------- paths & settings ----------
fn exe_dir() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn settings_file() -> PathBuf {
    exe_dir().join("settings.json")
}

fn read_settings() -> Value {
    fs::read_to_string(settings_file())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({"comfyPath": "", "port": 8188}))
}

fn write_settings(patch: &Value) -> Result<(), String> {
    let mut merged = read_settings();
    if let (Some(base), Some(p)) = (merged.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            base.insert(k.clone(), v.clone());
        }
    }
    let text = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())?;
    fs::write(settings_file(), text).map_err(|e| format!("无法写入设置文件: {e}"))
}

fn is_comfy_dir(p: &Path) -> bool {
    p.join("main.py").is_file()
}

/// Auto-detect ComfyUI next to the launcher exe:
/// the exe sits inside the ComfyUI directory itself, or one of its
/// immediate subdirectories is a ComfyUI directory (contains main.py).
fn auto_detect_comfy() -> Option<String> {
    let dir = exe_dir();
    if is_comfy_dir(&dir) {
        return Some(dir.to_string_lossy().into_owned());
    }
    let rd = fs::read_dir(&dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && is_comfy_dir(&p) {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

fn comfy_root() -> String {
    let cur = read_settings()
        .get("comfyPath")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !cur.is_empty() && is_comfy_dir(Path::new(&cur)) {
        return cur;
    }
    // not configured (or configured path is gone) -> try auto-detection
    if let Some(found) = auto_detect_comfy() {
        let _ = write_settings(&json!({ "comfyPath": found }));
        return found;
    }
    cur
}

fn comfy_port() -> u64 {
    read_settings().get("port").and_then(Value::as_u64).unwrap_or(8188)
}

// ---------- events ----------
fn log_line(app: &AppHandle, source: &str, text: &str) {
    let _ = app.emit("log", json!({"source": source, "text": text}));
}

fn set_comfy_state(app: &AppHandle, s: &str) {
    let st = app.state::<LauncherState>();
    *st.comfy_state.lock().unwrap() = s.to_string();
    let _ = app.emit("comfy-state", s);
}

// ---------- process helpers ----------
fn prep(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// ```text
/// 让 ComfyUI 跟着启动器一起结束：把子进程放进一个 KILL_ON_JOB_CLOSE 的 Job Object，
/// 启动器进程一退出（正常关闭、任务管理器结束、崩溃都算），系统会连子孙进程一起收掉，
/// 不会留下孤儿 python.exe。
/// ```
#[cfg(windows)]
mod winjob {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    type Handle = *mut core::ffi::c_void;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct ExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    extern "system" {
        fn CreateJobObjectW(attrs: *mut core::ffi::c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            class: u32,
            info: *const core::ffi::c_void,
            len: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    }

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;

    pub fn create_kill_on_close_job() -> isize {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut info = ExtendedLimitInformation::default();
            info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<ExtendedLimitInformation>() as u32,
            );
            if ok == 0 {
                return 0;
            }
            // 句柄故意不关：进程结束时系统关闭它，同时收掉 Job 里的所有进程
            job as isize
        }
    }

    pub fn assign(job: isize, child: &Child) -> bool {
        if job == 0 {
            return false;
        }
        unsafe { AssignProcessToJobObject(job as Handle, child.as_raw_handle() as Handle) != 0 }
    }
}

#[cfg(windows)]
fn kill_process_tree(pid: u32) {
    let pid = pid.to_string();
    let _ = prep(&mut Command::new("taskkill"))
        .args(["/PID", &pid, "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// ```text
/// 停掉 ComfyUI 及其派生的子进程（自定义节点常会拉起子进程）。
/// 返回 true 表示确实有一个进程被收掉。
/// ```
fn stop_comfy_inner(app: &AppHandle) -> bool {
    let st = app.state::<LauncherState>();
    *st.stopping.lock().unwrap() = true;
    let taken = { st.comfy_child.lock().unwrap().take() };
    match taken {
        Some(mut c) => {
            #[cfg(windows)]
            kill_process_tree(c.id());
            let _ = c.kill();
            let _ = c.wait();
            true
        }
        None => false,
    }
}

/// 启动器退出时调用：绝不留一个还在跑的 ComfyUI 在后台。
fn shutdown_comfy(app: &AppHandle) {
    stop_comfy_inner(app);
}

fn find_python(root: &str) -> String {
    let r = Path::new(root);
    // A Windows venv usually keeps python.exe in `Scripts\`, but venvs created
    // by uv / the portable ComfyUI builds put it at the venv root instead.
    let candidates = [
        r.join("python_embeded").join("python.exe"),
        r.join("..").join("python_embeded").join("python.exe"),
        r.join(".venv").join("python.exe"),
        r.join(".venv").join("Scripts").join("python.exe"),
        r.join("venv").join("python.exe"),
        r.join("venv").join("Scripts").join("python.exe"),
    ];
    for c in candidates {
        if c.exists() {
            return c.to_string_lossy().into_owned();
        }
    }
    "python".to_string()
}

/// ```text
/// 编码：ComfyUI 是在中文 Windows 上用 GBK 编码往管道里写日志的（app/logger.py 的
/// LogInterceptor 会沿用 sys.stdout 的 encoding），直接按 UTF-8 解出来就是一堆 "�"。
/// 所以先严格试 UTF-8，失败再按系统 ANSI 代码页解一次。
/// ```
#[cfg(windows)]
fn decode_ansi(bytes: &[u8]) -> Option<String> {
    extern "system" {
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            s: *const u8,
            cb: i32,
            out: *mut u16,
            cch: i32,
        ) -> i32;
    }
    const CP_ACP: u32 = 0; // 跟随系统 ANSI 代码页（简体中文 = 936 / GBK）
    const MB_ERR_INVALID_CHARS: u32 = 0x8;
    if bytes.is_empty() {
        return Some(String::new());
    }
    unsafe {
        let len = MultiByteToWideChar(
            CP_ACP,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        );
        if len <= 0 {
            return None;
        }
        let mut wide = vec![0u16; len as usize];
        let n = MultiByteToWideChar(
            CP_ACP,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            len,
        );
        if n <= 0 {
            return None;
        }
        wide.truncate(n as usize);
        Some(String::from_utf16_lossy(&wide))
    }
}

fn decode_bytes(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    #[cfg(windows)]
    if let Some(s) = decode_ansi(bytes) {
        return s;
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// ```text
/// ComfyUI 的日志带 ANSI 颜色转义（\x1b[0;32m[INFO]\x1b[0m），日志面板是纯文本，
/// 不处理就会露出 "[0;32m[INFO][0m" 这种乱码；顺手也丢掉其它控制字符。
/// ```
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            match it.next() {
                // CSI: 参数字节后跟一个 @..~ 的结束字节
                Some('[') => {
                    for c2 in it.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c2) {
                            break;
                        }
                    }
                }
                // OSC: 一直读到 BEL 或 ST(ESC \)
                Some(']') => {
                    while let Some(c2) = it.next() {
                        if c2 == '\u{7}' {
                            break;
                        }
                        if c2 == '\u{1b}' {
                            if it.peek().copied() == Some('\\') {
                                it.next();
                            }
                            break;
                        }
                    }
                }
                Some(_) => {}
                None => break,
            }
            continue;
        }
        if c == '\r' || c == '\t' {
            continue;
        }
        if (c as u32) < 0x20 || c == '\u{7f}' {
            continue;
        }
        out.push(c);
    }
    out.trim().to_string()
}

fn clean_line(bytes: &[u8]) -> String {
    strip_ansi(&decode_bytes(bytes))
}

/// ```text
/// ComfyUI 的 logging 输出走 stderr（logging.StreamHandler() 默认绑 sys.stderr），
/// 只有 print() 才走 stdout —— 所以两条管道都要盯，否则界面会一直卡在"启动中"。
/// ```
fn detect_readiness(app: &AppHandle, line: &str) {
    if !(line.contains("To see the GUI go to") || line.contains("Starting server")) {
        return;
    }
    let st = app.state::<LauncherState>();
    let became = {
        let mut cur = st.comfy_state.lock().unwrap();
        if *cur == "starting" {
            *cur = "running".to_string();
            true
        } else {
            false
        }
    };
    if became {
        let _ = app.emit("comfy-state", "running");
        log_line(app, "comfy", "✔ ComfyUI 已启动，可以开始使用了");
    }
}

/// Stream raw bytes from a pipe, emitting one cleaned-up UTF-8 line per log event.
fn stream_lines<R: Read>(mut reader: R, app: AppHandle, source: String, detect_ready: bool) {
    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                // 进度条那种只写 \r 不写 \n 的输出，靠长度上限兜底，避免无限堆积
                if byte[0] == b'\n' || buf.len() > 8192 {
                    let line = clean_line(&buf);
                    buf.clear();
                    if line.is_empty() {
                        continue;
                    }
                    if detect_ready {
                        detect_readiness(&app, &line);
                    }
                    log_line(&app, &source, &line);
                } else {
                    buf.push(byte[0]);
                }
            }
            Err(_) => break,
        }
    }
    if !buf.is_empty() {
        let line = clean_line(&buf);
        if !line.is_empty() {
            log_line(&app, &source, &line);
        }
    }
}

/// Run a command to completion while streaming output as log events.
fn run_stream(app: &AppHandle, source: &str, program: &str, args: &[String], cwd: &Path) -> Result<(), String> {
    log_line(app, source, &format!("$ {} {}", program, args.join(" ")));
    let mut child = prep(&mut Command::new(program))
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法运行 {program}: {e}"))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let app2 = app.clone();
    let src2 = source.to_string();
    let t = std::thread::spawn(move || stream_lines(stderr, app2, src2, false));
    stream_lines(stdout, app.clone(), source.to_string(), false);
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = t.join();
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} 运行失败（退出码 {:?}）", status.code()))
    }
}

/// Run a git command and return trimmed stdout, or None on failure.
/// Always passes safe.directory so ComfyUI folders owned by another
/// user / moved from elsewhere do not trip git's ownership check.
fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    git_output_owned(root, &strs(args))
}

fn strs(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

// ---------- git: resilient helpers ----------
/// models / input / output / user are the folders people commonly replace
/// with junctions or symlinks (this is not universal, so we always probe).
const LINK_DIR_NAMES: [&str; 4] = ["models", "input", "output", "user"];
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

fn git_root_arg(root: &Path) -> String {
    root.to_string_lossy().replace('\\', "/")
}

fn git_base_args(root: &Path) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!("safe.directory={}", git_root_arg(root)),
        "-c".to_string(),
        "http.version=HTTP/1.1".to_string(),
    ]
}

fn git_proxy_args(proxy: &str) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!("http.proxy={proxy}"),
        "-c".to_string(),
        format!("https.proxy={proxy}"),
    ]
}

fn git_output_owned(root: &Path, args: &[String]) -> Option<String> {
    let mut full = git_base_args(root);
    full.extend_from_slice(args);
    let out = prep(&mut Command::new("git"))
        .args(&full)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn git_ok(root: &Path, args: &[&str]) -> bool {
    git_output(root, args).is_some()
}

/// Same as `git_output` but keeps leading whitespace — git's porcelain status
/// lines start with two status columns, and trimming would shift the path.
fn git_output_raw(root: &Path, args: &[String]) -> Option<String> {
    let mut full = git_base_args(root);
    full.extend_from_slice(args);
    let out = prep(&mut Command::new("git"))
        .args(&full)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if out.status.success() {
        Some(
            String::from_utf8_lossy(&out.stdout)
                .trim_end_matches(|c| c == '\n' || c == '\r')
                .to_string(),
        )
    } else {
        None
    }
}

fn git_has_version() -> bool {
    prep(&mut Command::new("git"))
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Proxy candidates used after a couple of direct attempts fail:
/// the machine's own proxy env vars first, then common local proxy ports
/// that actually have something listening.
fn proxy_candidates() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |v: &mut Vec<String>, s: String| {
        if !s.trim().is_empty() && !v.contains(&s) {
            v.push(s)
        }
    };
    for key in [
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
        "http_proxy",
        "HTTP_PROXY",
    ] {
        if let Ok(v) = env::var(key) {
            push(&mut out, v.trim().to_string());
        }
    }
    // common local proxy ports: Clash / v2ray / shadowsocks / etc.
    if out.is_empty() {
        for port in ["7897", "1087", "8889", "1080"] {
            let addr = format!("127.0.0.1:{port}");
            if std::net::TcpStream::connect(&addr).is_ok() {
                push(&mut out, format!("http://{addr}"));
                push(&mut out, format!("socks5h://{addr}"));
            }
        }
    }
    out.truncate(3);
    out
}

/// Attempt plan for network git commands: two direct tries, then one try per proxy.
fn network_attempts() -> Vec<String> {
    let mut v = vec![String::new(), String::new()];
    for p in proxy_candidates() {
        v.push(p);
    }
    v
}

/// Run a network git command (fetch / pull) streaming its output, retrying
/// directly first and falling back to available proxies.
fn git_network(
    app: &AppHandle,
    root: &Path,
    args: &[String],
    failures: &mut Vec<String>,
) -> Result<(), String> {
    let attempts = network_attempts();
    let total = attempts.len();
    for (i, proxy) in attempts.iter().enumerate() {
        let attempt = i + 1;
        if attempt > 1 {
            let sleep = std::cmp::min(3 * i as u64, 8);
            let hint = if proxy.is_empty() {
                format!("第 {attempt}/{total} 次重试（直连）…")
            } else {
                format!("第 {attempt}/{total} 次重试，改用代理 {proxy}…")
            };
            log_line(app, "update", &hint);
            std::thread::sleep(std::time::Duration::from_secs(sleep));
        }
        let mut full = git_base_args(root);
        if !proxy.is_empty() {
            full.extend(git_proxy_args(proxy));
        }
        full.extend_from_slice(args);
        match run_stream(app, "update", "git", &full, root) {
            Ok(()) => {
                if attempt > 1 {
                    log_line(app, "update", "✔ 重试成功");
                }
                return Ok(());
            }
            Err(e) => failures.push(e),
        }
    }
    Err(failures.last().cloned().unwrap_or_else(|| "Git 命令失败".to_string()))
}

fn current_branch(root: &Path) -> Option<String> {
    git_output(root, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| !b.is_empty() && b != "HEAD")
}

fn remote_default_branch(root: &Path) -> Option<String> {
    if let Some(head) = git_output(root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        if let Some(b) = head.strip_prefix("origin/") {
            return Some(b.to_string());
        }
    }
    for candidate in ["master", "main"] {
        if git_ok(root, &["show-ref", "--verify", "--quiet", &format!("refs/remotes/origin/{candidate}")]) {
            return Some(candidate.to_string());
        }
    }
    None
}

/// ComfyUI ships as master, but plenty of people end up in detached HEAD.
fn ensure_on_branch(app: &AppHandle, root: &Path) -> Result<String, String> {
    if let Some(b) = current_branch(root) {
        return Ok(b);
    }
    let default = remote_default_branch(root).unwrap_or_else(|| "master".to_string());
    log_line(app, "update", &format!("检测到游离的 HEAD 状态，切换到 {default} 分支"));
    if git_ok(root, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{default}")]) {
        git_output(root, &["checkout", &default]);
    } else if git_output(root, &["checkout", "-b", &default, "--track", &format!("origin/{default}")]).is_none() {
        return Err(format!("无法切换到 {default} 分支"));
    }
    Ok(default)
}

// ---------- directory links (junctions / symlinks) ----------
fn is_reparse_point(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            }
            #[cfg(not(windows))]
            {
                m.file_type().is_symlink()
            }
        })
        .unwrap_or(false)
}

/// Only the folders that really are links get the special treatment;
/// users who never created a link keep fully standard git behaviour.
fn link_dirs(root: &Path) -> Vec<String> {
    LINK_DIR_NAMES
        .iter()
        .filter(|n| is_reparse_point(&root.join(n)))
        .map(|n| n.to_string())
        .collect()
}

fn path_in_link_dirs(path: &str, links: &[String]) -> bool {
    let p = path.replace('\\', "/");
    links.iter().any(|n| p == *n || p.starts_with(&format!("{n}/")))
}

fn git_ls_files(root: &Path, paths: &[String]) -> Option<Vec<String>> {
    let mut args = strs(&["ls-files", "--"]);
    args.extend_from_slice(paths);
    git_output_owned(root, &args).map(|out| {
        out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
    })
}

/// Tell git to stop watching the placeholder files that live inside links,
/// otherwise every update trips over "local changes would be overwritten".
fn link_skip_worktree(root: &Path, links: &[String]) -> Result<usize, String> {
    if links.is_empty() {
        return Ok(0);
    }
    let tracked = git_ls_files(root, links).unwrap_or_default();
    if tracked.is_empty() {
        return Ok(0);
    }
    let mut args = strs(&["update-index", "--skip-worktree", "--"]);
    args.extend(tracked.iter().cloned());
    if git_output_owned(root, &args).is_some() {
        Ok(tracked.len())
    } else {
        Err("无法让 Git 忽略链接目录下的文件".to_string())
    }
}

fn explain_status(line: &str) -> String {
    if line.len() < 4 {
        return line.to_string();
    }
    let state = &line[..2];
    let path = line[3..].to_string();
    let reason = if state.starts_with("??") {
        "未跟踪文件"
    } else if state.starts_with(" M") || state.starts_with("M ") || state.starts_with("MM") {
        "已修改文件"
    } else if state.starts_with(" A") || state.starts_with("A ") {
        "新增文件"
    } else if state.starts_with(" D") || state.starts_with("D ") {
        "已删除文件"
    } else if state.starts_with(" R") || state.starts_with("R ") {
        "重命名文件"
    } else if state.starts_with(" C") || state.starts_with("C ") {
        "复制文件"
    } else if state.starts_with(" U") || state.starts_with("U ") || state.starts_with("UU") {
        "冲突文件"
    } else {
        "改动文件"
    };
    format!("{reason}：{path}")
}

fn workspace_blockers(root: &Path, links: &[String]) -> Vec<String> {
    let status = git_output_raw(
        root,
        &strs(&["status", "--porcelain", "--untracked-files=no"]),
    )
    .unwrap_or_default();
    status
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !path_in_link_dirs(&l[3.min(l.len())..], links))
        .map(|l| explain_status(l))
        .collect()
}

/// Guard so only one long-running op executes at a time; runs f on a blocking thread.
/// Releases the "one long op at a time" flag even if the task panics,
/// otherwise a single crash would leave the launcher refusing every later task.
struct BusyGuard(AppHandle);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        let st = self.0.state::<LauncherState>();
        *st.busy.lock().unwrap() = false;
    }
}

async fn run_blocking<F>(app: AppHandle, f: F) -> Result<Value, String>
where
    F: FnOnce(AppHandle) -> Result<Value, String> + Send + 'static,
{
    {
        let st = app.state::<LauncherState>();
        let mut busy = st.busy.lock().unwrap();
        if *busy {
            return Err("已有任务正在进行中，请等它完成".to_string());
        }
        *busy = true;
    }
    let _guard = BusyGuard(app.clone());
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || f(app2))
        .await
        .map_err(|e| format!("任务异常结束：{e}"))?
}

// ---------- settings & dialogs ----------
#[tauri::command]
fn get_settings() -> Value {
    read_settings()
}

#[tauri::command]
fn set_settings(settings: Value) -> Result<Value, String> {
    write_settings(&settings)?;
    Ok(read_settings())
}

#[tauri::command]
fn pick_directory(window: Window, title: Option<String>) -> Option<String> {
    let mut dialog = rfd::FileDialog::new();
    dialog = dialog.set_parent(&window);
    if let Some(t) = title {
        dialog = dialog.set_title(&t);
    }
    dialog.pick_folder().map(|p| p.to_string_lossy().into_owned())
}

// ---------- comfy status / launch / stop ----------
#[tauri::command]
fn comfy_status(app: AppHandle) -> Value {
    // refresh state if the child exited on its own
    {
        let st = app.state::<LauncherState>();
        let mut guard = st.comfy_child.lock().unwrap();
        let exited = match guard.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(Some(_))),
            None => false,
        };
        if exited {
            *guard = None;
            drop(guard);
            let stopping = *st.stopping.lock().unwrap();
            let was = {
                let mut cur = st.comfy_state.lock().unwrap();
                let prev = cur.clone();
                *cur = "stopped".to_string();
                prev
            };
            if !stopping && was != "stopped" {
                let _ = app.emit("comfy-state", "stopped");
                if was == "starting" {
                    log_line(&app, "comfy", "✖ ComfyUI 启动失败：进程在启动完成前就退出了，具体原因见上面的日志");
                } else {
                    log_line(&app, "comfy", "ComfyUI 已退出");
                }
            }
        }
    }

    let configured = read_settings()
        .get("comfyPath")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let root = comfy_root();
    if configured != root && is_comfy_dir(Path::new(&root)) {
        log_line(&app, "comfy", &format!("已自动识别 ComfyUI 目录: {root}"));
    }
    let rootp = Path::new(&root);
    let installed = !root.is_empty() && rootp.join("main.py").exists();

    let mut version = Value::Null;
    let mut date = Value::Null;
    let mut node_count = 0usize;
    let mut python_found = false;
    if installed {
        if let Some(v) = git_output(rootp, &["rev-parse", "--short", "HEAD"]) {
            version = json!(v);
        }
        if let Some(d) = git_output(rootp, &["log", "-1", "--format=%cd", "--date=short"]) {
            date = json!(d);
        }
        if let Ok(rd) = fs::read_dir(rootp.join("custom_nodes")) {
            node_count = rd
                .flatten()
                .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
                .count();
        }
        let python = find_python(&root);
        python_found = python == "python" || Path::new(&python).exists();
    }

    let st = app.state::<LauncherState>();
    let state = st.comfy_state.lock().unwrap().clone();
    json!({
        "installed": installed,
        "version": version,
        "date": date,
        "nodeCount": node_count,
        "state": state,
        "pythonFound": python_found,
        "path": root,
        "port": comfy_port(),
    })
}

/// ```text
/// ComfyUI 日志文本可能会变、也可能来得慢，所以除了认文本，再直接探端口：
/// 能连上 127.0.0.1:port 就一定是在跑了。
/// ```
fn watch_port_until_ready(app: AppHandle, gen: u64, port: u16) {
    let addr: Option<std::net::SocketAddr> = format!("127.0.0.1:{port}").parse().ok();
    let Some(addr) = addr else { return };
    for _ in 0..360 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        // 每个锁都单独取、单独放：既不会和 watch_process_exit 的加锁顺序打架，
        // 也不会误改"下一次启动"的状态
        let same_gen = {
            let st = app.state::<LauncherState>();
            let g = *st.gen.lock().unwrap();
            g == gen
        };
        if !same_gen {
            return;
        }
        let still_starting = {
            let st = app.state::<LauncherState>();
            let cur = st.comfy_state.lock().unwrap();
            cur.as_str() == "starting"
        };
        if !still_starting {
            return;
        }
        let alive = {
            let st = app.state::<LauncherState>();
            let mut guard = st.comfy_child.lock().unwrap();
            match guard.as_mut() {
                Some(c) => matches!(c.try_wait(), Ok(None)),
                None => false,
            }
        };
        if !alive {
            return;
        }
        let up = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(400))
            .is_ok();
        if up {
            let same_gen = {
                let st = app.state::<LauncherState>();
                let g = *st.gen.lock().unwrap();
                g == gen
            };
            let st = app.state::<LauncherState>();
            let became = {
                let mut cur = st.comfy_state.lock().unwrap();
                if same_gen && *cur == "starting" {
                    *cur = "running".to_string();
                    true
                } else {
                    false
                }
            };
            if became {
                let _ = app.emit("comfy-state", "running");
                log_line(&app, "comfy", "✔ ComfyUI 已就绪，点击“打开 ComfyUI 界面”开始使用");
            }
            return;
        }
    }
}

/// ```text
/// 两条输出管道都读到 EOF 就说明进程真的结束了 —— 不用等下一次轮询。
/// 顺手把状态归位，并区分"启动失败"和"运行结束"。
/// ```
fn watch_process_exit(app: AppHandle, gen: u64, readers: Vec<std::thread::JoinHandle<()>>) {
    for r in readers {
        let _ = r.join();
    }
    let st = app.state::<LauncherState>();
    let was = {
        let mut child = st.comfy_child.lock().unwrap();
        // 先持 child 再看 gen / stopping（加锁顺序固定为 child → 其它），
        // 这样"取消后立刻重新启动"时，旧 watcher 不会把新会话的状态踩掉
        let taken_over = {
            let same_gen = *st.gen.lock().unwrap() == gen;
            !same_gen || *st.stopping.lock().unwrap()
        };
        if taken_over {
            return;
        }
        if let Some(c) = child.as_mut() {
            let _ = c.wait();
        }
        *child = None;
        let mut cur = st.comfy_state.lock().unwrap();
        let prev = cur.clone();
        *cur = "stopped".to_string();
        prev
    };
    if was == "stopped" {
        return;
    }
    let _ = app.emit("comfy-state", "stopped");
    if was == "starting" {
        log_line(
            &app,
            "comfy",
            "✖ ComfyUI 启动失败：进程在启动完成前就退出了，具体原因见上面的日志",
        );
        let _ = app.emit("comfy-error", "ComfyUI 启动失败，请查看运行日志");
    } else {
        log_line(&app, "comfy", "ComfyUI 已退出");
    }
}

fn port_in_use(port: u16) -> bool {
    match format!("127.0.0.1:{port}").parse::<std::net::SocketAddr>() {
        Ok(addr) => {
            std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
        }
        Err(_) => false,
    }
}

#[tauri::command]
fn comfy_launch(app: AppHandle) -> Result<Value, String> {
    {
        // 上次的进程如果已经退出（比如启动崩溃），直接清掉记录继续往下走，
        // 免得用户点第二次时被 "already" 挡回来什么都不发生。
        let st = app.state::<LauncherState>();
        let mut guard = st.comfy_child.lock().unwrap();
        if let Some(c) = guard.as_mut() {
            if matches!(c.try_wait(), Ok(Some(_))) {
                *guard = None;
            }
        }
        if guard.is_some() {
            return Ok(json!({"ok": true, "already": true}));
        }
    }
    let root = comfy_root();
    let rootp = Path::new(&root);
    if root.is_empty() || !rootp.join("main.py").exists() {
        return Err("请先在设置中选择 ComfyUI 安装目录".to_string());
    }
    let python = find_python(&root);
    let settings = read_settings();
    let port = comfy_port();
    let lan = settings.get("lanAccess").and_then(Value::as_bool).unwrap_or(false);
    let extra_args: Vec<String> = settings
        .get("extraArgs")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split_whitespace()
        .map(|t| t.to_string())
        .collect();
    let gen = {
        let st = app.state::<LauncherState>();
        *st.stopping.lock().unwrap() = false;
        let mut g = st.gen.lock().unwrap();
        *g += 1;
        *g
    };
    set_comfy_state(&app, "starting");
    log_line(&app, "comfy", &format!("使用 Python: {python}"));
    if port_in_use(port as u16) {
        log_line(
            &app,
            "comfy",
            &format!("⚠ 端口 {port} 已经被占用（可能是上一次没关掉的 ComfyUI），这次启动可能会失败"),
        );
    }

    let mut cmd = Command::new(&python);
    cmd.arg(rootp.join("main.py")).arg("--port").arg(port.to_string());
    if lan {
        cmd.arg("--listen").arg("0.0.0.0");
    }
    for a in &extra_args {
        cmd.arg(a);
    }
    let mut shown = format!("--port {port}");
    if lan {
        shown.push_str(" --listen 0.0.0.0");
    }
    if !extra_args.is_empty() {
        shown.push(' ');
        shown.push_str(&extra_args.join(" "));
    }
    log_line(&app, "comfy", &format!("启动参数: {shown}"));

    let mut child = prep(&mut cmd)
        .current_dir(rootp)
        // PYTHONUNBUFFERED: 日志实时输出，不在缓冲区里憋着
        // PYTHONIOENCODING: 管道里强制 UTF-8。默认会跟随系统 ANSI 代码页（简中 = GBK），
        //   ComfyUI 日志里的 ✅ 之类字符会让 logger 直接 UnicodeEncodeError 崩溃，
        //   同时中文也会被按 UTF-8 误读成 "�"。
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            set_comfy_state(&app, "stopped");
            format!("Python 启动失败: {e}")
        })?;

    #[cfg(windows)]
    {
        let job = {
            let st = app.state::<LauncherState>();
            let mut j = st.job.lock().unwrap();
            if *j == 0 {
                *j = winjob::create_kill_on_close_job();
            }
            *j
        };
        winjob::assign(job, &child);
    }

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let app_out = app.clone();
    let h_out = std::thread::spawn(move || stream_lines(stdout, app_out, "comfy".to_string(), true));
    let app_err = app.clone();
    let h_err = std::thread::spawn(move || stream_lines(stderr, app_err, "comfy".to_string(), true));

    {
        let st = app.state::<LauncherState>();
        *st.comfy_child.lock().unwrap() = Some(child);
    }

    let app_exit = app.clone();
    std::thread::spawn(move || watch_process_exit(app_exit, gen, vec![h_out, h_err]));
    let app_port = app.clone();
    std::thread::spawn(move || watch_port_until_ready(app_port, gen, port as u16));

    Ok(json!({"ok": true}))
}

#[tauri::command]
fn comfy_stop(app: AppHandle) -> Value {
    if stop_comfy_inner(&app) {
        log_line(&app, "comfy", "已停止 ComfyUI");
    }
    set_comfy_state(&app, "stopped");
    json!({"ok": true})
}

#[tauri::command]
fn comfy_open_browser() -> Value {
    let url = format!("http://127.0.0.1:{}", comfy_port());
    #[cfg(windows)]
    {
        let _ = prep(&mut Command::new("rundll32"))
            .args(["url.dll,FileProtocolHandler", &url])
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(&url).spawn();
    }
    json!({"ok": true})
}

// ---------- one-click update ----------
/// Validate the ComfyUI folder before doing anything destructive.
fn ensure_repo_ready(root: &Path) -> Result<(), String> {
    if root.to_string_lossy().trim().is_empty() || !root.join("main.py").exists() {
        return Err("请先选择 ComfyUI 安装目录".to_string());
    }
    if !root.join(".git").exists() {
        return Err("当前 ComfyUI 不是 Git 安装的，无法在线更新".to_string());
    }
    if !git_has_version() {
        return Err("未检测到 Git，请先安装 Git 后重试".to_string());
    }
    Ok(())
}

/// Check remote for the latest ComfyUI commit without touching the working tree.
#[tauri::command]
async fn comfy_check_update(app: AppHandle) -> Result<Value, String> {
    run_blocking(app, |app| {
        let root = comfy_root();
        let rootp = Path::new(&root);
        ensure_repo_ready(rootp)?;
        let links = link_dirs(rootp);
        let _ = link_skip_worktree(rootp, &links);
        let local = git_output(rootp, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
        let local_date =
            git_output(rootp, &["log", "-1", "--format=%cd", "--date=short"]).unwrap_or_default();
        let branch = current_branch(rootp)
            .or_else(|| remote_default_branch(rootp))
            .unwrap_or_else(|| "master".to_string());
        log_line(&app, "update", "正在连接远程仓库检查更新…");
        let mut failures = Vec::new();
        git_network(&app, rootp, &strs(&["fetch", "--prune", "origin", &branch]), &mut failures)
            .map_err(|e| format!("无法连接远程仓库（{e}），请检查网络或代理后重试"))?;
        let upstream = format!("origin/{branch}");
        // Some clones end up without a remote-tracking ref (explicit refspec fetch,
        // single-branch clones, …); FETCH_HEAD is the always-available fallback.
        let remote_ref = if git_output(rootp, &["rev-parse", "--verify", "--quiet", &upstream]).is_some() {
            upstream.clone()
        } else if git_output(rootp, &["rev-parse", "--verify", "--quiet", "FETCH_HEAD"]).is_some() {
            log_line(&app, "update", "未找到远程跟踪分支，改用 FETCH_HEAD 比对");
            "FETCH_HEAD".to_string()
        } else {
            return Err("远程仓库里没有找到对应分支，无法检查更新".to_string());
        };
        let remote = git_output(rootp, &["rev-parse", "--short", &remote_ref]).unwrap_or_default();
        let remote_date =
            git_output(rootp, &["log", "-1", "--format=%cd", "--date=short", &remote_ref])
                .unwrap_or_default();
        let remote_msg = git_output(rootp, &["log", "-1", "--format=%s", &remote_ref])
            .unwrap_or_default();
        let behind: i64 = git_output(rootp, &["rev-list", "--count", &format!("HEAD..{remote_ref}")])
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if behind > 0 {
            log_line(
                &app,
                "update",
                &format!("发现新版本 {remote}（落后 {behind} 个提交）"),
            );
        } else {
            log_line(&app, "update", "已是最新版本");
        }
        Ok(json!({
            "local": local,
            "localDate": local_date,
            "remote": remote,
            "remoteDate": remote_date,
            "remoteMsg": remote_msg,
            "behind": behind,
            "hasUpdate": behind > 0,
            "links": links,
        }))
    })
    .await
}

/// pip index fallback chain: official -> user's own pip config -> CN mirrors.
fn pip_install_with_fallback(
    app: &AppHandle,
    root: &Path,
    python: &str,
    requirements: &str,
) -> Result<(), String> {
    let third: Vec<(&str, Option<&str>)> = vec![
        ("官方软件源 pypi.org", Some("https://pypi.org/simple")),
        ("本机 pip 配置", None),
        ("清华镜像源", Some("https://pypi.tuna.tsinghua.edu.cn/simple")),
        ("腾讯云镜像源", Some("https://mirrors.cloud.tencent.com/pypi/simple")),
    ];
    let mut failures = Vec::new();
    for (i, (name, index)) in third.iter().enumerate() {
        let step = i + 1;
        let mut args = strs(&["-X", "utf8", "-m", "pip", "install", "-r", requirements, "--upgrade"]);
        if let Some(url) = index {
            args.push("--index-url".to_string());
            args.push(url.to_string());
        }
        if step > 1 {
            log_line(app, "update", &format!("上一个软件源安装失败，改用{name}重试…"));
        }
        if let Err(e) = run_stream(app, "update", python, &args, root) {
            failures.push(e);
        } else {
            if step > 1 {
                log_line(app, "update", &format!("✔ {name} 安装成功"));
            }
            return Ok(());
        }
    }
    Err(failures.pop().unwrap_or_else(|| "依赖安装失败".to_string()))
}

#[tauri::command]
async fn comfy_update(app: AppHandle) -> Result<Value, String> {
    run_blocking(app, |app| {
        let root = comfy_root();
        let rootp = Path::new(&root);
        ensure_repo_ready(rootp)?;

        let links = link_dirs(rootp);
        let total_steps = if links.is_empty() { 2 } else { 3 };
        log_line(
            &app,
            "update",
            &format!("—— 第 1 步 / 共 {total_steps} 步：更新 ComfyUI 本体 ——"),
        );

        // junctions / symlinks: keep git from choking on placeholder files
        if !links.is_empty() {
            log_line(
                &app,
                "update",
                &format!("检测到目录链接：{}，更新时会自动忽略这些目录内的文件变动", links.join("、")),
            );
            match link_skip_worktree(rootp, &links) {
                Ok(0) => {}
                Ok(n) => log_line(&app, "update", &format!("已让 Git 忽略链接目录下的 {n} 个占位文件")),
                Err(e) => log_line(&app, "update", &format!("⚠ {e}")),
            }
        }

        let blockers = workspace_blockers(rootp, &links);
        if !blockers.is_empty() {
            let shown: Vec<String> = blockers.iter().take(5).cloned().collect();
            log_line(&app, "update", "以下文件有本地改动，已暂停更新：");
            for b in &shown {
                log_line(&app, "update", &format!("  - {b}"));
            }
            if blockers.len() > shown.len() {
                log_line(&app, "update", &format!("  … 另有 {} 个文件", blockers.len() - shown.len()));
            }
            return Err(format!(
                "ComfyUI 里有 {} 个文件的本地改动可能会更新冲突，请先还原或备份：{}",
                blockers.len(),
                shown.join("；")
            ));
        }

        let branch = ensure_on_branch(&app, rootp)?;
        let mut failures = Vec::new();
        git_network(
            &app,
            rootp,
            &strs(&["fetch", "--prune", "origin", &branch]),
            &mut failures,
        )
        .map_err(|e| format!("无法连接 GitHub 更新 ComfyUI（{e}）"))?;

        if let Err(e) = git_network(&app, rootp, &strs(&["pull", "--ff-only", "origin", &branch]), &mut failures) {
            let upstream = format!("origin/{branch}");
            let remote_ref = if git_output(rootp, &["rev-parse", "--verify", "--quiet", &upstream]).is_some() {
                upstream
            } else {
                "FETCH_HEAD".to_string()
            };
            let ahead: i64 = git_output(rootp, &["rev-list", "--count", &format!("{remote_ref}..HEAD")])
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if ahead > 0 {
                return Err(format!(
                    "拉取更新失败（{e}）：本地分支领先远端 {ahead} 个提交，为避免覆盖你的改动已中止"
                ));
            }
            return Err(format!("拉取 ComfyUI 最新代码失败（{e}）"));
        }

        // links can be clobbered by checkout; report anything that got replaced
        let mut broken = Vec::new();
        for name in &links {
            if !is_reparse_point(&rootp.join(name)) {
                broken.push(name.clone());
            }
        }
        if !broken.is_empty() {
            log_line(
                &app,
                "update",
                &format!("⚠ 更新后这些链接目录被还原成了普通文件夹：{}，可能需要重新创建链接", broken.join("、")),
            );
        }
        let _ = link_skip_worktree(rootp, &links);

        let dep_step = if links.is_empty() { 2 } else { 2 };
        log_line(
            &app,
            "update",
            &format!("—— 第 {dep_step} 步 / 共 {total_steps} 步：更新运行依赖 ——"),
        );
        let requirements = rootp.join("requirements.txt");
        if requirements.exists() {
            let python = find_python(&root);
            pip_install_with_fallback(&app, rootp, &python, &requirements.to_string_lossy())?;
        } else {
            log_line(&app, "update", "未找到 requirements.txt，跳过依赖更新");
        }

        if !links.is_empty() {
            log_line(
                &app,
                "update",
                &format!("—— 第 {total_steps} 步 / 共 {total_steps} 步：检查目录链接 ——"),
            );
            for name in &links {
                if is_reparse_point(&rootp.join(name)) {
                    log_line(&app, "update", &format!("✔ {name} 链接正常"));
                } else {
                    log_line(&app, "update", &format!("⚠ {name} 已不是链接目录，请重新创建后再使用"));
                }
            }
        }

        if let Some(hash) = git_output(rootp, &["rev-parse", "--short", "HEAD"]) {
            log_line(&app, "update", &format!("✔ 更新完成，当前版本 {hash}"));
        } else {
            log_line(&app, "update", "✔ 更新完成");
        }
        Ok(json!({"ok": true}))
    })
    .await
}

// ---------- custom nodes ----------
fn safe_node_dir(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains("..") || name.contains(['/', '\\']) {
        return Err("非法节点名称".to_string());
    }
    let dir = Path::new(&comfy_root()).join("custom_nodes").join(name);
    Ok(dir)
}

#[tauri::command]
fn nodes_list() -> Value {
    let dir = Path::new(&comfy_root()).join("custom_nodes");
    let mut list = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let is_git = p.join(".git").exists();
            let version = if is_git {
                git_output(&p, &["rev-parse", "--short", "HEAD"])
            } else {
                None
            };
            list.push(json!({"name": name, "isGit": is_git, "version": version}));
        }
    }
    json!(list)
}

/// Run a git command quietly with a timeout, so one unreachable node repo
/// cannot stall the whole check forever.
fn git_output_timeout(root: &Path, args: &[String], secs: u64) -> Option<String> {
    let mut full = git_base_args(root);
    full.extend_from_slice(args);
    let child = prep(&mut Command::new("git"))
        .args(&full)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // wait_with_output 消费 child，进程结束后线程自然退出
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(std::time::Duration::from_secs(secs)) {
        Ok(Ok(out)) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        // timed out or failed: detach, the helper thread ends when git exits
        _ => None,
    }
}

/// Fetch one node repo, falling back through the proxy chain like the
/// ComfyUI update does (many users are behind Clash / v2ray).
///
/// 关键点：这里用带 refspec 的 `fetch origin <branch>`，结果只落在 FETCH_HEAD。
/// 不要指望 origin/<branch> 这个远端跟踪引用被更新 —— 带 refspec 的 fetch
/// 本来就不动它，所以判断"是否有更新"必须读 FETCH_HEAD。
fn git_fetch_quiet(dir: &Path, branch: &str) -> bool {
    // 慢速保护：10 秒低于 1KB/s 就放弃，避免卡在一个连不上的仓库上
    let guard = ["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=10"];
    for proxy in network_attempts() {
        let mut args: Vec<String> = guard.iter().map(|s| s.to_string()).collect();
        if !proxy.is_empty() {
            args.extend(git_proxy_args(&proxy));
        }
        args.extend(strs(&["fetch", "--quiet", "origin", branch]));
        if git_output_timeout(dir, &args, 60).is_some() {
            return true;
        }
    }
    false
}

/// Compare one node's local HEAD against the remote branch just fetched.
fn node_update_info(name: &str, dir: &Path) -> Value {
    let branch = current_branch(dir)
        .or_else(|| remote_default_branch(dir))
        .unwrap_or_else(|| "master".to_string());
    let local_sha = git_output(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
    let local = git_output(dir, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let fetched = git_fetch_quiet(dir, &branch);
    // FETCH_HEAD 是本次 fetch 的真实结果；远端跟踪引用可能压根没被更新
    let remote_sha = git_output(dir, &["rev-parse", "FETCH_HEAD"])
        .filter(|s| !s.is_empty())
        .or_else(|| git_output(dir, &["rev-parse", &format!("origin/{branch}")]));
    let behind = match &remote_sha {
        Some(r) if !local_sha.is_empty() && *r != local_sha => {
            git_output(dir, &["rev-list", "--count", &format!("HEAD..{r}")])
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(1) // 远端对象没下全也算"有新提交"
        }
        _ => 0,
    };
    let remote = remote_sha
        .as_ref()
        .map(|s| s.chars().take(7).collect::<String>())
        .unwrap_or_default();
    let error = if fetched {
        None
    } else {
        Some("无法连接远端，结果可能不是最新".to_string())
    };
    json!({
        "name": name,
        "local": local,
        "remote": remote,
        "behind": behind,
        "fetched": fetched,
        "error": error,
    })
}

/// 手动检查：哪些 Git 节点有新提交。联网操作，因此不随切页自动执行。
#[tauri::command]
async fn nodes_check_updates(app: AppHandle) -> Result<Value, String> {
    run_blocking(app, move |app| {
        let dir = Path::new(&comfy_root()).join("custom_nodes");
        let mut targets: Vec<(String, std::path::PathBuf)> = Vec::new();
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_dir() || !p.join(".git").exists() {
                    continue;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                targets.push((name, p));
            }
        }
        targets.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        if targets.is_empty() {
            return Ok(json!({"list": [], "outdated": []}));
        }
        let total = targets.len();
        log_line(&app, "nodes", &format!("开始检查 {total} 个 Git 节点是否有更新…"));
        let mut list: Vec<Value> = Vec::new();
        let mut done = 0usize;
        // 6 路并发：串行跑 60+ 个仓库会慢到不可用
        for chunk in targets.chunks(6) {
            std::thread::scope(|scope| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|(name, p)| scope.spawn(move || node_update_info(name, p.as_path())))
                    .collect();
                for h in handles {
                    if let Ok(v) = h.join() {
                        list.push(v);
                    }
                }
            });
            done += chunk.len();
            log_line(&app, "nodes", &format!("已检查 {done}/{total} …"));
        }
        let outdated: Vec<String> = list
            .iter()
            .filter(|v| v.get("behind").and_then(Value::as_u64).unwrap_or(0) > 0)
            .filter_map(|v| v.get("name").and_then(Value::as_str).map(|s| s.to_string()))
            .collect();
        if outdated.is_empty() {
            log_line(&app, "nodes", "✔ 检查完成：所有节点都已是最新");
        } else {
            log_line(&app, "nodes", &format!("✔ 检查完成：{} 个节点有更新", outdated.len()));
        }
        Ok(json!({"list": list, "outdated": outdated}))
    })
    .await
}

#[tauri::command]
async fn nodes_install(app: AppHandle, url: String) -> Result<Value, String> {
    run_blocking(app, move |app| {
        let url = url.trim().to_string();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err("请输入有效的 Git 仓库地址".to_string());
        }
        let name = url
            .trim_end_matches(".git")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            return Err("无法从地址解析节点名称".to_string());
        }
        let root = comfy_root();
        let dir = Path::new(&root).join("custom_nodes");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        if dir.join(&name).exists() {
            return Err(format!("节点 {name} 已存在"));
        }
        run_stream(&app, "nodes", "git", &[s("clone"), url], &dir)?;
        let node_dir = dir.join(&name);
        let req = node_dir.join("requirements.txt");
        if req.exists() {
            let python = find_python(&root);
            log_line(&app, "nodes", &format!("检测到 {name} 的依赖，自动安装中…"));
            run_stream(
                &app,
                "nodes",
                &python,
                &strs(&["-m", "pip", "install", "-r", "requirements.txt"]),
                &node_dir,
            )?;
        }
        log_line(&app, "nodes", &format!("✔ 节点 {name} 安装完成（重启 ComfyUI 后生效）"));
        Ok(json!({"ok": true}))
    })
    .await
}

fn s(v: &str) -> String {
    v.to_string()
}

#[tauri::command]
async fn nodes_update(app: AppHandle, name: String) -> Result<Value, String> {
    run_blocking(app, move |app| {
        let dir = safe_node_dir(&name)?;
        if !dir.join(".git").exists() {
            return Err("该节点不是通过 Git 安装的，无法更新".to_string());
        }
        run_stream(&app, "nodes", "git", &strs(&["pull"]), &dir)?;
        if dir.join("requirements.txt").exists() {
            let python = find_python(&comfy_root());
            run_stream(
                &app,
                "nodes",
                &python,
                &strs(&["-m", "pip", "install", "-r", "requirements.txt", "--upgrade"]),
                &dir,
            )?;
        }
        log_line(&app, "nodes", &format!("✔ 节点 {name} 已更新"));
        Ok(json!({"ok": true}))
    })
    .await
}

#[tauri::command]
async fn nodes_update_all(app: AppHandle, names: Option<Vec<String>>) -> Result<Value, String> {
    run_blocking(app, move |app| {
        let dir = Path::new(&comfy_root()).join("custom_nodes");
        let mut done = 0u32;
        let mut failed = 0u32;
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_dir() || !p.join(".git").exists() {
                    continue;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                // 传了名单就只更新这些（通常是"检查更新"筛出的落后节点）
                if let Some(sel) = &names {
                    if !sel.contains(&name) {
                        continue;
                    }
                }
                log_line(&app, "nodes", &format!("更新 {name} …"));
                match run_stream(&app, "nodes", "git", &strs(&["pull"]), &p) {
                    Ok(()) => done += 1,
                    Err(err) => {
                        failed += 1;
                        log_line(&app, "nodes", &format!("✖ {name} 更新失败: {err}"));
                    }
                }
            }
        }
        log_line(&app, "nodes", &format!("✔ 全部完成：成功 {done} 个，失败 {failed} 个"));
        Ok(json!({"ok": true, "done": done, "failed": failed}))
    })
    .await
}

#[tauri::command]
fn nodes_remove(app: AppHandle, name: String) -> Result<Value, String> {
    let dir = safe_node_dir(&name)?;
    if !dir.exists() {
        return Err("节点不存在".to_string());
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("删除失败: {e}"))?;
    log_line(&app, "nodes", &format!("✔ 节点 {name} 已删除（重启 ComfyUI 后生效）"));
    Ok(json!({"ok": true}))
}

// ---------- node registry search (ComfyUI Manager custom-node-list) ----------
// 数据来源与 ComfyUI Manager 相同：ltdrdata/ComfyUI-Manager 仓库的
// custom-node-list.json。缓存到 exe 旁的 node-registry.json（24h 过期），
// 用 Windows 自带的 curl.exe 下载（零新增依赖），主源失败时依次尝试国内镜像。
const REGISTRY_URLS: [&str; 3] = [
    "https://raw.githubusercontent.com/ltdrdata/ComfyUI-Manager/main/custom-node-list.json",
    "https://ghfast.top/https://raw.githubusercontent.com/ltdrdata/ComfyUI-Manager/main/custom-node-list.json",
    "https://gh-proxy.com/https://raw.githubusercontent.com/ltdrdata/ComfyUI-Manager/main/custom-node-list.json",
];

fn registry_cache_path() -> PathBuf {
    exe_dir().join("node-registry.json")
}

fn curl_get(url: &str, timeout_secs: u32) -> Result<String, String> {
    let timeout = timeout_secs.to_string();
    let out = prep(&mut Command::new("curl.exe"))
        .args(["-sL", "--fail", "--max-time", &timeout, url])
        .output()
        .map_err(|e| format!("curl 启动失败: {e}"))?;
    if !out.status.success() {
        return Err(format!("下载失败 (exit {:?})", out.status.code()));
    }
    String::from_utf8(out.stdout).map_err(|_| "响应编码错误".to_string())
}

fn fetch_registry(app: &AppHandle) -> Result<Value, String> {
    let mut last_err = String::from("未知错误");
    for u in REGISTRY_URLS {
        log_line(app, "nodes", &format!("获取节点仓库列表：{u}"));
        match curl_get(u, 45) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(v) if v.get("custom_nodes").and_then(Value::as_array).is_some() => {
                    let _ = fs::write(registry_cache_path(), &text);
                    log_line(app, "nodes", "✔ 节点仓库列表已更新");
                    return Ok(v);
                }
                _ => last_err = "返回内容不是有效的节点列表".to_string(),
            },
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

#[tauri::command]
async fn nodes_search(app: AppHandle, query: String, refresh: bool) -> Result<Value, String> {
    run_blocking(app, move |app| {
        // 读取缓存；缺失 / 超过 24h / 手动刷新时重新拉取
        let cache = registry_cache_path();
        let stale = fs::metadata(&cache)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|e| e.as_secs() > 24 * 3600).unwrap_or(true))
            .unwrap_or(true);
        let mut from_cache = false;
        let data = if refresh || stale {
            match fetch_registry(&app) {
                Ok(v) => v,
                Err(e) => {
                    if cache.exists() {
                        from_cache = true;
                        log_line(&app, "nodes", &format!("在线获取失败（{e}），改用本地缓存"));
                        let text = fs::read_to_string(&cache).map_err(|e2| e2.to_string())?;
                        serde_json::from_str(&text).map_err(|e2| e2.to_string())?
                    } else {
                        return Err(format!("无法获取节点列表：{e}"));
                    }
                }
            }
        } else {
            from_cache = true;
            let text = fs::read_to_string(&cache).map_err(|e| e.to_string())?;
            serde_json::from_str(&text).map_err(|e| e.to_string())?
        };

        // 已安装节点目录名（小写），用于标记 installed
        let mut installed: Vec<String> = Vec::new();
        let dir = Path::new(&comfy_root()).join("custom_nodes");
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    installed.push(e.file_name().to_string_lossy().to_lowercase());
                }
            }
        }

        let q = query.trim().to_lowercase();
        let nodes = data
            .get("custom_nodes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut results = Vec::new();
        for n in nodes {
            let title = n.get("title").and_then(Value::as_str).unwrap_or("");
            let author = n.get("author").and_then(Value::as_str).unwrap_or("");
            let desc = n.get("description").and_then(Value::as_str).unwrap_or("");
            let reference = n.get("reference").and_then(Value::as_str).unwrap_or("");
            if reference.is_empty() {
                continue;
            }
            if !q.is_empty() {
                let hay = format!("{title} {author} {desc} {reference}").to_lowercase();
                if !q.split_whitespace().all(|w| hay.contains(w)) {
                    continue;
                }
            }
            let repo_name = reference
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("")
                .trim_end_matches(".git")
                .to_lowercase();
            results.push(json!({
                "title": title,
                "author": author,
                "description": desc,
                "url": reference,
                "installed": installed.contains(&repo_name),
            }));
            if results.len() >= 80 {
                break;
            }
        }
        Ok(json!({ "results": results, "fromCache": from_cache }))
    })
    .await
}

// ---------- model paths (extra_model_paths.yaml) ----------
const KNOWN_MODEL_TYPES: [&str; 19] = [
    "checkpoints", "loras", "vae", "controlnet", "upscale_models", "embeddings",
    "clip", "clip_vision", "diffusion_models", "text_encoders", "unet",
    "gligen", "style_models", "hypernetworks", "photomaker", "classifiers",
    "model_patches", "audio_encoders", "vae_approx",
];

fn yaml_file() -> PathBuf {
    Path::new(&comfy_root()).join("extra_model_paths.yaml")
}

fn unquote(v: &str) -> String {
    v.trim().trim_matches('"').trim_matches('\'').to_string()
}

#[tauri::command]
#[allow(unused_assignments)] // flush! 宏在循环末尾最后一次展开时的重置是循环内必需的
fn models_get() -> Value {
    let mut entries: Vec<Value> = Vec::new();
    let Ok(text) = fs::read_to_string(yaml_file()) else {
        return json!(entries);
    };
    let mut cur_name: Option<String> = None;
    let mut cur_base = String::new();
    let mut cur_types = serde_json::Map::new();

    macro_rules! flush {
        () => {
            if let Some(n) = cur_name.take() {
                entries.push(json!({
                    "name": n,
                    "basePath": cur_base,
                    "types": Value::Object(cur_types.clone()),
                }));
                cur_base = String::new();
                cur_types = serde_json::Map::new();
            }
        };
    }

    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let top_level = !line.starts_with(' ') && !line.starts_with('\t');
        if top_level && t.ends_with(':') {
            flush!();
            cur_name = Some(unquote(t.trim_end_matches(':')));
        } else if cur_name.is_some() {
            if let Some((k, v)) = t.split_once(':') {
                let key = k.trim();
                let val = unquote(v);
                if key == "base_path" {
                    cur_base = val;
                } else if !key.is_empty() {
                    cur_types.insert(key.to_string(), Value::String(val));
                }
            }
        }
    }
    flush!();
    json!(entries)
}

const MODEL_FILE_EXTS: [&str; 6] = ["safetensors", "ckpt", "pt", "pth", "bin", "gguf"];

#[tauri::command]
fn models_scan(base_path: String) -> Value {
    let base = Path::new(&base_path);
    let mut found = serde_json::Map::new();
    for t in KNOWN_MODEL_TYPES {
        if base.join(t).is_dir() {
            found.insert(t.to_string(), Value::String(t.to_string()));
        }
    }
    // loose model files sitting directly in the root of the picked folder
    let mut loose = 0usize;
    if let Ok(rd) = fs::read_dir(base) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() {
                let ext = p
                    .extension()
                    .map(|x| x.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if MODEL_FILE_EXTS.contains(&ext.as_str()) {
                    loose += 1;
                }
            }
        }
    }
    json!({ "types": Value::Object(found), "looseFiles": loose })
}

#[tauri::command]
fn models_save(app: AppHandle, entries: Value) -> Result<Value, String> {
    let root = comfy_root();
    if root.is_empty() {
        return Err("请先选择 ComfyUI 安装目录".to_string());
    }
    let mut out = String::new();
    if let Some(list) = entries.as_array() {
        for e in list {
            let name = e.get("name").and_then(Value::as_str).unwrap_or("").trim();
            let base = e.get("basePath").and_then(Value::as_str).unwrap_or("").trim();
            if name.is_empty() || base.is_empty() {
                continue;
            }
            out.push_str(&format!("{name}:\n  base_path: {base}\n"));
            if let Some(types) = e.get("types").and_then(Value::as_object) {
                for (k, v) in types {
                    if let Some(val) = v.as_str() {
                        out.push_str(&format!("  {k}: {val}\n"));
                    }
                }
            }
            out.push('\n');
        }
    }
    fs::write(yaml_file(), out).map_err(|e| format!("写入模型路径配置失败: {e}"))?;
    log_line(&app, "models", "✔ 模型路径已保存（重启 ComfyUI 后生效）");
    Ok(json!({"ok": true}))
}

/// 前端把 CSS 里的窗口描边样式送进来：`--win-stroke`（颜色）与
/// `--win-stroke-size`（线粗，CSS 像素）。Rust 读不到 CSS，所以只能这么传。
/// 解析失败会静默沿用当前值 —— 描边是纯装饰，不该因为它让启动失败。
#[tauri::command]
fn frame_set_style(color: String, size: String) {
    #[cfg(target_os = "windows")]
    crate::rounded_frame::set_style(&color, &size);
    #[cfg(not(target_os = "windows"))]
    let _ = (color, size);
}

// ---------- entry ----------
#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Windows 11 会在窗口矩形外缘画一圈强调色边框（截图里的"蓝色外边框"）。
/// 通过 DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE 抑制它，让透明圆角窗口干净呈现。
#[cfg(target_os = "windows")]
fn suppress_accent_border(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR};
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    if let Ok(hwnd) = window.hwnd() {
        unsafe {
            let _ = DwmSetWindowAttribute(
                HWND(hwnd.0 as *mut core::ffi::c_void),
                DWMWA_BORDER_COLOR,
                &DWMWA_COLOR_NONE as *const u32 as *const core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
}

pub fn run() {
    // WebView2 缺失时窗口能建出来但内容渲染不出来 —— 用户只会看到一个白屏。
    // 所以先检测，缺了就显示原生说明窗口（不依赖 WebView2），然后退出。
    #[cfg(target_os = "windows")]
    if crate::webview2_gate::force_missing()
        || !crate::webview2_gate::webview2_installed()
    {
        crate::webview2_gate::show_missing_gate();
        return;
    }

    let built = tauri::Builder::default()
        .manage(LauncherState::new())
        .setup(|app| {
            #[cfg(target_os = "windows")]
            if let Some(win) = app.get_webview_window("main") {
                // 不透明窗口：把 WebView2 默认白底换成应用底色，消除启动白闪
                let _ = win.set_background_color(Some(tauri::webview::Color(242, 243, 247, 255)));
                suppress_accent_border(&win);
                crate::rounded_frame::apply_rounded_corners(&win);
                let win2 = win.clone();
                win.on_window_event(move |event| match event {
                    tauri::WindowEvent::Resized(_) => {
                        crate::rounded_frame::apply_rounded_corners(&win2)
                    }
                    // 关窗口 = 退程序，先把 ComfyUI 收掉，不留后台进程
                    tauri::WindowEvent::CloseRequested { .. } => {
                        shutdown_comfy(win2.app_handle());
                    }
                    _ => {}
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            pick_directory,
            comfy_status,
            comfy_launch,
            comfy_stop,
            comfy_open_browser,
            comfy_check_update,
            comfy_update,
            nodes_list,
            nodes_check_updates,
            nodes_install,
            nodes_update,
            nodes_update_all,
            nodes_remove,
            nodes_search,
            models_get,
            models_scan,
            models_save,
            frame_set_style,
        ])
        .build(tauri::generate_context!());

    // 兜底：组件存在但 WebView 仍初始化失败（运行时损坏 / 被策略禁用）时，
    // 同样给原生提示，而不是抛 panic 或直接白屏。
    let app = match built {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[TerryComfy启动器] 启动失败: {e}");
            #[cfg(target_os = "windows")]
            crate::webview2_gate::show_missing_gate();
            return;
        }
    };

    app.run(|app_handle, event| match event {
        // 兜底：无论以哪种方式退出，都不留下还在跑的 ComfyUI
        tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
            shutdown_comfy(app_handle);
        }
        _ => {}
    });
}
