# 打包（票 54；定案见票 18 §1）：NSIS per-user 安装包 + 免安装 zip + latest.json 占位。
#
#   pwsh -File .\pack.ps1
#
# 手工触发；CI 打 tag 时走同一条路（.github/workflows/ci.yml 的 bundle job），不另写一套。
# 产物落在 cargo 的 target 目录（尊重 CARGO_TARGET_DIR）：
#   release\bundle\nsis\Claude Gateway_<版本>_x64-setup.exe   per-user，装 %LOCALAPPDATA%
#   release\bundle\zip\Claude-Gateway_<版本>_x64-portable.zip 解压即跑
#   release\bundle\zip\latest.json                            占位，票 61 才写真签名与升级
[CmdletBinding()]
param()
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
cargo tauri build
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

# latest.json 占位：票 61 之前没有签名，这个文件不可用于自动更新（updater 强制校验签名）。
[ordered]@{
  version   = $version
  notes     = '占位（票 54）：真签名与升级流在票 61 落地，本文件不可用于自动更新。'
  pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
  platforms = @{}
} | ConvertTo-Json | Set-Content -Path (Join-Path $zipDir 'latest.json') -Encoding utf8

Write-Host "`n产物：" -ForegroundColor Cyan
Get-ChildItem (Join-Path $releaseDir 'bundle\nsis') -Filter *.exe -ErrorAction SilentlyContinue |
  ForEach-Object { Write-Host "  $($_.FullName)" }
Get-ChildItem $zipDir | ForEach-Object { Write-Host "  $($_.FullName)" }
