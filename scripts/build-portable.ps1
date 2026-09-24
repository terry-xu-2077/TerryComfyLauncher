# Build a portable single-file release:
#   release\ComfyUI-Launcher-portable\ComfyUI-Launcher.exe   <- double-click to use
# Config (settings.json) is stored next to the exe at runtime.
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Frontend = Join-Path $Root 'frontend'
$FrontendPackage = Join-Path $Frontend 'package.json'
$FrontendModules = Join-Path $Frontend 'node_modules'
$FrontendStamp = Join-Path $FrontendModules '.comfy-launcher-package.sha256'
$IconSource = Join-Path $Frontend 'src-tauri\app-icon.png'
$IconOut = Join-Path $Frontend 'src-tauri\icons'
$ReleaseExe = Join-Path $Frontend 'src-tauri\target\release\terry-comfy-launcher.exe'
$OutDir = Join-Path $Root 'release\ComfyUI-Launcher-portable'

function Write-Step([string]$Text) {
    Write-Host "`n==> $Text" -ForegroundColor Cyan
}

function Fail([string]$Text) {
    Write-Host "`n[ERROR] $Text" -ForegroundColor Red
    # Non-interactive (CI/background): ReadKey would hang forever, just exit
    if ([Environment]::UserInteractive -and -not $env:CI) {
        try {
            Write-Host 'Press any key to close...'
            $null = $Host.UI.RawUI.ReadKey('NoEcho,IncludeKeyDown')
        } catch { }
    }
    exit 1
}

function Refresh-RustPath {
    $CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if ((Test-Path $CargoBin) -and (($env:Path -split ';') -notcontains $CargoBin)) {
        $env:Path = "$CargoBin;$env:Path"
    }
}

Set-Location $Root
Write-Host 'ComfyUI Launcher - Portable Build' -ForegroundColor Green

if (-not (Get-Command node -ErrorAction SilentlyContinue)) { Fail 'Node.js was not found.' }
if (-not (Get-Command npm -ErrorAction SilentlyContinue)) { Fail 'npm was not found.' }
Refresh-RustPath
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Fail 'Cargo was not found. Run 启动项目.bat once first - it installs Rust automatically.' }

$env:CARGO_HTTP_MULTIPLEXING = 'false'
$env:CARGO_NET_RETRY = '2'
$env:CARGO_HTTP_TIMEOUT = '30'

$FrontendHash = (Get-FileHash -Algorithm SHA256 $FrontendPackage).Hash
$InstalledHash = if (Test-Path $FrontendStamp) { (Get-Content $FrontendStamp -Raw).Trim() } else { '' }
if ((-not (Test-Path $FrontendModules)) -or ($FrontendHash -ne $InstalledHash)) {
    Write-Step 'Installing frontend dependencies'
    Push-Location $Frontend
    try {
        & npm install --package-lock=false
        if ($LASTEXITCODE -ne 0) { Fail 'npm dependency installation failed.' }
    } finally { Pop-Location }
    New-Item -ItemType Directory -Path $FrontendModules -Force | Out-Null
    Set-Content -Path $FrontendStamp -Value $FrontendHash -NoNewline
}

# Fingerprint-based: regenerating icons/ from app-icon.png only when the source
# PNG actually changed (or the icons are missing). See scripts\sync-app-icon.ps1.
try {
    & (Join-Path $PSScriptRoot 'sync-app-icon.ps1') -Frontend $Frontend
} catch {
    # Do not check $LASTEXITCODE here: the sync script short-circuits without
    # running any native command when the fingerprint matches, leaving
    # $LASTEXITCODE unset ($null -ne 0 is true -> false failure).
    Fail "Application icon generation failed: $($_.Exception.Message)"
}

Write-Step 'Building frontend (vite)'
Push-Location $Frontend
try {
    & npm run build
    if ($LASTEXITCODE -ne 0) { Fail 'Frontend build failed.' }
} finally { Pop-Location }

Write-Step 'Compiling release exe (cargo, this can take a few minutes on first build)'
# IMPORTANT: `tauri/custom-protocol` must be enabled for production builds.
# Without it tauri treats the app as dev mode and the window tries to load
# devUrl (http://localhost:1420) instead of the embedded frontend assets,
# showing a "localhost refused connection" error page.
& cargo build --release --features tauri/custom-protocol --manifest-path (Join-Path $Frontend 'src-tauri\Cargo.toml')
if ($LASTEXITCODE -ne 0) { Fail 'Cargo release build failed.' }

if (-not (Test-Path $ReleaseExe)) { Fail "Release exe not found: $ReleaseExe" }

Write-Step 'Assembling portable folder'
# Overwrite in place instead of deleting the folder: keeps the user's
# settings.json (stored next to the exe) across rebuilds.
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
Copy-Item $ReleaseExe (Join-Path $OutDir 'ComfyUI-Launcher.exe') -Force

# WebView2 引导程序（约 1.5MB 在线安装包）。放在 exe 旁，这样离线机器点“立即安装”
# 也能直接装上；下载失败不影响打包，程序会退回打开官方下载页。
$Bootstrapper = Join-Path $OutDir 'MicrosoftEdgeWebview2Setup.exe'
if (-not (Test-Path $Bootstrapper)) {
    Write-Step 'Downloading WebView2 bootstrapper (optional, for offline machines)'
    try {
        Invoke-WebRequest -UseBasicParsing `
            -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' `
            -OutFile $Bootstrapper -TimeoutSec 60
        Write-Host 'Saved next to the exe. It is optional - safe to delete.' -ForegroundColor DarkGray
    } catch {
        Write-Host 'Download failed (offline/proxy). Skipped - the app will open the download page instead.' -ForegroundColor Yellow
    }
}

$SizeMB = [math]::Round((Get-Item (Join-Path $OutDir 'ComfyUI-Launcher.exe')).Length / 1MB, 1)
Write-Host "`nDone!" -ForegroundColor Green
Write-Host "Portable app: $OutDir\ComfyUI-Launcher.exe ($SizeMB MB)" -ForegroundColor Green
Write-Host 'Copy the whole folder anywhere and double-click the exe. settings.json is created next to it.' -ForegroundColor DarkGray
