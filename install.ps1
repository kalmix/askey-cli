# Askey Installer Script

$installDir = Join-Path $HOME ".askey\bin"
$exePath = Join-Path $PSScriptRoot "target\release\askey.exe"

if (-not (Test-Path $exePath)) {
    Write-Error "Release binary not found at $exePath. Please run 'cargo build --release' first."
    exit 1
}

if (-not (Test-Path $installDir)) {
    New-Item -ItemType Directory -Path $installDir | Out-Null
    Write-Host "Created installation directory at $installDir" -ForegroundColor Green
}

$destExe = Join-Path $installDir "askey.exe"
Copy-Item -Path $exePath -Destination $destExe -Force
Write-Host "Successfully installed askey.exe to $destExe" -ForegroundColor Green

$userPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
$cleanInstallDir = (Resolve-Path $installDir).Path

$paths = $userPath -split ';' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" }

if ($paths -notcontains $cleanInstallDir) {
    $newPath = ($paths + $cleanInstallDir) -join ';'
    [System.Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Write-Host "Successfully added $cleanInstallDir to your user PATH environment variable." -ForegroundColor Green
    Write-Warning "Please restart your terminal/editor to start using the 'askey' command globally!"
} else {
    Write-Host "$cleanInstallDir is already registered in your user PATH." -ForegroundColor Cyan
    Write-Host "You can use the 'askey' command globally in any new terminal window!" -ForegroundColor Green
}
