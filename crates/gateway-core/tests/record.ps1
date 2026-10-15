# 录制工具（票 19 §2、票 29 第 1 条）：跑真实上游一次，把**原始 SSE 字节**连同 meta.json 写进 fixtures/。
# 不进网关的生产路径；重录是人工动作，绝不在回归里自动发生。
#
# 票 41 搬到产品侧 `crates/gateway-core/tests/`：默认落点就是这个目录下的 `fixtures/`，
# **绝不**写回冻结原型 `.scratch/model-gateway/prototype/fixtures/`。
#
# 例：
#   pwsh -File .\record.ps1 -Scenario tools -Wire openai -Model deepseek-v4.1-flash `
#     -Url "$env:OPENCODE_BASE_URL/chat/completions" -KeyVar OPENCODE_API_KEY
[CmdletBinding()]
param(
  [Parameter(Mandatory)][string]$Scenario,
  [Parameter(Mandatory)][ValidateSet('anthropic', 'openai')][string]$Wire,
  [Parameter(Mandatory)][string]$Model,
  [Parameter(Mandatory)][string]$Url,
  [Parameter(Mandatory)][string]$KeyVar,          # prototype-upstream.env 里的键名
  [ValidateSet('bearer', 'x-api-key')][string]$Auth = 'bearer',
  [string]$BodyFile,                              # 不给就用标准纯文本请求
  [string]$ClientModel,                           # 客户端打过来的虚拟模型名（默认就用 $Model）
  [string[]]$Quirk = @(),
  # 网关会替上游注入的头（quirk 执法点在网关侧，录制要照着补，否则上游按「缺头」拒：
  # opencode Go 的 Chat 面就要求 x-opencode-session）。多个头用分号隔开，例如：
  #   -Header "x-opencode-session: gw-record-0001; user-agent: claude-gateway-prototype/0.0.1"
  [string]$Header = '',
  [string]$Out = "$PSScriptRoot\fixtures",
  [switch]$Force,
  [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
$utf8 = New-Object System.Text.UTF8Encoding($false)

# 1) 凭据：只认 prototype-upstream.env（已 gitignore），不接受命令行明文，免得写进历史。
#    产品侧 tests/ 往上三层是仓库根。
$envFile = Join-Path $PSScriptRoot '..\..\..\.scratch\model-gateway\prototype-upstream.env'
$key = $null
if (Test-Path $envFile) {
  foreach ($line in Get-Content $envFile) {
    $t = $line.Trim()
    if ($t -and -not $t.StartsWith('#') -and $t.StartsWith("$KeyVar=")) {
      $key = $t.Substring($KeyVar.Length + 1).Trim()
    }
  }
}
if (-not $key) { throw "读不到 $KeyVar（$envFile）" }

# 2) 请求体：默认形状与 mock fixture 一致，方便两种 fixture 用同一套断言。
$clientBody = if ($BodyFile) { Get-Content $BodyFile -Raw | ConvertFrom-Json }
else {
  [ordered]@{
    model      = $(if ($ClientModel) { $ClientModel } else { $Model })
    max_tokens = 256
    stream     = $true
    messages   = @([ordered]@{ role = 'user'; content = '你好' })
  }
}
$wireBody = $clientBody | ConvertTo-Json -Depth 12 -Compress | ConvertFrom-Json
$wireBody.model = $Model

$authHeader = if ($Auth -eq 'bearer') { @("authorization: Bearer $key") } else { @("x-api-key: $key") }
$injected = @($Header -split ';' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$headers = @("content-type: application/json") + $authHeader + $injected

$destDir = Join-Path $Out $Scenario
$ssePath = Join-Path $destDir "$Wire.sse"
$metaPath = Join-Path $destDir "$Wire.meta.json"
if ((Test-Path $ssePath) -and -not $Force) { throw "$ssePath 已存在；要重录请加 -Force" }

$sentBody = Join-Path $env:TEMP "record-body-$Scenario-$Wire.json"
[IO.File]::WriteAllText($sentBody, ($wireBody | ConvertTo-Json -Depth 12 -Compress), $utf8)

if ($DryRun) {
  "url      $Url"
  "auth     $Auth / $KeyVar（值不打印）"
  "body     $sentBody"
  "-> $ssePath"
  "-> $metaPath"
  exit 0
}

# 3) 真跑一次，字节原样落盘（curl -o 不经过 PowerShell 的编码转换）。
New-Item -ItemType Directory -Force -Path $destDir | Out-Null
$hdrFile = Join-Path $env:TEMP "record-hdr-$Scenario-$Wire.txt"
$curlArgs = @('-s', '-N', '--max-time', '120', '-D', $hdrFile, '-o', $ssePath, '-X', 'POST', $Url,
  '--data-binary', "@$sentBody")
foreach ($h in $headers) { $curlArgs += @('-H', $h) }
& curl.exe @curlArgs
if ($LASTEXITCODE -ne 0) { throw "curl 失败（exit $LASTEXITCODE）" }

$statusLine = (Get-Content $hdrFile -TotalCount 1) -replace '^HTTP/\S+\s+', ''
$status = [int]($statusLine.Split(' ')[0])
$bytes = [IO.File]::ReadAllBytes($ssePath)
$text = [Text.Encoding]::UTF8.GetString($bytes)
# Anthropic wire 有 `event:` 行；OpenAI wire 只有 `data:` 行，终态是 `data: [DONE]`。
$events = @([regex]::Matches($text, '(?m)^event: (\S+)$'))
$lastEvent = if ($events.Count -gt 0) {
  $events[-1].Groups[1].Value
}
elseif ($text -match '(?m)^data: \[DONE\]') { 'DONE' }
else { '' }
if ($status -ne 200) {
  Write-Warning "上游回了 $status；字节仍已落盘，但这份不该当黄金用"
}

# 4) meta.json：字段与票 19 §2 对齐；chunks 留空 = 内建切法（frame/half/byte/whole/crlf）。
$meta = [ordered]@{
  scenario      = $Scenario
  outbound_wire = $Wire
  upstream      = ([uri]$Url).Host
  model         = $Model
  origin        = 'recorded'
  recorded_at   = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
  request       = $clientBody
  request_sent  = [ordered]@{
    url     = $Url
    headers = @($headers | Where-Object { $_ -notmatch '^(authorization|x-api-key):' }) + '<凭据头已省略>'
    body    = $wireBody
  }
  expect        = [ordered]@{ status = $status; last_event = $lastEvent }
  quirks        = @($Quirk)
  masks         = @('message.id')
  chunks        = @()
  changed_why   = ''
}
New-Item -ItemType Directory -Force -Path $destDir | Out-Null
[IO.File]::WriteAllText($metaPath, ($meta | ConvertTo-Json -Depth 12), $utf8)
"[ok] $status $($bytes.Length) 字节 -> $ssePath"
"[ok] meta -> $metaPath（末帧 $lastEvent）"
