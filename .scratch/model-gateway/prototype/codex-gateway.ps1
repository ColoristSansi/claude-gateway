# 把 Codex 接到本机网关的最小配置写入器（票 32）。
#
#   pwsh -File .\codex-gateway.ps1                  # 写配置 + 生成模型目录（首写自动备份）
#   pwsh -File .\codex-gateway.ps1 -Mode show       # 只看当前接管状态，不动文件
#   pwsh -File .\codex-gateway.ps1 -Mode restore    # 一键还原 = 摘键（默认，保留别人的改动）
#   pwsh -File .\codex-gateway.ps1 -Mode rollback   # 整份回滚（仅当文件没被外部改过）
#   pwsh -File .\codex-gateway.ps1 -Mode selftest   # 插入/摘除的自检，不碰任何真实文件
#
# 接管 = **逐行认领**，不是「哨兵之间的整段」：
#   - 顶层键 model_provider / model / model_catalog_json（只认第一个 [表] 之前的那几行）
#   - [model_providers.claudegateway] 这张表（到下一个 [表] 为止）
#   - 我们自己写的两对哨兵注释行
#
# 2026-10-15 实测教训：哨兵围出的「区域」会被别的写手捅穿。Codex 桌面应用启动时把它的
# `[windows] sandbox = "elevated"` 插进了我们 provider 哨兵之间 —— 按区域删就会连它一起删掉。
# 所以这里改成「只删自己认识的那几行」，插进来的任何东西（[desktop] / [plugins.*] /
# [mcp_servers.*] / [windows] …）一个字节都不动。这正是票 28「显式接管键清单」的由来。
[CmdletBinding()]
param(
  [ValidateSet('gateway', 'restore', 'rollback', 'show', 'selftest')] [string]$Mode = 'gateway',
  [string]$CodexHome = (Join-Path $env:USERPROFILE '.codex'),
  [string]$BaseUrl = 'http://127.0.0.1:8787',
  [string]$ProviderId = 'claudegateway',
  [string]$Model = 'claude-sonnet',
  # 额外的虚拟模型名（普通虚拟模型 / 负例）：目录默认只装四个家族槽位。
  [string[]]$IncludeModel = @(),
  [int]$ContextWindow = 262144
)
$ErrorActionPreference = 'Stop'

$configPath = Join-Path $CodexHome 'config.toml'
$catalogName = 'claudegateway-model-catalog.json'
$catalogPath = Join-Path $CodexHome $catalogName
$backupPath = Join-Path $CodexHome 'config.toml.bak-claudegateway'

$topBegin = '# >>> claudegateway managed (top-level) >>>'
$topEnd = '# <<< claudegateway managed (top-level) <<<'
$tblBegin = '# >>> claudegateway managed (provider) >>>'
$tblEnd = '# <<< claudegateway managed (provider) <<<'
$tableHeader = "[model_providers.$ProviderId]"
# 顶层接管键：写进 config.toml 时用不上 ProviderId，认领时也不看值（票 28 的冲突判定没做）。
$ownedTopKeys = '^(model_provider|model|model_catalog_json)\s*='

# ---------- 逐字节读写：原文件什么行尾、有没有 BOM，改完还是什么 ----------

function Read-Raw {
  if (Test-Path $configPath) { return [System.IO.File]::ReadAllText($configPath) }
  return ''
}

function Read-FileRaw([string]$path) {
  if (Test-Path $path) { return [System.IO.File]::ReadAllText($path) }
  return ''
}

function Split-Raw([string]$raw) {
  if ($raw.Length -eq 0) { return @() }
  $lines = [System.Text.RegularExpressions.Regex]::Split($raw, "`r`n|`n")
  if ($lines.Count -le 1) { return @() }
  # 末尾换行会在数组尾部留一个空串；它不是一行内容。
  if ($lines[-1] -eq '') { $lines = $lines[0..($lines.Count - 2)] }
  return $lines
}

function Newline-Of([string]$raw) {
  if ($raw -match "`r`n") { return "`r`n" }
  return "`n"
}

function Write-Raw([string]$text) {
  $enc = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($configPath, $text, $enc)
}

# ---------- 认领 / 插入 ----------

