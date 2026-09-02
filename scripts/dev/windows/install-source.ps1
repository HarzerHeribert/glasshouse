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

# Stamp every extracted file to NOW. tar restores archive mtimes -- the local
# files' own mtimes -- and C:\ci\target persists across runs, so a source file
# older than an artifact built from a PREVIOUS tree is judged fresh and cargo
# runs stale test binaries against this tree. Two consecutive `test` runs
# compiled nothing (0.58s, 0.47s) and silently did not run a test the tree
# had just gained; one `touch` made the VM recompile and report 65 tests
# instead of 64 (GH-WINDOWS-TEST-BUILD, 2026-09-02). This is practice
# section 16's `touch`, applied at the sync boundary where it belongs.
$stamp = Get-Date
Get-ChildItem -LiteralPath $next -Recurse -File | ForEach-Object { $_.LastWriteTime = $stamp }
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
