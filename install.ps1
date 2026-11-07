# Umless installer for Windows: builds Umless, downloads the model tiers you choose,
# installs umless.exe with a Start menu shortcut and opens it.
#
#   powershell -ExecutionPolicy Bypass -File install.ps1            # asks which tier
#   powershell -ExecutionPolicy Bypass -File install.ps1 lite       # lite | standard | max | all
#
# Needs: Rust (rustup), the Visual Studio C++ build tools, cmake and LLVM (libclang, for
# llama.cpp's bindings). Missing ones are installed with winget after asking.
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Tiers)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

function Say($msg) { Write-Host $msg -ForegroundColor Cyan }
function Have($cmd) { [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }
function Refresh-Path {
  $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
              [Environment]::GetEnvironmentVariable('Path', 'User') + ";$env:USERPROFILE\.cargo\bin"
}
function Winget-Install($id, $what, $extra = @()) {
  if (-not (Have winget)) { throw "Install $what and run install.ps1 again (winget isn't available to do it for you)." }
  $yn = Read-Host "$what isn't installed. Install it now with winget ($id)? [Y/n]"
  if ($yn -and $yn -notmatch '^[Yy]') { throw "Install $what and run install.ps1 again." }
  winget install --id $id -e --accept-package-agreements --accept-source-agreements @extra
  Refresh-Path
}

# 1. Toolchain ---------------------------------------------------------------
Refresh-Path
if (-not (Have cargo)) { Winget-Install 'Rustlang.Rustup' 'Rust' }
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasVC = (Test-Path $vswhere) -and (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
if (-not $hasVC) {
  Winget-Install 'Microsoft.VisualStudio.2022.BuildTools' 'the Visual Studio C++ build tools' `
    @('--override', '--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended')
}
if (-not (Have cmake)) { Winget-Install 'Kitware.CMake' 'cmake' }
if (-not $env:LIBCLANG_PATH -and -not (Test-Path "$env:ProgramFiles\LLVM\bin\libclang.dll")) {
  Winget-Install 'LLVM.LLVM' 'LLVM (libclang)'
}
if (-not $env:LIBCLANG_PATH -and (Test-Path "$env:ProgramFiles\LLVM\bin\libclang.dll")) {
  $env:LIBCLANG_PATH = "$env:ProgramFiles\LLVM\bin"
}

# 2. Tiers -------------------------------------------------------------------
if (-not $Tiers) {
  $ram = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)
  $rec = if ($ram -lt 8) { 'lite' } else { 'standard' }
  Say "Which model tier? (this PC: $ram GB RAM -> recommended: $rec)"
  Write-Host '  1) lite      ~650 MB  fastest, any laptop (English)'
  Write-Host '  2) standard  ~1.8 GB  best balance (25 European languages)'
  Write-Host '  3) max       ~3.1 GB  most accurate, 99 languages incl. Hindi; best with a fast GPU'
  Write-Host '  4) all       ~5.5 GB  all three (to compare them with `umless bench`)'
  $c = Read-Host "Choose 1-4 [$rec]"
  $Tiers = @(switch ($c) { '1' { 'lite' } '2' { 'standard' } '3' { 'max' } '4' { 'all' } '' { $rec } default { $c } })
}

# 3. Build -------------------------------------------------------------------
Say 'Building Umless (the first build takes several minutes)...'
cargo build --release
if ($LASTEXITCODE) { throw 'The build failed.' }
$bin = Join-Path $PSScriptRoot 'target\release\umless.exe'

# 4. Models ------------------------------------------------------------------
$models = if ($env:UMLESS_MODELS) { $env:UMLESS_MODELS } else { Join-Path $PSScriptRoot 'models' }
Say "Downloading models: $($Tiers -join ' ')"
& $bin --models $models models install @Tiers
if ($LASTEXITCODE) { throw 'The model download failed.' }
$tier = if ($Tiers[0] -eq 'all') { 'standard' } else { $Tiers[0] }

# 5. Install -----------------------------------------------------------------
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Umless'
New-Item -ItemType Directory -Force $dest | Out-Null
Get-Process umless -ErrorAction SilentlyContinue | Stop-Process -Force
Copy-Item $bin (Join-Path $dest 'umless.exe') -Force
# sherpa-onnx (speech) and onnxruntime are DLLs on Windows: they go next to the exe.
Get-ChildItem (Join-Path $PSScriptRoot 'target\release') -Filter *.dll | Copy-Item -Destination $dest -Force
$exe = Join-Path $dest 'umless.exe'
$appArgs = "--models `"$models`" --tier $tier app --window"

$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath('Programs')) 'Umless.lnk'))
$lnk.TargetPath = $exe
$lnk.Arguments = $appArgs
$lnk.WorkingDirectory = $dest
$lnk.WindowStyle = 7  # minimized: Umless lives in the tray
$lnk.Description = 'Local voice dictation'
$lnk.Save()

# umless on PATH for terminal use.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$dest*") {
  [Environment]::SetEnvironmentVariable('Path', "$userPath;$dest", 'User')
}

Start-Process $exe -ArgumentList $appArgs -WindowStyle Minimized
Say 'Done. Umless is in the tray (bottom right). Hold Ctrl + Win anywhere and speak; let go to type.'
Write-Host '  Ctrl + Win + Alt is command mode. Set UMLESS_HOLD_KEY=right-ctrl (or right-alt) to use another key.'
Write-Host '  Start it from the Start menu (Umless). Re-run install.ps1 any time to update.'
Write-Host "  Add a tier later: umless --models `"$models`" models install max"
