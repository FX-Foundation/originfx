# Build the one-file Windows test app: dist\OriginFX-TestApp-windows-x86_64.exe
# Runs on a GitHub windows runner (needs gh, cargo, python, LLVM). FFmpeg 6.1 shared libraries
# come from the BtbN builds, because the engine's FFmpeg bindings target version 6.1.
$ErrorActionPreference = 'Continue'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $root
$log = Join-Path $root 'build.log'
New-Item -ItemType File -Force $log | Out-Null

function Fail($why) {
    Write-Host "::error::$why"
    Get-Content $log -ErrorAction SilentlyContinue | Select-String -Pattern 'error|panicked|not found|cannot|unresolved|failed' |
        Select-Object -First 8 | ForEach-Object { Write-Host ("::error::" + $_.Line.Trim()) }
    exit 1
}

function Run([string]$what, [scriptblock]$cmd) {
    Write-Host "=== $what"
    & $cmd 2>&1 | Tee-Object -FilePath $log -Append | ForEach-Object { Write-Host $_ }
    if ($LASTEXITCODE -ne 0) { Fail "$what failed (exit $LASTEXITCODE)" }
}

# 1. FFmpeg 6.1 shared build (headers, import libraries, DLLs).
$names = & gh release view latest -R BtbN/FFmpeg-Builds --json assets --jq '.assets[].name' 2>&1
if ($LASTEXITCODE -ne 0) { Fail "could not list BtbN releases: $names" }
$name = $names | Where-Object { $_ -match '^ffmpeg-.*win64-gpl-shared-6\.1\.zip$' } | Select-Object -First 1
if (-not $name) {
    $sample = ($names | Where-Object { $_ -match 'win64-gpl-shared' } | Select-Object -First 6) -join ', '
    Fail "no FFmpeg 6.1 win64 gpl-shared build found. Available: $sample"
}
Write-Host "FFmpeg build: $name"
Run 'download FFmpeg' { gh release download latest -R BtbN/FFmpeg-Builds -p $name -D ffmpeg-dl --clobber }
Expand-Archive -Path (Join-Path 'ffmpeg-dl' $name) -DestinationPath 'ffmpeg-x' -Force
$ff = (Get-ChildItem 'ffmpeg-x' -Directory | Select-Object -First 1).FullName
$env:FFMPEG_DIR = $ff
if (-not (Test-Path (Join-Path $ff 'lib\avcodec.lib'))) { Fail "avcodec.lib missing in $ff\lib" }
if (-not $env:LIBCLANG_PATH) { $env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin' }

# 2. The app.
Run 'build the app' { cargo build --release -p originfx-testapp }

# 3. Payload: the app and the FFmpeg DLLs.
$stage = Join-Path $root 'stage'
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $stage | Out-Null
Copy-Item 'target\release\originfx-testapp.exe' $stage
Copy-Item (Join-Path $ff 'bin\*.dll') $stage
Write-Host ("carrying " + (Get-ChildItem $stage).Count + " files")
Run 'zip the payload' { python packaging\make_zip.py $stage (Join-Path $root 'payload.zip') }

# 4. The one-file launcher with the payload inside.
$env:ORIGINFX_PAYLOAD = Join-Path $root 'payload.zip'
Run 'build the launcher' { cargo build --release --manifest-path packaging\launcher\Cargo.toml }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item 'packaging\launcher\target\release\originfx-launcher.exe' 'dist\OriginFX-TestApp-windows-x86_64.exe'
Get-ChildItem dist | Format-Table Name, Length
