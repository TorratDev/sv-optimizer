param([string]$GamePath = "", [switch]$InstallMod)
$ErrorActionPreference = "Stop"
$projectDir = Split-Path -Parent $PSScriptRoot
function Run-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw "Install current stable Rust from https://rustup.rs/" }
if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) { throw "Install the .NET 8 SDK from https://dotnet.microsoft.com/download/dotnet/8.0" }
if (-not $GamePath) {
    foreach ($candidate in @("${env:ProgramFiles(x86)}\Steam\steamapps\common\Stardew Valley", "$env:ProgramFiles\Steam\steamapps\common\Stardew Valley")) {
        if (Test-Path (Join-Path $candidate "Stardew Valley.dll")) { $GamePath = $candidate; break }
    }
}
Push-Location $projectDir
try {
    Run-Checked "cargo" @("build", "--release", "--locked")
    if ($GamePath) {
        if (-not (Test-Path (Join-Path $GamePath "StardewModdingAPI.dll"))) { throw "Install SMAPI into $GamePath using https://smapi.io/ first." }
        Run-Checked "dotnet" @("build", "mod/Observer/Observer.csproj", "-c", "Release", "-p:GamePath=$GamePath")
        if ($InstallMod) {
            $destination = Join-Path $GamePath "Mods\SvOptimizer.Observer"
            $backup = $null
            if (Test-Path $destination) {
                $backup = "$destination.backup.$(Get-Date -Format yyyyMMddTHHmmss)"
                if (Test-Path $backup) { throw "Backup already exists: $backup" }
                Move-Item $destination $backup
            }
            New-Item -ItemType Directory -Path $destination | Out-Null
            Copy-Item "mod/Observer/bin/Release/net6.0/*" $destination -Recurse
            if ($backup -and (Test-Path (Join-Path $backup "config.json"))) { Copy-Item (Join-Path $backup "config.json") $destination }
            Write-Host "Observer installed in $destination"
        }
    } elseif ($InstallMod) { throw "Game not detected; pass -GamePath DIRECTORY." }
    else { Write-Host "Game not detected. Rust built; pass -GamePath DIRECTORY to build the observer." }
    Run-Checked "target/release/sv-optimizer.exe" @("doctor", "--hardware-only")
    Write-Host "Install/run Ollama and explicitly download the recommended model. Then run target/release/sv-optimizer.exe chat."
} finally { Pop-Location }
