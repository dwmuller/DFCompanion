<#
.SYNOPSIS
Creates the dfh alias for dfhack-run.exe in the current PowerShell session.

.DESCRIPTION
Imports the DFHack helper module, resolves dfhack-run.exe, and defines dfh as a
global alias so it remains available after this script finishes.

.EXAMPLE
.\tools\set-dfh-alias.ps1
#>
$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'dfhack.psm1') -ErrorAction Stop
Set-Alias -Name dfh -Value (Resolve-DfHackExe) -Scope Global
