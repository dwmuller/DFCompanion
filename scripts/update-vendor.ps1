# Installs the pinned browser libraries with pnpm and copies their minified
# builds into static/vendor, which is committed and compiled into the binary.
# The Rust build itself does not need pnpm.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    pnpm install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw 'pnpm install failed' }
    $vendor = Join-Path $root 'static\vendor'
    New-Item -ItemType Directory -Force $vendor | Out-Null
    Copy-Item node_modules\marked\lib\marked.umd.js (Join-Path $vendor 'marked.min.js') -Force
    Copy-Item node_modules\dompurify\dist\purify.min.js (Join-Path $vendor 'purify.min.js') -Force
} finally {
    Pop-Location
}
