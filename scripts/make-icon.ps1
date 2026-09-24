# Generates frontend\src-tauri\app-icon.png (1024x1024) with System.Drawing:
# rounded-square blue-violet gradient + white play triangle, matching the UI style.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$Root = Split-Path -Parent $PSScriptRoot
$Out = Join-Path $Root 'frontend\src-tauri\app-icon.png'

$Size = 1024
$bmp = New-Object System.Drawing.Bitmap($Size, $Size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)

function New-RoundedRectPath([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $r * 2
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

# gradient rounded square (inset so tauri icon padding looks right)
$pad = 40
$rect = New-RoundedRectPath $pad $pad ($Size - 2 * $pad) ($Size - 2 * $pad) 240
$brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.PointF(0, 0)),
    (New-Object System.Drawing.PointF($Size, $Size)),
    [System.Drawing.Color]::FromArgb(106, 139, 255),
    [System.Drawing.Color]::FromArgb(143, 107, 255)
)
$g.FillPath($brush, $rect)

# subtle top highlight
$hl = New-RoundedRectPath ($pad + 24) ($pad + 24) ($Size - 2 * $pad - 48) (($Size - 2 * $pad) / 2) 210
$hlBrush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.PointF(0, ([float]$pad))),
    (New-Object System.Drawing.PointF(0, ([float]($Size / 2)))),
    [System.Drawing.Color]::FromArgb(60, 255, 255, 255),
    [System.Drawing.Color]::FromArgb(0, 255, 255, 255)
)
$g.FillPath($hlBrush, $hl)

# white play triangle (optically centered: shifted slightly right)
$cx = [float]($Size / 2 + 30)
$cy = [float]($Size / 2)
$tw = 250   # half-width
$th = 290   # half-height
$tri = New-Object System.Drawing.Drawing2D.GraphicsPath
$tri.AddLine($cx - $tw + 60, $cy - $th, $cx - $tw + 60, $cy + $th)
$tri.AddLine($cx - $tw + 60, $cy + $th, $cx + $tw, $cy)
$tri.CloseFigure()
$white = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
$g.FillPath($white, $tri)

$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Host "Icon written: $Out" -ForegroundColor Green
