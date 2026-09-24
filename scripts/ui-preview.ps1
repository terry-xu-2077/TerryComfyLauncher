$ErrorActionPreference = 'Stop'

$Root     = Split-Path -Parent $PSScriptRoot
$Frontend = Join-Path $Root 'frontend'
$Port     = 1420

function Write-Step([string]$Text) { Write-Host "`n==> $Text" -ForegroundColor Cyan }
function Fail([string]$Text) {
    Write-Host "`n[ERROR] $Text" -ForegroundColor Red
    if ([Environment]::UserInteractive) { try { $null = $Host.UI.RawUI.ReadKey('NoEcho,IncludeKeyDown') } catch { } }
    exit 1
}

Set-Location $Root

if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
    Fail 'npm was not found. Install Node.js LTS first.'
}

if (-not (Test-Path (Join-Path $Frontend 'node_modules'))) {
    Write-Step 'node_modules missing - installing frontend dependencies'
    Push-Location $Frontend
    try {
        & npm install --package-lock=false
        if ($LASTEXITCODE -ne 0) { Pop-Location; Fail 'npm install failed.' }
    } finally { if ($PWD.Path -eq $Frontend) { Pop-Location } }
}

# strictPort is enabled in vite.config.js, so a stale server would hard-fail.
# Detect it early and report who owns the port instead of crashing.
$owner = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue
if ($owner) {
    $pid2 = $owner[0].OwningProcess
    $name = (Get-Process -Id $pid2 -ErrorAction SilentlyContinue).ProcessName
    Write-Host "[WARN] Port $Port is already in use by $name (PID $pid2)." -ForegroundColor Yellow
    Write-Host "       Reusing it - if that window shows the old UI, kill PID $pid2 and run again." -ForegroundColor Yellow
}

# Some proxies (incl. the env vars set by other dev tools) intercept loopback
# traffic and answer localhost requests with 502. Force loopback to bypass.
$env:NO_PROXY  = 'localhost,127.0.0.1'
$env:no_proxy  = 'localhost,127.0.0.1'
$env:HTTP_PROXY  = $null
$env:HTTPS_PROXY = $null

Write-Step "Starting Vite dev server on http://localhost:$Port"
Write-Host 'Tip: this mode uses the in-browser mock in frontend\src\api.js (no Rust backend).' -ForegroundColor DarkGray
Write-Host '     For the real window frame + real backend, use 启动项目.bat instead.' -ForegroundColor DarkGray
Write-Host ''

Start-Sleep -Milliseconds 400
Start-Process "http://localhost:$Port"

Push-Location $Frontend
try {
    & npm run dev -- --open
} finally {
    Pop-Location
}
