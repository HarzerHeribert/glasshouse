param(
    [string]$Archive = "$env:USERPROFILE\glasshouse-source.tar.gz"
)
$ErrorActionPreference = 'Stop'
$ciRoot = 'C:\ci'
$destination = Join-Path $ciRoot 'glasshouse'
$next = Join-Path $ciRoot 'glasshouse-next'

if (-not (Test-Path -LiteralPath $Archive)) {
    throw "Source archive not found: $Archive"
}
if (Test-Path -LiteralPath $next) {
    & cmd.exe /d /c "rd /s /q \\?\$next"
}
New-Item -ItemType Directory -Path $next -Force | Out-Null
& tar.exe -xzf $Archive -C $next
if ($LASTEXITCODE -ne 0) {
    throw "Source extraction failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath (Join-Path $next 'Cargo.toml'))) {
    throw 'Extracted source has no Cargo.toml'
}
if (Test-Path -LiteralPath $destination) {
    & cmd.exe /d /c "rd /s /q \\?\$destination"
}
if (Test-Path -LiteralPath $destination) {
    throw "Could not replace CI source tree: $destination"
}
Move-Item -LiteralPath $next -Destination $destination
Remove-Item -LiteralPath $Archive -Force
$count = @(Get-ChildItem -LiteralPath $destination -File -Recurse).Count
Write-Output "Source synchronized: $count files"
