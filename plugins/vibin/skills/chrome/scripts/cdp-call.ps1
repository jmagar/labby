# Generic CDP method invoker. Opens a WebSocket to a tab (or browser endpoint),
# sends one JSON-RPC call, loops past unsolicited events until it gets id:1, prints
# the matching response to stdout. Params can come from -Params or stdin.
#
# Usage:
#   cdp-call.ps1 -Pattern github -Method Runtime.evaluate -Params '{"expression":"document.title"}'
#   cdp-call.ps1 -Browser -Method Target.getTargets
#   '{"expression":"window.location.href","returnByValue":true}' | cdp-call.ps1 -Pattern github -Method Runtime.evaluate -ParamsStdin
param(
    [string]$Pattern = '',
    [int]$Port = 9222,
    [Parameter(Mandatory=$true)][string]$Method,
    [string]$Params = '{}',
    [switch]$ParamsStdin,
    [switch]$Browser
)
$ErrorActionPreference = 'Stop'
if ($ParamsStdin) { $Params = [Console]::In.ReadToEnd() }
if ($Browser) {
    $ver = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/json/version" -TimeoutSec 3
    $wsUrl = $ver.webSocketDebuggerUrl
    [Console]::Error.WriteLine("browser: $($ver.Browser)")
} else {
    $tabs = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/json" -TimeoutSec 3 | Where-Object { $_.type -eq 'page' }
    if ($Pattern) { $tabs = $tabs | Where-Object { $_.title -like "*$Pattern*" -or $_.url -like "*$Pattern*" } }
    if (-not $tabs) { [Console]::Error.WriteLine('NO_MATCH'); exit 1 }
    $tab = $tabs | Select-Object -First 1
    $wsUrl = $tab.webSocketDebuggerUrl
    [Console]::Error.WriteLine("tab: $($tab.title)")
}
$ws = New-Object System.Net.WebSockets.ClientWebSocket
$ct = New-Object System.Threading.CancellationTokenSource
$ws.ConnectAsync([Uri]$wsUrl, $ct.Token).Wait()
$payload = '{"id":1,"method":"' + $Method + '","params":' + $Params + '}'
$bytes = [Text.Encoding]::UTF8.GetBytes($payload)
$seg = New-Object System.ArraySegment[byte] -ArgumentList (,$bytes)
$ws.SendAsync($seg, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct.Token).Wait()
$buf = New-Object byte[] 65536
# Loop past any auto-emitted events (which arrive without an `id` field) until we see id:1.
while ($true) {
    $ms = New-Object System.IO.MemoryStream
    do {
        $seg2 = New-Object System.ArraySegment[byte] -ArgumentList (,$buf)
        $r = $ws.ReceiveAsync($seg2, $ct.Token).Result
        $ms.Write($buf, 0, $r.Count)
    } while (-not $r.EndOfMessage)
    $text = [Text.Encoding]::UTF8.GetString($ms.ToArray())
    try { $obj = $text | ConvertFrom-Json } catch { continue }
    if ($obj.id -eq 1) {
        [Console]::Out.Write($text)
        break
    }
    # else: it was an event — discard and keep reading
}
$ws.CloseAsync('NormalClosure', '', $ct.Token).Wait()