function Get-ManagedTopBlock {
  @(
    $topBegin,
    "model_provider = `"$ProviderId`"",
    "model = `"$Model`"",
    "model_catalog_json = `"$catalogName`"",
    $topEnd
  )
}

function Get-ManagedTableBlock {
  # 这一段刻意**不写解释性注释**：解释行会被插进来的表切开、摘键时变成孤儿（自检抓过）。
  # 决策写在脚本头与 README，配置文件里只留哨兵。
  @(
    $tblBegin,
    $tableHeader,
    "name = `"Claude Gateway`"",
    "base_url = `"$($BaseUrl.TrimEnd('/'))/v1`"",
    # 刻意不写 requires_openai_auth / env_key（票 24）：网关入站不鉴权，
    # 而 env_key 声明的变量必须真存在，否则 Codex 直接拒发。
    'wire_api = "responses"',
    $tblEnd
  )
}

function Remove-Owned([string[]]$lines) {
  $out = [System.Collections.Generic.List[string]]::new()
  $inOwnedTable = $false
  $tableSeen = $false
  foreach ($l in $lines) {
    $t = $l.Trim()
    if ($t -eq $topBegin -or $t -eq $topEnd -or $t -eq $tblBegin -or $t -eq $tblEnd) { continue }
    if ($t.StartsWith('[')) {
      $tableSeen = $true
      if ($t -eq $tableHeader) { $inOwnedTable = $true; continue }
      $inOwnedTable = $false
      $out.Add($l)
      continue
    }
    if ($inOwnedTable) { continue }
    if (-not $tableSeen -and $t -match $ownedTopKeys) { continue }
    $out.Add($l)
  }
  return $out
}

function Add-Managed([string[]]$lines) {
  $out = [System.Collections.Generic.List[string]]::new()
  $firstTable = -1
  for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i].Trim().StartsWith('[')) { $firstTable = $i; break }
  }
  $top = Get-ManagedTopBlock
  $tbl = Get-ManagedTableBlock
  if ($firstTable -lt 0) {
    foreach ($l in $lines) { $out.Add($l) }
    foreach ($l in $top) { $out.Add($l) }
  }
  else {
    # 顶层键必须排在第一个 [表] 之前，否则会被算进那张表里。
    for ($i = 0; $i -lt $firstTable; $i++) { $out.Add($lines[$i]) }
    foreach ($l in $top) { $out.Add($l) }
    for ($i = $firstTable; $i -lt $lines.Count; $i++) { $out.Add($lines[$i]) }
  }
  # 一个空行都不添：添了就得在摘键时把它认回来，而空行无法自证归属（幂等会破）。
  foreach ($l in $tbl) { $out.Add($l) }
  return $out
}

# ---------- 模型目录（票 31）----------

# 中性模板：抄 cc-switch 的 `src-tauri/src/resources/codex_native_responses_template.json`
# （MIT）。判据是**入站**是 Responses —— cc-switch 正是给「native /responses 网关」用这份，
# 它不带 freeform `apply_patch` / `web_search`，编辑走 `shell_type="shell_command"` 的
# JSON-schema `shell` 工具。票 31 原文写的是「模板按出站 wire 分」，本票按这份证据修正为
# 「按入站协议分」：出站是 anthropic 还是 openai-chat，跟 Codex 发不发自由格式工具无关。
# （网关仍能桥接 `type:"custom"` 工具，见 responses.rs；用经典模板时不至于崩。）
$neutralTemplate = @'
{
  "slug": "placeholder",
  "display_name": "placeholder",
  "description": "placeholder",
  "base_instructions": "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.",
  "default_reasoning_level": "high",
  "supported_reasoning_levels": [
    { "effort": "none", "description": "Disable Thinking" },
    { "effort": "high", "description": "Enabled Thinking" }
  ],
  "shell_type": "shell_command",
  "visibility": "list",
  "supported_in_api": true,
  "priority": 0,
  "supports_reasoning_summaries": true,
  "default_reasoning_summary": "none",
  "support_verbosity": false,
  "truncation_policy": { "mode": "bytes", "limit": 10000 },
  "supports_parallel_tool_calls": false,
  "supports_image_detail_original": false,
  "context_window": 262144,
  "max_context_window": 262144,
  "effective_context_window_percent": 95,
  "experimental_supported_tools": [],
  "input_modalities": ["text", "image"],
  "supports_search_tool": false
}
'@

function Get-GatewayRoutes {
  $resp = Invoke-RestMethod -Uri "$($BaseUrl.TrimEnd('/'))/api/routes" -TimeoutSec 10
  # 目录只装四个家族槽位（票 31 的「只装网关暴露的虚拟模型」）；-IncludeModel 另点名要的
  # 普通虚拟模型/负例也进。prototype 的 mock 场景与负例默认一律不进目录。
  $wanted = @($resp.routes | Where-Object { $_.family_slot -or ($IncludeModel -contains $_.name) })
  if ($wanted.Count -eq 0) { throw '网关没报出任何家族槽位，先确认网关在跑' }
  foreach ($n in $IncludeModel) {
    if (-not ($wanted | Where-Object { $_.name -eq $n })) { throw "网关没有虚拟模型 $n" }
  }
  return $wanted
}

function New-CatalogEntry($template, $route, $index) {
  $e = $template.PSObject.Copy()
  $e.slug = $route.name
  $e.display_name = $route.name
  $e.description = "Claude Gateway -> $($route.upstream) / $($route.real_model)"
  $e.visibility = 'list'
  $e.context_window = $ContextWindow
  $e.max_context_window = $ContextWindow
  $e.priority = 1000 + $index
  return $e
}

# 写前自检（票 31）：不过就不写，免得 Codex 因为半份目录直接看不到任何模型。
function Assert-Catalog([object]$catalog) {
  $models = @($catalog.models)
  if ($models.Count -eq 0) { throw '目录自检失败：models 为空' }
  if (@($models | Where-Object { $_.visibility -eq 'list' }).Count -eq 0) {
    throw '目录自检失败：没有一条 visibility=list（官方模型会全被藏起来）'
  }
  $slugs = @($models | ForEach-Object { $_.slug })
  if (@($slugs | Where-Object { -not $_ }).Count -gt 0) { throw '目录自检失败：slug 有空值' }
  if (($slugs | Select-Object -Unique).Count -ne $slugs.Count) { throw '目录自检失败：slug 重复' }
  foreach ($m in $models) {
    $hasInstructions = $m.base_instructions -or $m.model_messages.instructions_template
    if (-not $hasInstructions) { throw "目录自检失败：$($m.slug) 缺 base_instructions / instructions_template" }
    if ($null -eq $m.supports_reasoning_summaries) { throw "目录自检失败：$($m.slug) 缺 supports_reasoning_summaries" }
    if ($null -eq $m.supports_parallel_tool_calls) { throw "目录自检失败：$($m.slug) 缺 supports_parallel_tool_calls" }
  }
}

# ---------- 只读状态 ----------

function Show-State {
  Write-Host "CODEX_HOME : $CodexHome"
  Write-Host "config.toml: $(if (Test-Path $configPath) { '存在' } else { '不存在' })"
  Write-Host "备份       : $(if (Test-Path $backupPath) { '有（首写快照）' } else { '无' })"
  Write-Host "目录文件   : $(if (Test-Path $catalogPath) { "有（$((Get-Item $catalogPath).Length) 字节）" } else { '无' })"
  $lines = Split-Raw (Read-Raw)
  if ($lines.Count -eq 0) { return }
  $i = [array]::IndexOf($lines, $topBegin)
  if ($i -ge 0) {
    Write-Host '--- 接管中的行 ---'
    $lines[$i..([array]::IndexOf($lines, $topEnd))] | ForEach-Object { Write-Host "  $_" }
  }
  else { Write-Host '当前没有网关接管的内容。' }
}

# ---------- 自检：认领逻辑的正确性不靠真实配置文件去试 ----------

function Invoke-SelfTest {
  $orig = @(
    'notify = [ "x" ]', '', '[desktop]', 'followUpQueueMode = "steer"', '',
    '[mcp_servers.node_repl]', "command = 'C:\x.exe'"
  )
  $inserted = Add-Managed $orig
  # 模拟实测那一幕：别的写手把自己的表插进了我们的 provider 表后面。
  $co = [System.Collections.Generic.List[string]]::new()
  foreach ($l in $inserted) {
    $co.Add($l)
    if ($l.Trim() -eq 'wire_api = "responses"') { $co.Add('[windows]'); $co.Add('sandbox = "elevated"') }
  }
  $back = @(Remove-Owned $co)
  $expect = @(
    'notify = [ "x" ]', '', '[desktop]', 'followUpQueueMode = "steer"', '',
    '[mcp_servers.node_repl]', "command = 'C:\x.exe'", '[windows]', 'sandbox = "elevated"'
  )
  if (($back -join "`n") -ne ($expect -join "`n")) {
    throw "自检失败：摘键没能回到原样（且必须保住别人的表）`n--- 得到 ---`n$($back -join "`n")"
  }
  $again = @(Remove-Owned (Add-Managed $back))
  if (($again -join "`n") -ne ($back -join "`n")) { throw '自检失败：插入/摘除不幂等' }
  Write-Host '自检通过：插入 → 外部写手插表 → 摘除，回到原样且保住别人的表；重写幂等。'
}

