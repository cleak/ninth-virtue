param(
    [Parameter(Mandatory = $true)]
    [string]$GameDir,

    [string]$OutputPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$resolvedGameDir = (Resolve-Path -LiteralPath $GameDir).Path
$patterns = @("ULTIMA.EXE", "DATA.OVL", "*.OVL", "dosbox*.conf")

$files = Get-ChildItem -LiteralPath $resolvedGameDir -File |
    Where-Object {
        $name = $_.Name
        foreach ($pattern in $patterns) {
            if ($name -like $pattern) {
                return $true
            }
        }
        return $false
    } |
    Sort-Object Name -Unique

if (-not $files) {
    throw "No Ultima V artifacts matched under $resolvedGameDir"
}

$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add("# Visibility Artifact Freeze")
$lines.Add("")
$lines.Add("- Generated: $(Get-Date -Format o)")
$lines.Add("- Game Dir: $resolvedGameDir")
$lines.Add("")
$lines.Add("| File | Size | SHA-256 |")
$lines.Add("|---|---:|---|")

foreach ($file in $files) {
    $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    $lines.Add("| $($file.Name) | $($file.Length) | ``$hash`` |")
}

$content = $lines -join [Environment]::NewLine
if ($OutputPath) {
    Set-Content -LiteralPath $OutputPath -Value $content -Encoding UTF8
} else {
    $content
}
