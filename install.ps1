[CmdletBinding()]
param (
  [string]$InstallDir = "$env:LOCALAPPDATA\previewproxy\bin",
  [string]$Version = "latest"
)

$ErrorActionPreference = "Stop"
$Repo = "vigrise/previewproxy"
$BinName = "previewproxy.exe"

# Detect architecture
$Arch = switch ($env:PROCESSOR_ARCHITECTURE) {
  "AMD64" { "x86_64" }
  "ARM64" { "aarch64" }
  default { Write-Error "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE"; exit 1 }
}

$Artifact = "previewproxy-windows-$Arch.exe"
$UrlBase = if ($Version -eq "latest") {
  "https://github.com/$Repo/releases/latest/download"
} else {
  "https://github.com/$Repo/releases/download/$Version"
}
$Url = "$UrlBase/$Artifact"
$Dest = Join-Path $InstallDir $BinName

if (-not (Test-Path $InstallDir)) {
  New-Item -ItemType Directory -Path $InstallDir | Out-Null
}

Write-Host "Downloading $Artifact..."
Invoke-WebRequest -Uri $Url -OutFile $Dest -UseBasicParsing

# Add to PATH for current user if not already present
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$InstallDir*") {
  [Environment]::SetEnvironmentVariable("PATH", "$UserPath;$InstallDir", "User")
  Write-Host "Added $InstallDir to PATH (restart your shell to apply)"
}

Write-Host "Installed to $Dest"
& $Dest --version
