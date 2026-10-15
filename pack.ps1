# 打包（票 54；定案见票 18 §1）：NSIS per-user 安装包 + 免安装 zip + 更新清单 latest.json。
#
#   pwsh -File .\pack.ps1                                   # 不打签名的裸包（本地开发）
#   pwsh -File .\pack.ps1 -Sign -BaseUrl http://127.0.0.1:8791
#   pwsh -File .\pack.ps1 -Sign -BaseUrl https://github.com/<owner>/<repo>/releases/download/v0.2.0 \
#                          -PrivateKeyPath C:\path\updater.key
#
# 手工触发；CI 打 tag 时走同一条路（.github/workflows/ci.yml 的 bundle job），不另写一套。
# 产物落在 cargo 的 target 目录（尊重 CARGO_TARGET_DIR）：
#   release\bundle\nsis\Claude Gateway_<版本>_x64-setup.exe   per-user，装 %LOCALAPPDATA%
#   release\bundle\zip\Claude-Gateway_<版本>_x64-portable.zip 解压即跑
#   release\bundle\updates\latest.json                        真签名清单（updater 读它）
#   release\bundle\updates\Claude Gateway_<版本>_x64-setup.exe 与 latest.json 同源的安装包
#   release\bundle\updates\<安装包>.sig                       签名文件（给人工核对用）
#
# 票 61：签名走 `cargo tauri signer sign --app-version <版本>`（不是 bundle 的
# createUpdaterArtifacts）——理由：本地没钥匙时也要能出包（CI 的 check job 与开发机不打 tag 时
# 都不该因为缺密钥而失败），且 latest.json 里的下载地址要我们自己拼（GitHub Release 的
# 资产名与本地源的地址不是一回事）。`--app-version` 是必须的：生产配置开了
# requireSignedVersion，签名里不带版本会被插件直接拒（MissingSignedVersion）。
[CmdletBinding()]
param(
  # 打签名 + 写真的 latest.json（缺密钥就报错退出，不静默出个装不上的清单）。
  [switch]$Sign,
  # latest.json 里安装包地址的前缀（不带尾斜杠）。本地验收用 http://127.0.0.1:8791，
  # 正式发布用 https://github.com/<owner>/<repo>/releases/download/<tag>。不传就不写 platforms。
  [string]$BaseUrl,
  # 私钥文件（**用户资产**，绝不进仓库）：缺省读环境变量 TAURI_SIGNING_PRIVATE_KEY_PATH，
  # 再缺省读 TAURI_SIGNING_PRIVATE_KEY（CI 里放的是密钥内容本身）。
  [string]$PrivateKeyPath,
  # 私钥口令：缺省读 TAURI_SIGNING_PRIVATE_KEY_PASSWORD。
  [string]$PrivateKeyPassword,
  # 挂一份覆盖配置（如 src-tauri\tauri.conf.test.json：本地 http 更新源）。
  [string]$Config
)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

if ($env:CARGO_TARGET_DIR) { $targetDir = $env:CARGO_TARGET_DIR }
else {
  $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
  if ($LASTEXITCODE -ne 0) { throw "cargo metadata 失败（exit $LASTEXITCODE）" }
  $targetDir = $meta.target_directory
}

# 前端由 tauri.conf.json 的 beforeBuildCommand（pnpm --dir ../app build）现编进 frontendDist，
# 这里不重复调；NSIS target 与 per-user 安装模式也都在 tauri.conf.json 里。
$buildArgs = @('tauri', 'build')
if ($Config) { $buildArgs += @('--config', $Config) }
cargo @buildArgs
if ($LASTEXITCODE -ne 0) { throw "cargo tauri build 失败（exit $LASTEXITCODE）" }

