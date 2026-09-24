$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Frontend = Join-Path $Root 'frontend'
$FrontendPackage = Join-Path $Frontend 'package.json'
$FrontendModules = Join-Path $Frontend 'node_modules'
$FrontendStamp = Join-Path $FrontendModules '.comfy-launcher-package.sha256'
$TauriManifest = Join-Path $Frontend 'src-tauri\Cargo.toml'
$IconSource = Join-Path $Frontend 'src-tauri\app-icon.png'
$IconOut = Join-Path $Frontend 'src-tauri\icons'
$ProxyHost = '127.0.0.1'
$ProxyPort = 7897
$ProxyUrl = "http://${ProxyHost}:${ProxyPort}"
$RustupUrl = 'https://win.rustup.rs/x86_64'

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

function Test-LocalPort([string]$HostName, [int]$Port) {
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $result = $client.BeginConnect($HostName, $Port, $null, $null)
        if (-not $result.AsyncWaitHandle.WaitOne(700)) { return $false }
        $client.EndConnect($result)
        return $true
    } catch {
        return $false
    } finally {
        $client.Close()
    }
}

function Clear-ProxyEnv {
    'HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','CARGO_HTTP_PROXY','GIT_HTTP_PROXY','GIT_HTTPS_PROXY' | ForEach-Object {
        Remove-Item "Env:$_" -ErrorAction SilentlyContinue
    }
}

function Enable-ProxyEnv {
    $env:HTTP_PROXY = $ProxyUrl
    $env:HTTPS_PROXY = $ProxyUrl
    $env:ALL_PROXY = $ProxyUrl
    $env:CARGO_HTTP_PROXY = $ProxyUrl
    $env:GIT_HTTP_PROXY = $ProxyUrl
    $env:GIT_HTTPS_PROXY = $ProxyUrl
}

function Refresh-RustPath {
    $CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if ((Test-Path $CargoBin) -and (($env:Path -split ';') -notcontains $CargoBin)) {
        $env:Path = "$CargoBin;$env:Path"
    }
}

function Install-RustToolchain {
    Write-Step 'Rust/Cargo not found - installing Rust automatically'

    # rustup-init inspects argv[0] / its executable filename on Windows to decide whether
    # it is the installer or one of rustup's proxy tools. Keep the canonical filename.
    $RustupInstallerDir = Join-Path $env:TEMP 'comfy-launcher-rustup'
    $RustupInstaller = Join-Path $RustupInstallerDir 'rustup-init.exe'
    New-Item -ItemType Directory -Path $RustupInstallerDir -Force | Out-Null
    Remove-Item $RustupInstaller -ErrorAction SilentlyContinue

    Clear-ProxyEnv
    try {
        Write-Host 'Downloading official rustup installer...' -ForegroundColor DarkGray
        Invoke-WebRequest -UseBasicParsing -Uri $RustupUrl -OutFile $RustupInstaller -TimeoutSec 60
    } catch {
        if (-not $ProxyAvailable) {
            Remove-Item $RustupInstallerDir -Recurse -Force -ErrorAction SilentlyContinue
            Fail 'Rust could not be downloaded from rustup.rs and the local proxy is unavailable.'
        }
        Write-Host "Rust direct download failed. Retrying through $ProxyUrl ..." -ForegroundColor Yellow
        try {
            Invoke-WebRequest -UseBasicParsing -Uri $RustupUrl -OutFile $RustupInstaller -Proxy $ProxyUrl -TimeoutSec 90
        } catch {
            Remove-Item $RustupInstallerDir -Recurse -Force -ErrorAction SilentlyContinue
            Fail 'Rust automatic download failed both directly and through the local proxy.'
        }
    }

    Write-Host 'Installing Rust stable toolchain (minimal profile)...' -ForegroundColor DarkGray
    & $RustupInstaller -y --profile minimal --default-toolchain stable
    $RustupExit = $LASTEXITCODE
    Remove-Item $RustupInstallerDir -Recurse -Force -ErrorAction SilentlyContinue
    if ($RustupExit -ne 0) {
        Fail "rustup installation failed with exit code $RustupExit."
    }

    Refresh-RustPath
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Fail 'Rust was installed, but Cargo is still unavailable in the current process.'
    }

    Write-Host 'Rust/Cargo installed successfully.' -ForegroundColor Green
}

