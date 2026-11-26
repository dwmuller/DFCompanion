<#
.SYNOPSIS
Resolves the path to dfhack-run.exe.

.DESCRIPTION
Reads DFHACK_INSTALL_DIR from the workspace's CMake cache and returns the
installed executable path when it exists. Otherwise, returns dfhack-run.exe
so PowerShell can resolve it from PATH.

.EXAMPLE
Resolve-DfHackExe
#>
function Resolve-DfHackExe {
    # Resolve relative to the workspace containing this tools directory.
    $workspaceRoot = Split-Path -Parent $PSScriptRoot
    $cacheCandidates = @(
        (Join-Path $workspaceRoot "build\CMakeCache.txt"),
        (Join-Path $workspaceRoot "build\CmakeCache.txt")
    )

    foreach ($cacheFile in $cacheCandidates) {
        if (Test-Path -LiteralPath $cacheFile) {
            # Match: DFHACK_INSTALL_DIR:PATH=E:/SteamLibrary/steamapps/common/DFHack
            $match = Select-String -LiteralPath $cacheFile -Pattern '^DFHACK_INSTALL_DIR:[^=]+=(.+)$' | Select-Object -First 1
            if ($match) {
                $installDir = $match.Matches[0].Groups[1].Value.Trim()
                $exePath = Join-Path $installDir "hack\dfhack-run.exe"
                if (Test-Path -LiteralPath $exePath) {
                    return $exePath
                }
            }
        }
    }

    # Fallback to PATH if CMakeCache.txt doesn't exist yet.
    return "dfhack-run.exe"
}

Export-ModuleMember -Function Resolve-DfHackExe
