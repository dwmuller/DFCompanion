# tools/dfhack-mcp.ps1
$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'dfhack.psm1') -ErrorAction Stop

$DfHackExe = Resolve-DfHackExe

# Ensure UTF-8 without BOM on stdout so VS Code's JSON-RPC parser never chokes
[Console]::InputEncoding  = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

function Send-McpResponse {
    param([Parameter(Mandatory)]$Payload)
    $json = $Payload | ConvertTo-Json -Depth 10 -Compress
    [Console]::Out.WriteLine($json)
    [Console]::Out.Flush()
}

[Console]::Error.WriteLine("DFHack MCP server starting...")   

while ($null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }

    try {
        $req = $line | ConvertFrom-Json
    } catch {
        continue
    }

    # Notifications (no 'id' property) do not expect a JSON-RPC response
    if ($null -eq $req.id) { continue }

    switch ($req.method) {
        "initialize" {
            Send-McpResponse @{
                jsonrpc = "2.0"
                id      = $req.id
                result  = @{
                    protocolVersion = "2024-11-05"
                    capabilities    = @{ tools = @{} }
                    serverInfo      = @{ name = "dfhack-run"; version = "1.0.0" }
                }
            }
        }

        "tools/list" {
            Send-McpResponse @{
                jsonrpc = "2.0"
                id      = $req.id
                result  = @{
                    tools = @(
                        @{
                            name        = "dfhack_run"
                            description = "ALWAYS use this to execute a DFHack command in a running Dwarf Fortress instance instead of running dfhack-run.exe manually."
                            inputSchema = @{
                                type       = "object"
                                properties = @{
                                    command = @{
                                        type        = "string"
                                        description = "The DFHack command to run (e.g., 'ls', 'prospect all', 'lua print(df.global.world.frame_counter)')."
                                    }
                                    args = @{
                                        type        = "array"
                                        items       = @{ type = "string" }
                                        description = "Optional separate arguments if not included directly in the command string."
                                    }
                                }
                                required = @("command")
                            }
                        }
                    )
                }
            }
        }

        "tools/call" {
            # Re-resolve on call in case CMake was configured after the MCP server started
            $DfHackExe = Resolve-DfHackExe

            $cmdName = $req.params.arguments.command
            $extraArgs = @($req.params.arguments.args | Where-Object { $null -ne $_ })

            # Split command string into tokens if extraArgs wasn't explicitly used, preserving quoted substrings
            $tokens = [System.Management.Automation.PSParser]::Tokenize($cmdName, [ref]$null) |
                      Where-Object { $_.Type -in 'Command', 'CommandArgument', 'String' } |
                      ForEach-Object { $_.Content }
            if (-not $tokens) { $tokens = @($cmdName) }

            $allArgs = @($tokens) + $extraArgs

            try {
                $output = & $DfHackExe @allArgs 2>&1 | Out-String
                $isError = ($LASTEXITCODE -ne 0)
            } catch {
                $output = "Failed running '$DfHackExe': $($_.Exception.Message)"
                $isError = $true
            }

            if ([string]::IsNullOrWhiteSpace($output)) {
                $output = if ($isError) { "dfhack-run.exe failed with exit code $LASTEXITCODE." } else { "(Command succeeded with no output)" }
            }

            Send-McpResponse @{
                jsonrpc = "2.0"
                id      = $req.id
                result  = @{
                    content = @(
                        @{
                            type = "text"
                            text = $output.Trim()
                        }
                    )
                    isError = $isError
                }
            }
        }

        default {
            Send-McpResponse @{
                jsonrpc = "2.0"
                id      = $req.id
                error   = @{
                    code    = -32601
                    message = "Method not found: $($req.method)"
                }
            }
        }
    }
}
