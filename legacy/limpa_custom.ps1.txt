param(
    [string[]]$targets,
    [switch]$dryRun = $false
)

$ErrorActionPreference = "Continue"

foreach ($pattern in $targets) {
    $expanded = [Environment]::ExpandEnvironmentVariables($pattern)
    if ($dryRun) {
        if (Test-Path $expanded) {
            Write-Host "[DRY RUN] Encontrado: $expanded" -ForegroundColor Yellow
        } else {
            Write-Host "[DRY RUN] NÃO encontrado: $expanded" -ForegroundColor DarkGray
        }
    }
    else {
        if (Test-Path $expanded) {
            try {
                Remove-Item $expanded -Recurse -Force -ErrorAction SilentlyContinue
                Write-Host "Removido: $expanded" -ForegroundColor Green
            }
            catch {
                Write-Host "Falha ao remover $expanded" -ForegroundColor Red
            }
        } else {
            Write-Host "NÃO encontrado: $expanded" -ForegroundColor DarkGray
        }
    }
}
