# Tab screenshot via CDP. Connects to a tab matched by pattern (title/url substring),
# calls Page.captureScreenshot, writes PNG to OutDir, prints filename to stdout.
param(
    [string]$Pattern = '',
    [int]$Port = 9222,
    [string]$OutDir = 'C:\screens'
)
$ErrorActionPreference = 'Stop'
$tabs = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/json" -TimeoutSec 3 | Where-Object { $_.type -eq 'page' }
if ($Pattern) { $tabs = $tabs | Where-Object { $_.title -like "*$Pattern*" -or $_.url -like "*$Pattern*" } }
if (-not $tabs) { [Console]::Error.WriteLine('NO_MATCH'); exit 1 }
$tab = $tabs | Select-Object -First 1
[Console]::Error.WriteLine("tab: $($tab.title)")
$ws = New-Object System.Net.WebSockets.ClientWebSocket
$ct = New-Object System.Threading.CancellationTokenSource
$ws.ConnectAsync([Uri]$tab.webSocketDebuggerUrl, $ct.Token).Wait()
$payload = '{"id":1,"method":"Page.captureScreenshot","params":{"format":"png"}}'
$bytes = [Text.Encoding]::UTF8.GetBytes($payload)
$seg = New-Object System.ArraySegment[byte] -ArgumentList (,$bytes)
$ws.SendAsync($seg, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct.Token).Wait()
$buf = New-Object byte[] 65536
$obj = $null
while ($true) {
    $ms = New-Object System.IO.MemoryStream
    do {
        $seg2 = New-Object System.ArraySegment[byte] -ArgumentList (,$buf)
        $r = $ws.ReceiveAsync($seg2, $ct.Token).Result
        $ms.Write($buf, 0, $r.Count)
    } while (-not $r.EndOfMessage)
    $text = [Text.Encoding]::UTF8.GetString($ms.ToArray())
    try { $candidate = $text | ConvertFrom-Json } catch { continue }
    if ($candidate.id -eq 1) { $obj = $candidate; break }
}
if (-not $obj.result.data) { [Console]::Error.WriteLine('NO_DATA'); exit 1 }
$name = 'cdp-' + (Get-Date -f yyyyMMdd-HHmmss) + '.png'
[IO.File]::WriteAllBytes((Join-Path $OutDir $name), [Convert]::FromBase64String($obj.result.data))
[Console]::Out.Write($name)
$ws.CloseAsync('NormalClosure', '', $ct.Token).Wait()
