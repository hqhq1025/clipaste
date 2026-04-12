# clipaste installer for Windows
# Usage: irm https://raw.githubusercontent.com/hqhq1025/clipaste/main/install.ps1 | iex

$ErrorActionPreference = "Stop"

Write-Host "clipaste installer" -ForegroundColor Cyan
Write-Host ""

# Get latest release
$release = Invoke-RestMethod "https://api.github.com/repos/hqhq1025/clipaste/releases/latest"
$version = $release.tag_name
$asset = $release.assets | Where-Object { $_.name -like "*windows*" } | Select-Object -First 1

if (-not $asset) {
    Write-Host "Error: No Windows release found for $version" -ForegroundColor Red
    exit 1
}

Write-Host "Installing clipaste $version..." -ForegroundColor Green

# Create install directory
$installDir = "$env:LOCALAPPDATA\clipaste"
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

# Download and extract
$zipPath = "$env:TEMP\clipaste.zip"
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zipPath
Expand-Archive -Path $zipPath -DestinationPath $installDir -Force
Remove-Item $zipPath

$exePath = "$installDir\clipaste.exe"
if (-not (Test-Path $exePath)) {
    Write-Host "Error: clipaste.exe not found after extraction" -ForegroundColor Red
    exit 1
}

# Add to PATH (current user)
$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($userPath -notlike "*clipaste*") {
    [Environment]::SetEnvironmentVariable("PATH", "$userPath;$installDir", "User")
    Write-Host "Added to PATH" -ForegroundColor Green
}

# Set auto-start via Registry (HKCU, no admin needed)
$regPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
Set-ItemProperty -Path $regPath -Name "clipaste" -Value $exePath
Write-Host "Set to start on login" -ForegroundColor Green

# Start now
Start-Process -FilePath $exePath -WindowStyle Hidden
Write-Host ""
Write-Host "Done! clipaste $version installed and running." -ForegroundColor Cyan
Write-Host "  Binary: $exePath"
Write-Host "  Auto-start: Registry (HKCU\...\Run)"
Write-Host ""
Write-Host "To uninstall:" -ForegroundColor Yellow
Write-Host "  Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'clipaste'"
Write-Host "  Remove-Item -Recurse '$installDir'"
