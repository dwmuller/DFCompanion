<#
.SYNOPSIS
Runs the live game tests against a running Dwarf Fortress with a fortress loaded.

.DESCRIPTION
Runs the ignored tests in tests/live_game_tests.rs. They share one game, so
they run on a single thread. Do not run this while installing the Lua files;
a concurrent install can make the tests fail spuriously.

.PARAMETER Filter
Optional test name filter, for example ai_tools_end_to_end.

.EXAMPLE
.\tools\run-live-tests.ps1

.EXAMPLE
.\tools\run-live-tests.ps1 -Filter ai_tools_end_to_end
#>
param([string]$Filter)

$ErrorActionPreference = 'Stop'
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $testArgs = @('--ignored', '--test-threads=1')
    if ($Filter) {
        $testArgs = @($Filter) + $testArgs
    }
    & cargo test --offline --test live_game_tests -- @testArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Live tests failed with exit code $LASTEXITCODE."
    }
}
finally {
    Pop-Location
}
