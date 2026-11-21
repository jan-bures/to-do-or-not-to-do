<#
.SYNOPSIS
    Probes a todo-server deployment from the outside, without credentials.

.DESCRIPTION
    Run it from any machine, with no account or token. It:

    - lists every route that is reachable without authentication and what each one answers;
    - checks that a sample of everything else (the API, MCP, the admin pages, unknown paths and
      unusual methods) is refused with 401, 403, 404 or 405, or with a redirect to /login for a page
      load;
    - checks TLS (a valid certificate, TLS 1.2 or newer) and HSTS, unless -AllowHttp is given for a
      local http:// server;
    - checks that /health is an empty 200 that names no server or version;
    - checks the security headers on an HTML page and on an API error.

    It sends about 35 requests, paced -DelayMs apart, and no more than 10 of them to the paths the
    Cloudflare rate-limiting rule covers (/auth, /pair, /login, /setup). It changes nothing on the
    server: requests that could start a sign-in or a pairing are sent so that they are refused before
    they do anything (no Origin header, or an empty body).

    The base URL is a parameter; nothing about a deployment is built in.

    Exit code: 0 if every check passed, 1 if any failed.

.PARAMETER BaseUrl
    The server's public origin, for example https://todo.example.com.

.PARAMETER DelayMs
    Pause between requests, in milliseconds.

.PARAMETER AllowHttp
    Accept an http:// base URL (a local server). TLS and HSTS checks are skipped.

.EXAMPLE
    ./deploy/probe.ps1 -BaseUrl https://todo.example.com