# ---------- 主流程 ----------

Show-State
if ($Mode -eq 'show') { exit 0 }
if ($Mode -eq 'selftest') { Invoke-SelfTest; exit 0 }

if ($Mode -eq 'restore' -or $Mode -eq 'rollback') {
  $raw = Read-Raw
  $nl = Newline-Of $raw
  $current = @(Remove-Owned (Split-Raw $raw))
  if ($Mode -eq 'rollback') {
    if (-not (Test-Path $backupPath)) { throw '没有首写快照，无法整份回滚（用 restore 摘键）' }
    $origRaw = Read-FileRaw $backupPath
    $original = @(Remove-Owned (Split-Raw $origRaw))
    if (($current -join "`n") -ne ($original -join "`n")) {
      throw '文件在接管之后被外部改过（[windows] / [plugins.*] 这类），不做整份回滚；用 -Mode restore 摘键。'
    }
    Copy-Item -LiteralPath $backupPath -Destination $configPath -Force
    Write-Host '已整份回滚 config.toml（首写快照拷回）'
  }
  elseif ($current.Count -eq 0 -and -not (Test-Path $backupPath)) {
    # 首写时文件不存在 → 网关新建的这份整份删掉。
    Remove-Item -LiteralPath $configPath -Force
    Write-Host '已删除 config.toml（首写时它本来就不存在）'
  }
  else {
    Write-Raw (($current -join $nl) + $nl)
    Write-Host '已摘键：拿掉模型目录 / model_provider / model / [model_providers.claudegateway]，其余原样。'
  }
  if (Test-Path $catalogPath) { Remove-Item -LiteralPath $catalogPath -Force; Write-Host '已删除模型目录文件' }
  Write-Host '还原完成。Codex 需完全退出重启。'
  exit 0
}

