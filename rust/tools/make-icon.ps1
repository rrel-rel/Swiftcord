# Draws the placeholder app icon (a blurple rounded square with a white "S") and writes
# crates/app/assets/icon.ico (16–256 px) and icon.png (256 px). Replace them with real artwork any time.
param([string]$Out = "$PSScriptRoot\..\crates\app\assets")
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

function Draw([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = "AntiAlias"
    $g.TextRenderingHint = "AntiAliasGridFit"
    $g.Clear([System.Drawing.Color]::Transparent)
    $r = [single]($size * 0.22)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $w = [single]($size - 1)
    $path.AddArc(0, 0, $r * 2, $r * 2, 180, 90)
    $path.AddArc($w - $r * 2, 0, $r * 2, $r * 2, 270, 90)
    $path.AddArc($w - $r * 2, $w - $r * 2, $r * 2, $r * 2, 0, 90)
    $path.AddArc(0, $w - $r * 2, $r * 2, $r * 2, 90, 90)
    $path.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0x58, 0x65, 0xF2))), $path)
    $font = New-Object System.Drawing.Font "Segoe UI", ([single]($size * 0.62)), ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)
    $fmt = New-Object System.Drawing.StringFormat
    $fmt.Alignment = "Center"; $fmt.LineAlignment = "Center"
    $rect = New-Object System.Drawing.RectangleF 0, ([single]($size * 0.02)), $size, $size
    $g.DrawString("S", $font, [System.Drawing.Brushes]::White, $rect, $fmt)
    $g.Dispose()
    $bmp
}

function Png($bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    , $ms.ToArray()
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$images = foreach ($s in $sizes) { , (Png (Draw $s)) }

# ICO: a header, a directory entry per image, then the PNGs.
$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter $ms
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]
    $bw.Write([byte]($s % 256)); $bw.Write([byte]($s % 256)); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32)
    $bw.Write([uint32]$images[$i].Length); $bw.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $bw.Write($img) }
$bw.Flush()
[IO.File]::WriteAllBytes((Join-Path $Out "icon.ico"), $ms.ToArray())
[IO.File]::WriteAllBytes((Join-Path $Out "icon.png"), $images[-1])
"icon.ico and icon.png written to $Out"
