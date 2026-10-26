# Regenerates crates/app/assets/emoji.json: Unicode emoji with Discord's names and picker categories.
#
# Data comes from emojibase-data (MIT, Miles Johnson). Discord's emoji names are, with a few exceptions, the
# JoyPixels shortcodes (with "_" for "-"), and for emoji newer than JoyPixels' list the Unicode name in
# snake_case, so those are used. Emoji newer than Discord supports (MaxVersion) are left out.
#
# Each entry is [emoji, name, other names, keywords, category, [5 skin tones]?]; names are space-separated.
param(
    [string]$Version = "17.0.0",
    [double]$MaxVersion = 16.0,
    [string]$SheetVersion = "16.0.0",
    [string]$Out = "$PSScriptRoot\..\crates\app\assets\emoji.json"
)
$ErrorActionPreference = "Stop"
$base = "https://cdn.jsdelivr.net/npm/emojibase-data@$Version/en"
function Get-Json($url) { [Text.Encoding]::UTF8.GetString((Invoke-WebRequest $url -UseBasicParsing).RawContentStream.ToArray()) | ConvertFrom-Json }
$data = Get-Json "$base/data.json"
$joy = Get-Json "$base/shortcodes/joypixels.json"
$cldr = Get-Json "$base/shortcodes/cldr.json"

# Discord's categories, in its picker's order, from emojibase's groups (2 is skin-tone components: skipped).
$categories = @("People", "Nature", "Food", "Activities", "Travel", "Objects", "Symbols", "Flags")
$groupToCategory = @{ 0 = 0; 1 = 0; 3 = 1; 4 = 2; 6 = 3; 5 = 4; 7 = 5; 8 = 6; 9 = 7 }

function Codes($hex) { $hex -split '-' | ForEach-Object { [Convert]::ToInt32($_, 16) } }
function Text($codes) { -join ($codes | ForEach-Object { [char]::ConvertFromUtf32($_) }) }

# How Discord writes an emoji: as emojibase's hexcode, plus the emoji presentation selector on a lone symbol
# that is text by default (❤ → ❤️). Discord drops the selectors from 👁️‍🗨️.
function Discord-Form($e) {
    if ($e.hexcode -eq "1F441-FE0F-200D-1F5E8-FE0F") { return Text (Codes "1F441-200D-1F5E8") }
    $codes = @(Codes $e.hexcode)
    if ($e.type -eq 0 -and $codes.Count -eq 1 -and $codes[0] -notin 0xA9, 0xAE, 0x2122, 0x26A7) { $codes += 0xFE0F }
    Text $codes
}

function Shortcodes($table, $hex) {
    $p = $table.PSObject.Properties[$hex]
    if (-not $p) { return @() }
    @($p.Value) | ForEach-Object { $_ -replace '(?<=[a-z0-9])-(?=[a-z0-9])', '_' }
}

function Snake($label) { (($label.ToLowerInvariant() -replace '[^a-z0-9+]+', '_').Trim('_')) }

$entries = New-Object System.Collections.Generic.List[object]
foreach ($e in $data | Sort-Object order) {
    if ($null -eq $e.group -or -not $groupToCategory.ContainsKey([int]$e.group)) { continue }
    if ([double]$e.version -gt $MaxVersion) { continue }
    $names = @(Shortcodes $joy $e.hexcode)
    if ($names.Count -eq 0) { $names = @(Snake $e.label) }
    $aliases = @($names | Select-Object -Skip 1) + @(Shortcodes $cldr $e.hexcode) | Where-Object { $_ -and $_ -ne $names[0] } | Select-Object -Unique
    $keywords = (@($e.label.ToLowerInvariant()) + @($e.tags)) -join ' '
    $entry = @((Discord-Form $e), $names[0], (@($aliases) -join ' '), $keywords, $groupToCategory[[int]$e.group])
    # Uniform skin tones only; Discord's picker doesn't offer mixed ones.
    $skins = @($e.skins | Where-Object { $_.tone -is [int] -or $_.tone -is [long] } | Sort-Object tone | ForEach-Object { Discord-Form $_ })
    if ($skins.Count -eq 5) { $entry += , $skins }
    $entries.Add($entry)
}
# Discord also offers the regional indicator letters (🇦…🇿), at the end of Symbols.
$symbolsEnd = 0
for ($i = 0; $i -lt $entries.Count; $i++) { if ($entries[$i][4] -eq 6) { $symbolsEnd = $i + 1 } }
$letters = 0..25 | ForEach-Object {
    $l = [char](97 + $_)
    , @((Text @(0x1F1E6 + $_)), "regional_indicator_$l", "", "letter $l", 6)
}
$entries.InsertRange($symbolsEnd, [object[]]$letters)

# Where each emoji is on emoji-datasource's Twemoji sprite sheets (MIT; Twemoji graphics CC-BY 4.0), keyed
# by the emoji without its presentation selectors. The client draws emoji from a sheet, fetched once.
$sheetData = Get-Json "https://cdn.jsdelivr.net/npm/emoji-datasource-twitter@$SheetVersion/emoji.json"
$positions = [ordered]@{}
function Add-Position($unified, $x, $y) {
    $key = Text @(Codes $unified | Where-Object { $_ -ne 0xFE0F })
    if (-not $positions.Contains($key)) { $positions[$key] = @($x, $y) }
}
foreach ($d in $sheetData) {
    if (-not $d.has_img_twitter) { continue }
    Add-Position $d.unified $d.sheet_x $d.sheet_y
    if ($d.skin_variations) {
        foreach ($s in $d.skin_variations.PSObject.Properties.Value) {
            if ($s.has_img_twitter) { Add-Position $s.unified $s.sheet_x $s.sheet_y }
        }
    }
}
$sheet = [ordered]@{ version = $SheetVersion; cells = 62; positions = $positions }

$json = [ordered]@{ source = "emojibase-data $Version (MIT, Miles Johnson); names as Discord uses them"; groups = $categories; emoji = $entries; sheet = $sheet }
$text = $json | ConvertTo-Json -Depth 5 -Compress
[IO.File]::WriteAllText((Resolve-Path -LiteralPath (Split-Path $Out)).Path + "\" + (Split-Path $Out -Leaf), $text, (New-Object Text.UTF8Encoding $false))
"$($entries.Count) emoji written to $Out"
