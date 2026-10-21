# Builds a ready-to-copy folder at dist\glide\ (and dist\glide.zip).
# Usage (from the repo root):  powershell -ExecutionPolicy Bypass -File scripts\package.ps1
# If a glide.toml exists in the repo root it is included, so your settings
# and secret come along. It's gitignored; never commit it.

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    $out = Join-Path $root "dist\glide"
    New-Item -ItemType Directory $out -Force | Out-Null

    # Update the program files in place. An existing dist\glide\glide.toml is
    # the live config the app has been saving to, so it's never overwritten.
    Copy-Item target\release\glide.exe, target\release\glide-cli.exe, README.md, glide.example.toml $out -Force
    if ((Test-Path glide.toml) -and -not (Test-Path (Join-Path $out "glide.toml"))) { Copy-Item glide.toml $out }

    $zip = Join-Path $root "dist\glide.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path "$out\*" -DestinationPath $zip
    Write-Host "Packaged $out (and $zip)"
} finally {
    Pop-Location
}
