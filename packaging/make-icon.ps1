# Generate the PLACEHOLDER icon used by the shipped packages.
#
# There is no branding art in this repository, so this draws a deliberately
# plain, obviously provisional mark: a dark rounded tile with a lighter
# aperture/play glyph. Do NOT treat it as MetalVJ's identity; replace it with
# real art (same filename, 256x256 RGBA PNG) when branding lands.
#
#   powershell -ExecutionPolicy Bypass -File packaging/make-icon.ps1
#
# Writes: packaging/icons/MetalVJ.png (256x256 RGBA, also the AppDir icon)
#         dist/MetalVJ.ico          (multi-size, only when -Ico is given)
#
# Pure .NET (System.Drawing) so it needs no external tooling.

param(
    [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot),
    [switch]$Ico,
    [string]$Out = (Join-Path $PSScriptRoot "icons\MetalVJ.png")
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$size = 256
$bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)

# Rounded tile background.
function RoundRect([System.Drawing.RectangleF]$r, [float]$rad) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $rad * 2
    $p.AddArc($r.X, $r.Y, $d, $d, 180, 90)
    $p.AddArc($r.Right - $d, $r.Y, $d, $d, 270, 90)
    $p.AddArc($r.Right - $d, $r.Bottom - $d, $d, $d, 0, 90)
    $p.AddArc($r.X, $r.Bottom - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    $p
}

$tile = New-Object System.Drawing.RectangleF(8, 8, 240, 240)
$tilePath = RoundRect $tile 48
$bg = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    $tile,
    [System.Drawing.Color]::FromArgb(255, 24, 26, 32),
    [System.Drawing.Color]::FromArgb(255, 52, 58, 72),
    90.0)
$g.FillPath($bg, $tilePath)

# Aperture ring.
$accent = [System.Drawing.Color]::FromArgb(255, 92, 214, 255)
$ringPen = New-Object System.Drawing.Pen($accent, 14)
$g.DrawEllipse($ringPen, 56, 56, 144, 144)

# Play triangle through the ring (provisional "VJ" cue).
$pts = @(
    (New-Object System.Drawing.PointF(110, 92)),
    (New-Object System.Drawing.PointF(110, 164)),
    (New-Object System.Drawing.PointF(168, 128))
)
$triBrush = New-Object System.Drawing.SolidBrush($accent)
$g.FillPolygon($triBrush, $pts)

# Corner tick marks to read as a placeholder/test pattern, not a logo.
$tickPen = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(120, 255, 255, 255), 4)
$g.DrawLine($tickPen, 40, 40, 40, 64)
$g.DrawLine($tickPen, 40, 40, 64, 40)
$g.DrawLine($tickPen, 216, 216, 216, 192)
$g.DrawLine($tickPen, 216, 216, 192, 216)

$g.Dispose()

$outFull = [System.IO.Path]::GetFullPath($Out)
$outDir = Split-Path -Parent $outFull
if (-not (Test-Path -LiteralPath $outDir)) { New-Item -ItemType Directory -Force -Path $outDir | Out-Null }
$bmp.Save($outFull, [System.Drawing.Imaging.ImageFormat]::Png)
Write-Host "wrote $outFull ($size x $size, placeholder)"

if ($Ico) {
    $dist = Join-Path $RepoRoot "dist"
    if (-not (Test-Path -LiteralPath $dist)) { New-Item -ItemType Directory -Force -Path $dist | Out-Null }
    # 32bpp ARGB icon; Windows accepts a PNG-compressed payload inside ICO,
    # which keeps this to the one bitmap we already have.
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $png = $ms.ToArray()
    $ms.Dispose()
    $icoStream = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($icoStream)
    $bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]1)   # reserved, type=icon, count
    $bw.Write([byte]0); $bw.Write([byte]0)                             # 0 means 256
    $bw.Write([byte]0); $bw.Write([byte]0)                             # palette, reserved
    $bw.Write([uint16]1); $bw.Write([uint16]32)                        # planes, bpp
    $bw.Write([uint32]$png.Length); $bw.Write([uint32](6 + 16))        # bytes, offset
    $bw.Write($png)
    $bw.Flush()
    [System.IO.File]::WriteAllBytes((Join-Path $dist "MetalVJ.ico"), $icoStream.ToArray())
    $bw.Dispose(); $icoStream.Dispose()
    Write-Host "wrote $dist\MetalVJ.ico"
}

$bmp.Dispose()
