@echo off
cd /d "%~dp0"
if not exist "photocraft.exe" (
  echo photocraft.exe was not found. Please extract the whole archive first.
  pause
  exit /b 1
)
echo PhotoCraft Generative Fill - enter your fal.ai API key below.
echo The key will be hidden and is not saved to a file.
powershell.exe -NoProfile -Command "$secure = Read-Host 'fal.ai API key' -AsSecureString; $key = [System.Net.NetworkCredential]::new('', $secure).Password; if ([string]::IsNullOrWhiteSpace($key)) { Write-Error 'No API key entered'; exit 1 }; $info = [System.Diagnostics.ProcessStartInfo]::new((Join-Path (Get-Location).Path 'photocraft.exe')); $info.UseShellExecute = $false; $info.WorkingDirectory = (Get-Location).Path; $info.EnvironmentVariables['FAL_KEY'] = $key; $child = [System.Diagnostics.Process]::Start($info); if ($null -eq $child) { Write-Error 'Failed to launch PhotoCraft'; exit 1 }"
if errorlevel 1 (
  echo Could not start PhotoCraft. Check the message above.
  pause
)
