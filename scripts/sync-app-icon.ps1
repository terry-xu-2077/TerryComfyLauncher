# Regenerate frontend\src-tauri\icons\* from the single source image
# frontend\src-tauri\app-icon.png using the official Tauri icon command.
#
# Detection is fingerprint-based: the SHA256 of app-icon.png is stored in
# icons/.app-icon.sha256. Whenever the source PNG changes (or the generated
# icons are missing), the platform icons are regenerated; otherwise the step
# is skipped so repeated builds stay fast and do not churn file timestamps
# (which would force a needless cargo relink).
#
# Usage: .\sync-app-icon.ps1 -Frontend <abs path to frontend dir>
param(
    [Parameter(Mandatory = $true)]
    [string]$Frontend
)

$ErrorActionPreference = 'Stop'

$IconSource = Join-Path $Frontend 'src-tauri\app-icon.png'
$IconOut = Join-Path $Frontend 'src-tauri\icons'
$Stamp = Join-Path $IconOut '.app-icon.sha256'

if (-not (Test-Path $IconSource)) { throw "App icon source is missing: $IconSource" }

$SourceHash = (Get-FileHash -Algorithm SHA256 $IconSource).Hash
$BuiltHash = if (Test-Path $Stamp) { (Get-Content $Stamp -Raw).Trim() } else { '' }
$MissingOut = -not ((Test-Path (Join-Path $IconOut 'icon.ico')) -and (Test-Path (Join-Path $IconOut 'icon.png')))

if ((-not $MissingOut) -and ($SourceHash -eq $BuiltHash)) {
    Write-Host 'App icons are up to date.' -ForegroundColor DarkGray
    return
}

if ($MissingOut) {
    Write-Host "`n==> Generating application icons from app-icon.png" -ForegroundColor Cyan
} else {
    Write-Host "`n==> app-icon.png changed - regenerating application icons" -ForegroundColor Cyan
    Write-Host "    old fingerprint: $BuiltHash" -ForegroundColor DarkGray
    Write-Host "    new fingerprint: $SourceHash" -ForegroundColor DarkGray
}

New-Item -ItemType Directory -Path $IconOut -Force | Out-Null
Push-Location $Frontend
try {
    & npm run tauri -- icon "src-tauri/app-icon.png" --output "src-tauri/icons"
    if ($LASTEXITCODE -ne 0) { throw 'Tauri icon generation failed.' }
} finally {
    Pop-Location
}

# Written only on success, so a failed run retries on the next build.
Set-Content -Path $Stamp -Value $SourceHash -NoNewline
Write-Host "Icons written to $IconOut" -ForegroundColor Green
