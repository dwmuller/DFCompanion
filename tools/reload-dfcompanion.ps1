<#
.SYNOPSIS
Reloads the DFCompanion plugin and Lua modules in a running DFHack instance.

.DESCRIPTION
Clears the cached DFCompanion Lua modules and reloads the rpc.dfcompanion
facade, reloads the dfcompanion plugin script, and enables the plugin.

.EXAMPLE
.\tools\reload-dfcompanion.ps1
#>
$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'dfhack.psm1') -ErrorAction Stop
$DfHackExe = Resolve-DfHackExe

function Invoke-DfHackCommand {
    param([Parameter(Mandatory)][string[]]$Arguments)

    & $DfHackExe @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "dfhack-run failed for '$($Arguments -join ' ')' with exit code $LASTEXITCODE."
    }
}

$moduleReload = @'
for name in pairs(package.loaded) do
    if name == 'rpc.dfcompanion' or name:match('^dfcompanion%.internal%.') then
        package.loaded[name] = nil
    end
end
require('rpc.dfcompanion')
'@

Invoke-DfHackCommand -Arguments @('lua', $moduleReload)
Invoke-DfHackCommand -Arguments @('lua', "reqscript('dfcompanion')")
