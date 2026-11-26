<#
.SYNOPSIS
Installs the DFCompanion Lua files into DFHack without building the Rust binary.

.DESCRIPTION
Runs cmake/InstallDfhackScript.cmake directly, using DFHACK_INSTALL_DIR from
the workspace's CMake cache. This avoids the install-dfhack-script build
target, which fails while a running dfcompanion.exe locks its build output.
Then reloads the modules in the running DFHack instance unless -NoReload is
given.

.PARAMETER NoReload
Skip reloading the installed modules in DFHack.

.EXAMPLE
.\tools\install-dfcompanion-lua.ps1
#>
param([switch]$NoReload)

$ErrorActionPreference = 'Stop'

$workspaceRoot = Split-Path -Parent $PSScriptRoot
$cacheFile = Join-Path $workspaceRoot 'build\CMakeCache.txt'
if (-not (Test-Path -LiteralPath $cacheFile)) {
    throw "CMake cache not found at '$cacheFile'. Configure the build first."
}

$match = Select-String -LiteralPath $cacheFile -Pattern '^DFHACK_INSTALL_DIR:[^=]+=(.+)$' | Select-Object -First 1
if (-not $match) {
    throw "DFHACK_INSTALL_DIR is not set in '$cacheFile'."
}
$installDir = $match.Matches[0].Groups[1].Value.Trim()
$luaSourceDir = (Join-Path $workspaceRoot 'lua') -replace '\\', '/'

& cmake "-DDFHACK_INSTALL_DIR=$installDir" "-DLUA_SOURCE_DIR=$luaSourceDir" `
    -P (Join-Path $workspaceRoot 'cmake\InstallDfhackScript.cmake')
if ($LASTEXITCODE -ne 0) {
    throw "cmake install failed with exit code $LASTEXITCODE."
}

if (-not $NoReload) {
    & (Join-Path $PSScriptRoot 'reload-dfcompanion.ps1')
}
