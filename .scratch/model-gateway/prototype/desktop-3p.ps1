# 票 15 的最小「配置写入引擎」：写/撤 Claude Desktop 3P 的 configLibrary。
#   -Mode gateway   写入网关 profile 并置为生效（首次写入前整目录备份）
#   -Mode standard  从所有 profile 里摘掉网关字段（app 回标准/1P 模式）
#   -Mode restore   用首次备份整目录还原（大退路）
# 备份只做一次，路径记在 configLibrary.backup.path，避免后续写入覆盖掉原始状态。
param(
  [Parameter(Mandatory = $true)]
  [ValidateSet('gateway', 'standard', 'restore')]
  [string]$Mode,
  [string]$BaseUrl = 'http://127.0.0.1:8787',
  [string]$ApiKey = 'prototype-local-key'
)

$ErrorActionPreference = 'Stop'

$library = Join-Path $env:LOCALAPPDATA 'Claude-3p\configLibrary'
$backupPointer = Join-Path $env:LOCALAPPDATA 'Claude-3p\configLibrary.backup.path'
# 固定 id → 反复执行幂等，不会在 entries 里堆出一串 profile。
$profileId = '9f1c2b3a-4d5e-4f60-8a71-000000000001'
$profilePath = Join-Path $library "$profileId.json"
$metaPath = Join-Path $library '_meta.json'

$gatewayKeys = @(
  'inferenceProvider', 'inferenceGatewayBaseUrl', 'inferenceGatewayApiKey',
  'inferenceGatewayAuthScheme', 'inferenceCredentialKind', 'inferenceModels'
)

function Backup-Once {
  if (Test-Path $backupPointer) {
    Write-Host "备份已存在，跳过：$(Get-Content $backupPointer -Raw)"
    return
  }
  if (-not (Test-Path $library)) {
    Write-Host "configLibrary 不存在，无内容可备份（首次进入 3P）。"
    return
  }
  $dest = "$library.backup-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
  Copy-Item -LiteralPath $library -Destination $dest -Recurse
  Set-Content -LiteralPath $backupPointer -Value $dest -NoNewline
  Write-Host "已备份原 configLibrary → $dest"
}

function Read-Json([string]$path) {
  if (-not (Test-Path $path)) { return $null }
  return Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
}

function Write-Json($object, [string]$path) {
  $object | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $path -Encoding utf8
}

switch ($Mode) {
  'gateway' {
    Backup-Once
    New-Item -ItemType Directory -Force -Path $library | Out-Null

    Write-Json ([ordered]@{
        inferenceProvider         = 'gateway'
        inferenceGatewayBaseUrl   = $BaseUrl
        inferenceGatewayApiKey    = $ApiKey
        inferenceGatewayAuthScheme = 'bearer'
        inferenceCredentialKind   = 'static'
        # 四项全是完整模型 ID（网关侧的路由名）→ app 会跳过 /v1/models 自动发现。
        inferenceModels           = @(
          @{ name = 'claude-fable' }
          @{ name = 'claude-opus' }
          @{ name = 'claude-sonnet' }
          @{ name = 'claude-haiku' }
        )
      }) $profilePath

    $meta = Read-Json $metaPath
    if ($null -eq $meta) { $meta = [pscustomobject]@{ appliedId = $null; entries = @() } }
    $entries = @($meta.entries | Where-Object { $_.id -ne $profileId })
    $meta.entries = $entries + [pscustomobject]@{ id = $profileId; name = 'Claude Gateway (prototype)' }
    $meta.appliedId = $profileId
    Write-Json $meta $metaPath

    Write-Host "3P 已写入并生效："
    Write-Host "  profile : $profilePath"
    Write-Host "  _meta   : appliedId=$profileId"
    Write-Host "  baseUrl : $BaseUrl  (authScheme=bearer)"
    Write-Host "  模型    : claude-fable / claude-opus / claude-sonnet / claude-haiku"
    Write-Host "改完需完全退出 Claude Desktop 再启动。"
  }

  'standard' {
    if (-not (Test-Path $library)) { Write-Host "configLibrary 不存在，本来就是标准模式。"; break }
    $touched = 0
    Get-ChildItem -LiteralPath $library -Filter '*.json' -File |
      Where-Object { $_.Name -ne '_meta.json' } |
      ForEach-Object {
        $cfg = Read-Json $_.FullName
        if ($null -eq $cfg) { return }
        $had = $false
        foreach ($k in $gatewayKeys) {
          if ($cfg.PSObject.Properties.Name -contains $k) { $cfg.PSObject.Properties.Remove($k); $had = $true }
        }
        if ($had) { Write-Json $cfg $_.FullName; Write-Host "已摘掉网关字段：$($_.Name)"; $touched++ }
      }
    if ($touched -eq 0) { Write-Host "没有 profile 带网关字段（已是 1P）。" }
    Write-Host "app 认的激活动词是 inferenceProvider，字段一摘即回标准模式；登录态在 1P 的 userData 目录里，未被动过。"
    Write-Host "要整目录按字节还原原状：-Mode restore"
  }

  'restore' {
    $dest = if (Test-Path $backupPointer) { Get-Content $backupPointer -Raw } else { $null }
    if (-not $dest -or -not (Test-Path $dest)) { throw "找不到备份（$backupPointer），无法还原。" }
    if (Test-Path $library) { Remove-Item -LiteralPath $library -Recurse -Force }
    Copy-Item -LiteralPath $dest -Destination $library -Recurse
    Write-Host "已从 $dest 整目录还原 configLibrary。"
  }
}