$releaseDir = Join-Path $targetDir 'release'
$version = (Get-Content (Join-Path $PSScriptRoot 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version

# zip 版 = 免安装：只有一个 exe（前端编在 exe 里，没有额外资源目录）。
# 它与安装版共用 %APPDATA%\com.claudegateway.desktop（Tauri 按 identifier 命名，票 18 §5）。
$exe = Join-Path $releaseDir 'claude-gateway.exe'
if (-not (Test-Path $exe)) { throw "没找到 $exe" }
$zipDir = Join-Path $releaseDir 'bundle\zip'
$null = New-Item -ItemType Directory -Force -Path $zipDir
$zip = Join-Path $zipDir "Claude-Gateway_${version}_x64-portable.zip"
Compress-Archive -Path $exe -DestinationPath $zip -Force

# 安装包：NSIS 产物（updater 在 Windows 上装的就是它）。
$setup = Get-ChildItem (Join-Path $releaseDir 'bundle\nsis') -Filter '*_x64-setup.exe' -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) { throw "没找到 NSIS 安装包（$releaseDir\bundle\nsis\*_x64-setup.exe）" }

$pubDate = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$updatesDir = Join-Path $releaseDir 'bundle\updates'
$null = New-Item -ItemType Directory -Force -Path $updatesDir
$staged = Join-Path $updatesDir $setup.Name
Copy-Item $setup.FullName $staged -Force

$notes = "Claude Gateway $version（票 61：签名产物与 latest.json 由 pack.ps1 -Sign 产出）"

if ($Sign) {
  if ($env:TAURI_SIGNING_PRIVATE_KEY -and -not $PrivateKeyPath) {
    # CI：密钥内容本身放在 Secret 里（票 63 配）。
    Write-Host '用环境变量 TAURI_SIGNING_PRIVATE_KEY 里的私钥签名' -ForegroundColor Cyan
    $signArgs = @('tauri', 'signer', 'sign', $staged, '--app-version', $version)
  }
  else {
    $keyPath = if ($PrivateKeyPath) { $PrivateKeyPath } elseif ($env:TAURI_SIGNING_PRIVATE_KEY_PATH) { $env:TAURI_SIGNING_PRIVATE_KEY_PATH } else { $null }
    if (-not $keyPath) { throw '要求签名但没有私钥：给 -PrivateKeyPath，或设 TAURI_SIGNING_PRIVATE_KEY_PATH / TAURI_SIGNING_PRIVATE_KEY' }
    if (-not (Test-Path $keyPath)) { throw "私钥文件不存在：$keyPath" }
    Write-Host "用私钥文件签名（内容不打印）：$keyPath" -ForegroundColor Cyan
    $signArgs = @('tauri', 'signer', 'sign', $staged, '--private-key-path', $keyPath, '--app-version', $version)
  }
  $password = if ($PrivateKeyPassword) { $PrivateKeyPassword } else { $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD }
  if ($password) { $signArgs += @('--password', $password) }

  # 口令经环境变量传给 signer，避免出现在进程命令行里（`ps` / 事件日志能看到命令行）。
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $password
  cargo @signArgs
  if ($LASTEXITCODE -ne 0) { throw "签名失败（exit $LASTEXITCODE）" }

  $sigFile = "$staged.sig"
  if (-not (Test-Path $sigFile)) { throw "签名命令没产出 $sigFile" }
  $signature = (Get-Content $sigFile -Raw).Trim()

  if (-not $BaseUrl) { throw '签名时必须给 -BaseUrl（latest.json 里的下载地址前缀）' }
  $url = "$($BaseUrl.TrimEnd('/'))/$([uri]::EscapeDataString($setup.Name))"
  $platforms = [ordered]@{}
  # 装机版（bundle_type = nsis）先查带安装器名的那一个，再退到不带的那一个：两份都写。
  foreach ($key in @('windows-x86_64-nsis', 'windows-x86_64')) {
    $platforms[$key] = [ordered]@{ signature = $signature; url = $url }
  }
  [ordered]@{
    version   = $version
    notes     = $notes
    pub_date  = $pubDate
    platforms = $platforms
  } | ConvertTo-Json -Depth 5 | Set-Content -Path (Join-Path $updatesDir 'latest.json') -Encoding utf8

  Write-Host "`n签名清单：" -ForegroundColor Cyan
  Write-Host "  $(Join-Path $updatesDir 'latest.json')"
  Write-Host "  下载地址：$url"
}
else {
  # 没签名 = 出不了能用的更新清单：给一份**明显不可用**的占位，写清原因，别让人误以为能升级。
  [ordered]@{
    version   = $version
    notes     = '占位（未签名）：本文件不可用于自动更新，请用 pack.ps1 -Sign 重打。'
    pub_date  = $pubDate
    platforms = @{}
  } | ConvertTo-Json | Set-Content -Path (Join-Path $updatesDir 'latest.json') -Encoding utf8
  Write-Host '没签名（-Sign 未给）：latest.json 是占位，不可用于自动更新。' -ForegroundColor Yellow
}

# 旧的 zip\latest.json 位置（票 54）不再写：清单只有一份，在 bundle\updates\。
Remove-Item (Join-Path $zipDir 'latest.json') -ErrorAction SilentlyContinue

Write-Host "`n产物：" -ForegroundColor Cyan
Get-ChildItem (Join-Path $releaseDir 'bundle\nsis') -Filter *.exe -ErrorAction SilentlyContinue |
  ForEach-Object { Write-Host "  $($_.FullName)" }
Get-ChildItem $zipDir | ForEach-Object { Write-Host "  $($_.FullName)" }
Get-ChildItem $updatesDir | ForEach-Object { Write-Host "  $($_.FullName)" }