New-Item -ItemType Directory -Path $CodexHome -Force | Out-Null

# 1. 首写快照：只备份一次，重写不会把快照覆盖成「已接管」的版本。
# 首写时文件不存在就什么都不留：还原时「摘完键文件为空」本身就说明该整份删掉。
if (-not (Test-Path $backupPath) -and (Test-Path $configPath)) {
  Copy-Item -LiteralPath $configPath -Destination $backupPath -Force
  Write-Host "首写备份 -> $backupPath"
}

# 2. 模型目录
$routes = Get-GatewayRoutes
$template = $neutralTemplate | ConvertFrom-Json

$entries = @()
for ($i = 0; $i -lt $routes.Count; $i++) {
  $entries += New-CatalogEntry $template $routes[$i] $i
  Write-Host "目录条目: $($routes[$i].name)  ->  $($routes[$i].upstream) / $($routes[$i].real_model)  [$($routes[$i].wire)]"
}
$catalog = [pscustomobject]@{ models = $entries }
Assert-Catalog $catalog
$catalog | ConvertTo-Json -Depth 100 -Compress | Set-Content -LiteralPath $catalogPath -Encoding utf8 -NoNewline
Write-Host "模型目录 -> $catalogPath（$((Get-Item $catalogPath).Length) 字节，$($entries.Count) 条）"

# 3. config.toml：先摘掉自己那几行，再插回去。
$raw = Read-Raw
$nl = Newline-Of $raw
$out = Add-Managed (Remove-Owned (Split-Raw $raw))
Write-Raw (($out -join $nl) + $nl)
Write-Host "已写入 $configPath（行尾 $(if ($nl -eq "`r`n") { 'CRLF' } else { 'LF' })）"
Write-Host ''
Write-Host '下一步：完全退出并重启 Codex（CLI 直接新开一个终端即可）。'