function Install-FrontendDependencies {
    Push-Location $Frontend
    try {
        Clear-ProxyEnv
        & npm install --package-lock=false
        if ($LASTEXITCODE -eq 0) { return $true }

        if ($ProxyAvailable) {
            Write-Host "npm direct install failed. Retrying through $ProxyUrl ..." -ForegroundColor Yellow
            Enable-ProxyEnv
            & npm install --package-lock=false
            $ok = $LASTEXITCODE -eq 0
            Clear-ProxyEnv
            return $ok
        }
        return $false
    } finally {
        Pop-Location
    }
}

function Invoke-CargoFetch([string]$Mode) {
    Write-Step "Preparing Rust dependencies ($Mode)"
    & cargo fetch --manifest-path $TauriManifest
    return $LASTEXITCODE
}

Set-Location $Root

Write-Host 'ComfyUI Launcher - Development Launcher' -ForegroundColor Green
Write-Host "Project: $Root"

$ProxyAvailable = Test-LocalPort $ProxyHost $ProxyPort
if ($ProxyAvailable) {
    Write-Host "Local development proxy available: $ProxyUrl" -ForegroundColor Green
} else {
    Write-Host "Local development proxy unavailable: $ProxyUrl" -ForegroundColor Yellow
}

if (-not (Get-Command node -ErrorAction SilentlyContinue)) { Fail 'Node.js was not found. Install Node.js LTS and run this launcher again.' }
if (-not (Get-Command npm -ErrorAction SilentlyContinue)) { Fail 'npm was not found. Reinstall Node.js LTS and run this launcher again.' }

Refresh-RustPath
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Install-RustToolchain
}

$env:CARGO_HTTP_MULTIPLEXING = 'false'
$env:CARGO_NET_RETRY = '2'
$env:CARGO_HTTP_TIMEOUT = '30'
Write-Host 'Cargo source replacement: crates.io -> RsProxy sparse' -ForegroundColor Green

$FrontendHash = (Get-FileHash -Algorithm SHA256 $FrontendPackage).Hash
$InstalledHash = if (Test-Path $FrontendStamp) { (Get-Content $FrontendStamp -Raw).Trim() } else { '' }
if ((-not (Test-Path $FrontendModules)) -or ($FrontendHash -ne $InstalledHash)) {
    Write-Step 'Installing/updating frontend dependencies'
    if (-not (Install-FrontendDependencies)) { Fail 'npm dependency installation failed.' }
    New-Item -ItemType Directory -Path $FrontendModules -Force | Out-Null
    Set-Content -Path $FrontendStamp -Value $FrontendHash -NoNewline
} else {
    Write-Host 'Frontend dependencies are up to date.' -ForegroundColor DarkGray
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

Clear-ProxyEnv
$env:CARGO_NET_RETRY = '2'
$env:CARGO_HTTP_TIMEOUT = '30'
$FetchCode = Invoke-CargoFetch 'RsProxy direct'
$NetworkMode = 'RsProxy direct'

if (($FetchCode -ne 0) -and $ProxyAvailable) {
    Write-Host "RsProxy direct failed. Retrying RsProxy through $ProxyUrl ..." -ForegroundColor Yellow
    Enable-ProxyEnv
    $env:CARGO_NET_RETRY = '3'
    $env:CARGO_HTTP_TIMEOUT = '45'
    $FetchCode = Invoke-CargoFetch "RsProxy via proxy $ProxyUrl"
    $NetworkMode = 'RsProxy via proxy'
}

if ($FetchCode -ne 0) {
    Fail "Cargo dependency download from RsProxy failed. Local proxy checked: $ProxyUrl."
}

Clear-ProxyEnv

Write-Step 'Starting ComfyUI Launcher (Tauri development mode)'
Write-Host "Rust dependency route used: $NetworkMode" -ForegroundColor DarkGray
Write-Host 'Close the app window or press Ctrl+C here to stop.' -ForegroundColor DarkGray

Push-Location $Frontend
try {
    & npm run desktop:dev
    $ExitCode = $LASTEXITCODE
} finally { Pop-Location }

if ($ExitCode -ne 0) {
    Fail "Tauri exited with code $ExitCode. Rust dependencies were already prefetched, so the messages above should now be a real build/runtime error rather than a dependency download error."
}
