# Build PulseCut for Windows and produce a zip (plus an installer if Inno Setup is installed).
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
cargo build --release -p pulsecut-app -p pulsecut-cli
$dist = "target/dist/PulseCut"
Remove-Item -Recurse -Force $dist -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$dist/models" | Out-Null
Copy-Item target/release/PulseCut.exe, target/release/pulsecut-cli.exe $dist
Copy-Item README.md, LICENSE, docs/USER_GUIDE.md, docs/LICENSING.md $dist
Compress-Archive -Force -Path "$dist/*" -DestinationPath "target/dist/PulseCut-windows-x64.zip"
$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
if (Test-Path $iscc) { & $iscc packaging/windows/pulsecut.iss }
Write-Host "built target/dist"
