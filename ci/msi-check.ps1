# Installs the MSI per user, checks both executables, the PATH entry and the Start menu
# shortcut, runs `wdc` from a fresh shell through PATH, then uninstalls and checks that
# everything is gone.
param([Parameter(Mandatory = $true)][string]$Msi, [Parameter(Mandatory = $true)][string]$Version)
$ErrorActionPreference = 'Stop'

$msi = (Resolve-Path $Msi).Path
# The log goes to the runner's temp folder, never next to the MSI (a release uploads that folder).
$log = Join-Path ([IO.Path]::GetTempPath()) 'msi-install.log'
$p = Start-Process msiexec.exe -ArgumentList @('/i', "`"$msi`"", '/qn', '/l*v', "`"$log`"") -Wait -PassThru
if ($p.ExitCode -ne 0) { Get-Content $log -Tail 40; throw "install failed with exit code $($p.ExitCode)" }

$dir = Join-Path $env:LOCALAPPDATA 'Programs\WinDevCleaner'
foreach ($f in 'wdc.exe', 'WinDevCleaner.exe') {
    if (-not (Test-Path (Join-Path $dir $f))) { throw "$f was not installed in $dir" }
}
$userPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
if (($userPath -split ';') -notcontains $dir.TrimEnd('\') -and ($userPath -split ';') -notcontains "$dir\") {
    throw "the user PATH does not contain $dir : $userPath"
}
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'WinDevCleaner.lnk'
if (-not (Test-Path $shortcut)) { throw "Start menu shortcut missing: $shortcut" }

# A new shell finds `wdc` through the user PATH written by the installer.
$machinePath = [Environment]::GetEnvironmentVariable('PATH', 'Machine')
$out = & cmd.exe /c "set `"PATH=$machinePath;$userPath`" && wdc --version"
if ($LASTEXITCODE -ne 0 -or $out -ne "wdc $Version") { throw "wdc through PATH printed '$out'" }
Write-Host "installed: $out, PATH and shortcut in place"

$p = Start-Process msiexec.exe -ArgumentList @('/x', "`"$msi`"", '/qn') -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "uninstall failed with exit code $($p.ExitCode)" }
if (Test-Path (Join-Path $dir 'wdc.exe')) { throw 'wdc.exe still present after uninstall' }
if (Test-Path $shortcut) { throw 'shortcut still present after uninstall' }
$userPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
if ($userPath -and ($userPath -split ';' | Where-Object { $_.TrimEnd('\') -eq $dir.TrimEnd('\') })) {
    throw "PATH still contains $dir after uninstall"
}
Write-Host 'uninstalled cleanly'
