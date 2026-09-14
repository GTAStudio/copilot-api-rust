param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2
Add-Type -AssemblyName System.Net.Http

if (-not ('CopilotReleasePayloadVerifier' -as [type])) {
    Add-Type -TypeDefinition @'
public static class CopilotReleasePayloadVerifier
{
    public static bool Contains(byte[] image, byte[] payload)
    {
        if (payload.Length == 0 || payload.Length > image.Length) return false;
        for (int start = 0; start <= image.Length - payload.Length; start++)
        {
            if (image[start] != payload[0]) continue;
            int index = 1;
            while (index < payload.Length && image[start + index] == payload[index]) index++;
            if (index == payload.Length) return true;
        }
        return false;
    }
}
'@
}

$serverPath = Join-Path $Root 'rust-server/target/release/copilot-api-server.exe'
$guiPath = Join-Path $Root 'gui-slint/target/release/copilot-api-gui.exe'
$embeddedFiles = @(Get-ChildItem -Path (Join-Path $Root 'gui-slint/target/release/build/copilot-api-gui-*/out/server_embedded.gz') -File)
$serverHash = (Get-FileHash -Algorithm SHA256 -Path $serverPath).Hash.ToLowerInvariant()
$guiBytes = [System.IO.File]::ReadAllBytes($guiPath)
$verifiedPayload = $null
foreach ($candidate in $embeddedFiles) {
    $inputStream = [System.IO.File]::OpenRead($candidate.FullName)
    $decoder = New-Object System.IO.Compression.GZipStream($inputStream, [System.IO.Compression.CompressionMode]::Decompress)
    $hasher = [System.Security.Cryptography.SHA256]::Create()
    try {
        $embeddedHash = [BitConverter]::ToString($hasher.ComputeHash($decoder)).Replace('-', '').ToLowerInvariant()
    } finally {
        $hasher.Dispose()
        $decoder.Dispose()
        $inputStream.Dispose()
    }
    if ($embeddedHash -eq $serverHash -and [CopilotReleasePayloadVerifier]::Contains($guiBytes, [System.IO.File]::ReadAllBytes($candidate.FullName))) {
        $verifiedPayload = $candidate
        break
    }
}
if ($null -eq $verifiedPayload) {
    throw 'No cached payload matches both the release server hash and the GUI executable contents.'
}

function Start-IsolatedServer([string]$ListenHost, [string]$ApiKey) {
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = $serverPath
    $start.Arguments = "start --host $ListenHost --port 0"
    $start.WorkingDirectory = $Root
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($name in @('COPILOT_API_KEY', 'COPILOT_GITHUB_TOKEN', 'ANTHROPIC_API_KEY', 'OPENAI_API_KEY', 'AZURE_OPENAI_KEY')) {
        $start.EnvironmentVariables.Remove($name)
    }
    $start.EnvironmentVariables['COPILOT_PROVIDER'] = 'anthropic'
    $start.EnvironmentVariables['COPILOT_EDITOR_VERSION'] = '1.0.0'
    $start.EnvironmentVariables['COPILOT_HOOKS_ENABLED'] = '0'
    $start.EnvironmentVariables['COPILOT_DISABLE_PROXY'] = '1'
    $start.EnvironmentVariables['COPILOT_MANUAL_APPROVE'] = '0'
    $start.EnvironmentVariables['RUST_LOG'] = 'info'
    if ($ApiKey) { $start.EnvironmentVariables['COPILOT_API_KEY'] = $ApiKey }
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $start
    if (-not $process.Start()) { throw 'Failed to start isolated server.' }
    return $process
}

function Stop-OwnedProcess([System.Diagnostics.Process]$Process, [System.Threading.Tasks.Task[]]$Readers = @()) {
    if ($null -eq $Process) { return }
    try {
        if (-not $Process.HasExited) { $Process.Kill() }
        if (-not $Process.WaitForExit(10000)) { throw 'Owned test process did not exit.' }
        foreach ($reader in $Readers) {
            if ($null -ne $reader -and -not $reader.Wait(10000)) {
                throw 'Owned test process output was not drained.'
            }
        }
    } finally {
        $Process.Dispose()
    }
}