.EXAMPLE
    ./deploy/probe.ps1 -BaseUrl http://127.0.0.1:47320 -AllowHttp
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BaseUrl,
    [int]$DelayMs = 250,
    [int]$TimeoutSec = 15,
    [switch]$AllowHttp
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Net.Http
try {
    # Windows PowerShell 5.1 runs on .NET Framework, which may default to older protocols.
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor `
        [Net.SecurityProtocolType]::Tls13
} catch {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
}

$base = [Uri]($BaseUrl.TrimEnd('/'))
if ($base.AbsolutePath -ne '/' -or $base.Query) {
    throw "BaseUrl must be an origin such as https://todo.example.com, without a path."
}
$https = $base.Scheme -eq 'https'
if (-not $https -and -not $AllowHttp) {
    throw "BaseUrl is not https. Pass -AllowHttp to probe a local http:// server."
}
$origin = $base.GetLeftPart([UriPartial]::Authority)

$handler = New-Object System.Net.Http.HttpClientHandler
$handler.AllowAutoRedirect = $false
$handler.UseCookies = $false
$client = New-Object System.Net.Http.HttpClient($handler)
$client.Timeout = [TimeSpan]::FromSeconds($TimeoutSec)
$client.DefaultRequestHeaders.UserAgent.ParseAdd('todo-probe/1')

$script:failures = 0
$script:warnings = 0
$script:sent = 0
$script:limited = 0

function Write-Result([string]$Verdict, [string]$What, [string]$Detail) {
    $color = switch ($Verdict) { 'PASS' { 'Green' } 'WARN' { 'Yellow' } 'SKIP' { 'DarkGray' } default { 'Red' } }
    if ($Verdict -eq 'FAIL') { $script:failures++ }
    if ($Verdict -eq 'WARN') { $script:warnings++ }
    Write-Host ('{0,-4}  {1,-44} {2}' -f $Verdict, $What, $Detail) -ForegroundColor $color
}

# Sends one request and returns status, headers (lower-case names) and body.
function Invoke-Probe {
    param(
        [string]$Method,
        [string]$Path,
        [hashtable]$Headers = @{},
        [string]$Body = $null
    )
    if ($script:sent -gt 0) { Start-Sleep -Milliseconds $DelayMs }
    $script:sent++
    if ($Path -match '^/(auth|pair|login|setup)') { $script:limited++ }
    $req = New-Object System.Net.Http.HttpRequestMessage((New-Object System.Net.Http.HttpMethod($Method)), "$origin$Path")
    foreach ($k in $Headers.Keys) { [void]$req.Headers.TryAddWithoutValidation($k, [string]$Headers[$k]) }
    # A [string] parameter turns $null into an empty string, so test for that.
    if (-not [string]::IsNullOrEmpty($Body)) {
        $req.Content = New-Object System.Net.Http.StringContent($Body, [Text.Encoding]::UTF8, 'application/json')
    }
    try {
        $res = $client.SendAsync($req, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
    } catch {
        return [pscustomobject]@{ Status = 0; Headers = @{}; Body = ''; Error = $_.Exception.GetBaseException().Message }
    }
    $h = @{}
    foreach ($pair in @($res.Headers) + @($res.Content.Headers)) {
        $h[$pair.Key.ToLowerInvariant()] = ($pair.Value -join ', ')
    }
    $text = ''
    $type = if ($h.ContainsKey('content-type')) { $h['content-type'] } else { '' }
    # Never wait on an open event stream: if one answered, that is the finding.
    if ($type -notlike 'text/event-stream*') {
        try {
            $text = $res.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        } catch {
            # A body cut off mid-way still leaves the status to judge.
            $text = ''
        }
    }
    $code = [int]$res.StatusCode
    $res.Dispose()
    return [pscustomobject]@{ Status = $code; Headers = $h; Body = $text; Error = $null }
}

function Get-Header($Res, [string]$Name) {
    if ($Res.Headers.ContainsKey($Name)) { return $Res.Headers[$Name] }
    return ''
}

function Format-Answer($Res) {
    if ($Res.Status -eq 0) { return "no answer: $($Res.Error)" }
    $type = (Get-Header $Res 'content-type') -replace ';.*$', ''
    $loc = Get-Header $Res 'location'
    $s = "$($Res.Status), $($Res.Body.Length) bytes"
    if ($type) { $s += ", $type" }
    if ($loc) { $s += ", -> $loc" }
    return $s
}

# ---- TLS -----------------------------------------------------------------------------------------

Write-Host "Probing $origin" -ForegroundColor Cyan
Write-Host ''
Write-Host 'Transport' -ForegroundColor Cyan
if ($https) {
    $tcp = New-Object System.Net.Sockets.TcpClient
    try {
        $tcp.Connect($base.Host, $base.Port)
        $ssl = New-Object System.Net.Security.SslStream($tcp.GetStream(), $false)
        # Default validation: an untrusted, expired or mismatched certificate throws.
        $ssl.AuthenticateAsClient($base.Host)
        $proto = [string]$ssl.SslProtocol
        $cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2($ssl.RemoteCertificate)
        $days = [int]($cert.NotAfter - (Get-Date)).TotalDays
        if ($proto -in @('Tls12', 'Tls13')) {
            Write-Result 'PASS' 'TLS version' $proto
        } else {
            Write-Result 'FAIL' 'TLS version' "$proto (want TLS 1.2 or newer)"
        }
        if ($days -ge 14) {
            Write-Result 'PASS' 'certificate' "valid, expires in $days days"
        } else {
            Write-Result 'WARN' 'certificate' "valid, but expires in $days days"
        }
        $ssl.Dispose()
    } catch {
        Write-Result 'FAIL' 'TLS handshake' $_.Exception.GetBaseException().Message
    } finally {
        $tcp.Dispose()
    }
    # Plain HTTP should redirect to HTTPS or not answer at all.
    $plain = "http://$($base.Host)/health"
    try {
        $script:sent++
        $r = $client.GetAsync($plain).GetAwaiter().GetResult()
        $loc = if ($r.Headers.Location) { [string]$r.Headers.Location } else { '' }
        $code = [int]$r.StatusCode
        $r.Dispose()
        if ($code -ge 300 -and $code -lt 400 -and $loc -like 'https://*') {
            Write-Result 'PASS' 'plain HTTP' "redirects to HTTPS ($code)"
        } else {
            Write-Result 'WARN' 'plain HTTP' "answers $code without redirecting (turn on Always Use HTTPS)"
        }
    } catch {
        Write-Result 'PASS' 'plain HTTP' 'not served'
    }
} else {
    Write-Result 'SKIP' 'TLS' 'http:// base URL (-AllowHttp)'
}

# ---- public routes -------------------------------------------------------------------------------

Write-Host ''
Write-Host 'Reachable without authentication' -ForegroundColor Cyan

# Every route the server documents as public. Requests that could change something are sent so that
# the server refuses them first: web POSTs without an Origin header, API POSTs with an empty body.
$public = @(
    @{ M = 'GET'; P = '/health' },
    @{ M = 'GET'; P = '/login' },
    @{ M = 'GET'; P = '/setup' },
    @{ M = 'GET'; P = '/pair' },
    @{ M = 'POST'; P = '/auth/login/start'; Note = 'no Origin: refused before a ceremony starts' },
    @{ M = 'POST'; P = '/auth/login/finish'; Note = 'no Origin' },
    @{ M = 'POST'; P = '/auth/setup/start'; Note = 'no Origin' },
    @{ M = 'POST'; P = '/auth/setup/finish'; Note = 'no Origin' },
    @{ M = 'POST'; P = '/api/v1/pair/start'; B = '{}'; Note = 'empty body: no pairing starts' },
    @{ M = 'POST'; P = '/api/v1/pair/poll'; B = '{}'; Note = 'empty body' },
    @{ M = 'GET'; P = '/assets/app.css' },
    @{ M = 'GET'; P = '/s/AAAAAAAAAAAAAAAAAAAAAA'; Note = 'a made-up share token' }
)
$answers = @{}
foreach ($r in $public) {
    $body = if ($r.ContainsKey('B')) { $r.B } else { $null }
    $res = Invoke-Probe -Method $r.M -Path $r.P -Body $body
    $answers["$($r.M) $($r.P)"] = $res
    $note = if ($r.ContainsKey('Note')) { "  ($($r.Note))" } else { '' }
    $ok = $res.Status -ne 0 -and $res.Status -lt 500 -and $res.Status -ne 429
    Write-Result $(if ($ok) { 'PASS' } else { 'FAIL' }) "$($r.M) $($r.P)" ((Format-Answer $res) + $note)
}

# ---- everything else -----------------------------------------------------------------------------

Write-Host ''
Write-Host 'Refused without authentication' -ForegroundColor Cyan

$uuid = '00000000-0000-4000-8000-000000000000'
$rpc = '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
$protected = @(
    @{ M = 'GET'; P = '/api/v1/lists' },
    @{ M = 'GET'; P = '/api/v1/lists'; H = @{ Authorization = 'Bearer tda_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA' }; Note = 'made-up token' },
    @{ M = 'GET'; P = "/api/v1/tasks/$uuid" },
    @{ M = 'GET'; P = '/api/v1/tasks?query=' },
    @{ M = 'GET'; P = '/api/v1/events'; H = @{ Accept = 'text/event-stream' } },
    @{ M = 'GET'; P = '/api/v1/openapi.json' },
    @{ M = 'POST'; P = '/api/v1/commands'; B = '{}' },
    @{ M = 'POST'; P = '/api/v1/events/1/undo'; B = '{}' },
    @{ M = 'POST'; P = '/mcp'; B = $rpc; H = @{ Accept = 'application/json, text/event-stream' } },
    @{ M = 'GET'; P = '/mcp' },
    @{ M = 'GET'; P = '/' },
    @{ M = 'GET'; P = '/admin' },
    @{ M = 'GET'; P = '/admin/agents' },
    @{ M = 'POST'; P = '/admin/agents'; B = '{}' },
    @{ M = 'POST'; P = '/auth/step-up/start'; B = '{}' },
    @{ M = 'POST'; P = '/pair/approve'; B = '{}' },
    @{ M = 'GET'; P = '/api/v1/nope' },
    @{ M = 'GET'; P = '/nope' },
    @{ M = 'GET'; P = '/.env' },
    @{ M = 'PUT'; P = '/health'; B = '{}' },
    @{ M = 'DELETE'; P = '/api/v1/lists' },
    @{ M = 'OPTIONS'; P = '/api/v1/lists' },
    @{ M = 'TRACE'; P = '/' },
    @{ M = 'BREW'; P = '/admin' }
)
foreach ($r in $protected) {
    $body = if ($r.ContainsKey('B')) { $r.B } else { $null }
    $hdrs = if ($r.ContainsKey('H')) { $r.H } else { @{} }
    $res = Invoke-Probe -Method $r.M -Path $r.P -Headers $hdrs -Body $body
    $answers["$($r.M) $($r.P)"] = $res
    $loc = Get-Header $res 'location'
    $ok = $res.Status -in @(401, 403, 404, 405)
    # A page load is sent to sign-in instead.
    if ($res.Status -eq 303 -and $loc -like '/login?next=*') { $ok = $true }
    # The edge may refuse unusual methods itself, before they reach the server.
    if ($r.M -in @('TRACE', 'BREW') -and $res.Status -in @(400, 501)) { $ok = $true }
    $note = if ($r.ContainsKey('Note')) { "  ($($r.Note))" } else { '' }
    Write-Result $(if ($ok) { 'PASS' } else { 'FAIL' }) "$($r.M) $($r.P)" ((Format-Answer $res) + $note)
}

# ---- what the answers carry ----------------------------------------------------------------------

Write-Host ''
Write-Host 'Headers and contents' -ForegroundColor Cyan

$health = $answers['GET /health']
if ($health.Status -eq 200 -and $health.Body.Length -eq 0) {
    Write-Result 'PASS' '/health' 'empty 200'
} else {
    Write-Result 'FAIL' '/health' "want an empty 200, got $(Format-Answer $health)"
}
$server = Get-Header $health 'server'
$leaky = @($health.Headers.Keys | Where-Object { $_ -in @('x-powered-by', 'x-version', 'x-app-version') -or $_ -like '*version*' })
if ($leaky.Count -eq 0 -and ($server -eq '' -or $server -eq 'cloudflare')) {
    Write-Result 'PASS' '/health reveals nothing' $(if ($server) { "Server: $server (the edge)" } else { 'no Server header' })
} else {
    Write-Result 'FAIL' '/health reveals nothing' "Server: '$server', headers: $($leaky -join ', ')"
}

function Test-Hsts([string]$What, $Res) {
    if (-not $https) { Write-Result 'SKIP' "HSTS on $What" 'http:// base URL'; return }
    $v = Get-Header $Res 'strict-transport-security'
    $age = 0
    if ($v -match 'max-age=(\d+)') { $age = [int64]$Matches[1] }
    if ($age -ge 15552000) {
        Write-Result 'PASS' "HSTS on $What" $v
    } else {
        Write-Result 'FAIL' "HSTS on $What" "want max-age of at least 180 days, got '$v'"
    }
}
Test-Hsts '/health' $health

$login = $answers['GET /login']
$csp = Get-Header $login 'content-security-policy'
$checks = @(
    @('CSP frame-ancestors', ($csp -match "frame-ancestors 'none'"), $csp),
    @('CSP default-src', ($csp -match "default-src '(self|none)'"), $csp),
    @('X-Frame-Options', ((Get-Header $login 'x-frame-options') -eq 'DENY'), (Get-Header $login 'x-frame-options')),
    @('X-Content-Type-Options', ((Get-Header $login 'x-content-type-options') -eq 'nosniff'), (Get-Header $login 'x-content-type-options')),
    @('Referrer-Policy', ((Get-Header $login 'referrer-policy') -in @('no-referrer', 'same-origin', 'strict-origin')), (Get-Header $login 'referrer-policy'))
)
foreach ($c in $checks) {
    Write-Result $(if ($c[1]) { 'PASS' } else { 'FAIL' }) "/login $($c[0])" $c[2]
}
Test-Hsts '/login' $login

$api = $answers['GET /api/v1/lists']
$www = Get-Header $api 'www-authenticate'
Write-Result $(if ($api.Status -eq 401 -and $www -eq 'Bearer') { 'PASS' } else { 'FAIL' }) 'API 401' "WWW-Authenticate: $www"
Write-Result $(if ((Get-Header $api 'x-content-type-options') -eq 'nosniff') { 'PASS' } else { 'FAIL' }) 'API nosniff' (Get-Header $api 'x-content-type-options')
Test-Hsts 'API errors' $api

$css = $answers['GET /assets/app.css']
$cssOk = (Get-Header $css 'content-type') -like 'text/css*' -and (Get-Header $css 'x-content-type-options') -eq 'nosniff'
Write-Result $(if ($cssOk) { 'PASS' } else { 'FAIL' }) 'static asset typed' "$(Get-Header $css 'content-type'), $(Get-Header $css 'x-content-type-options')"

$client.Dispose()

Write-Host ''
$summary = "$($script:sent) requests ($($script:limited) to rate-limited paths), $($script:failures) failed, $($script:warnings) warnings"
if ($script:failures -gt 0) {
    Write-Host $summary -ForegroundColor Red
    exit 1
}
Write-Host $summary -ForegroundColor Green
exit 0