$key = [Guid]::NewGuid().ToString('N')
$process = $null
$client = $null
$handler = $null
$outputTask = $null
$errorTask = $null
$checks = New-Object System.Collections.Generic.List[object]
try {
    $process = Start-IsolatedServer '127.0.0.1' $key
    $outputTask = $process.StandardOutput.ReadToEndAsync()
    $startupLine = $process.StandardError.ReadLineAsync()
    if (-not $startupLine.Wait(30000) -or $startupLine.Result -notmatch 'listening on') {
        throw 'Isolated server did not report readiness.'
    }
    $errorTask = $process.StandardError.ReadToEndAsync()
    $listeners = @(Get-NetTCPConnection -State Listen -OwningProcess $process.Id)
    if ($listeners.Count -ne 1 -or $listeners[0].LocalAddress -ne '127.0.0.1') {
        throw 'Unexpected isolated listener.'
    }
    $baseUrl = "http://127.0.0.1:$($listeners[0].LocalPort)"
    $handler = New-Object System.Net.Http.HttpClientHandler
    $handler.UseProxy = $false
    $handler.AllowAutoRedirect = $false
    $client = New-Object System.Net.Http.HttpClient($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(10)
    $cases = @(
        @{ Name = 'unauthenticated'; Method = 'GET'; Path = '/'; Headers = @{}; Body = $null; Expected = 401 },
        @{ Name = 'bearer health'; Method = 'GET'; Path = '/'; Headers = @{ Authorization = "Bearer $key" }; Body = $null; Expected = 200 },
        @{ Name = 'x-api-key health'; Method = 'GET'; Path = '/'; Headers = @{ 'x-api-key' = $key }; Body = $null; Expected = 200 },
        @{ Name = 'cross-origin'; Method = 'GET'; Path = '/'; Headers = @{ 'x-api-key' = $key; Origin = 'https://untrusted.example' }; Body = $null; Expected = 403 },
        @{ Name = 'raw copilot token removed'; Method = 'GET'; Path = '/token'; Headers = @{ 'x-api-key' = $key }; Body = $null; Expected = 404 },
        @{ Name = 'raw github token removed'; Method = 'GET'; Path = '/auth/token'; Headers = @{ 'x-api-key' = $key }; Body = $null; Expected = 404 },
        @{ Name = 'invalid count body'; Method = 'POST'; Path = '/v1/messages/count_tokens'; Headers = @{ 'x-api-key' = $key }; Body = '{"model":"claude-sonnet-4-6","messages":[]}'; Expected = 400 },
        @{ Name = 'malformed JSON'; Method = 'POST'; Path = '/v1/messages'; Headers = @{ 'x-api-key' = $key }; Body = '{broken'; Expected = 400 }
    )
    foreach ($case in $cases) {
        $method = New-Object System.Net.Http.HttpMethod($case.Method)
        $request = New-Object System.Net.Http.HttpRequestMessage($method, "$baseUrl$($case.Path)")
        $response = $null
        try {
            foreach ($name in $case.Headers.Keys) { [void]$request.Headers.TryAddWithoutValidation($name, [string]$case.Headers[$name]) }
            if ($null -ne $case.Body) {
                $request.Content = New-Object System.Net.Http.StringContent($case.Body, [System.Text.Encoding]::UTF8, 'application/json')
            }
            $response = $client.SendAsync($request).GetAwaiter().GetResult()
            $status = [int]$response.StatusCode
            if ($status -ne $case.Expected) { throw "$($case.Name): expected $($case.Expected), received $status." }
            if ($response.Headers.Contains('Access-Control-Allow-Origin')) { throw 'Unexpected browser CORS permission.' }
            $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
            if ($body.Contains($key)) { throw 'Local key exposed in response.' }
            if ($status -eq 200 -and $body -ne 'Server running') { throw 'Unexpected health response.' }
            $checks.Add([ordered]@{ name = $case.Name; status = $status; passed = $true })
        } finally {
            if ($null -ne $response) { $response.Dispose() }
            $request.Dispose()
        }
    }
} finally {
    if ($null -ne $client) { $client.Dispose() }
    if ($null -ne $handler) { $handler.Dispose() }
    Stop-OwnedProcess $process @($outputTask, $errorTask)
}

$remote = $null
$remoteOutput = $null
$remoteError = $null
try {
    $remote = Start-IsolatedServer '0.0.0.0' ''
    $remoteOutput = $remote.StandardOutput.ReadToEndAsync()
    $remoteError = $remote.StandardError.ReadToEndAsync()
    if (-not $remote.WaitForExit(30000)) { throw 'Unauthenticated remote binding was not refused promptly.' }
    if ($remote.ExitCode -ne 2 -or $remoteError.GetAwaiter().GetResult() -notmatch 'COPILOT_API_KEY is required') {
        throw 'Expected non-loopback authentication configuration rejection.'
    }
    $checks.Add([ordered]@{ name = 'keyless remote binding rejected'; exitCode = $remote.ExitCode; passed = $true })
} finally {
    Stop-OwnedProcess $remote @($remoteOutput, $remoteError)
}

$result = [ordered]@{
    passed = $true
    checkedAt = [DateTime]::UtcNow.ToString('o')
    embeddedMatchesServer = $true
    embeddedPayloadPresentInGui = $true
    verifiedPayload = $verifiedPayload.FullName
    cachedPayloadCandidates = $embeddedFiles.Count
    server = [ordered]@{ bytes = (Get-Item $serverPath).Length; sha256 = $serverHash }
    gui = [ordered]@{ bytes = (Get-Item $guiPath).Length; sha256 = (Get-FileHash -Algorithm SHA256 -Path $guiPath).Hash.ToLowerInvariant() }
    checks = $checks.ToArray()
    liveInferenceTested = $false
    visibleGuiTested = $false
}
$result | ConvertTo-Json -Depth 5 | Out-File -Encoding utf8 (Join-Path $Root 'rust-server/target/release-verification.json')
$result | ConvertTo-Json -Depth 5