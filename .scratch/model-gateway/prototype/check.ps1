# 回归门禁的单一入口（票 19 §7、票 29 第 6 条）。取代已退役的 verify.ps1。
#
#   pwsh -File .\check.ps1            # 全跑：fmt + clippy + 单元测试 + 回放集成测试
#   pwsh -File .\check.ps1 -Quick     # 秒级：fmt + clippy + 单元测试
#   pwsh -File .\check.ps1 -Replay    # 回放集成测试（测试自己起网关与 mock）
#   pwsh -File .\check.ps1 -Live      # 打真实上游的 #[ignore] 手动测试（读 prototype-upstream.env）
#   pwsh -File .\check.ps1 -Smoke     # 打印客户端冒烟清单，逐项等人签字
#
# 不带参数不跑 Live 与 Smoke 的人工部分；聚合出口码，最后列出失败项。
[CmdletBinding()]
param(
  [switch]$Quick,
  [switch]$Replay,
  [switch]$Live,
  [switch]$Smoke
)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

if (-not ($Quick -or $Replay -or $Live -or $Smoke)) { $Quick = $true; $Replay = $true }

$steps = [System.Collections.Generic.List[object]]::new()
if ($Quick) {
  $steps.Add(@{ name = 'fmt    (cargo fmt --check)'; run = { cargo fmt --check } })
  $steps.Add(@{ name = 'clippy (cargo clippy -D warnings)'; run = { cargo clippy --all-targets -- -D warnings } })
  $steps.Add(@{ name = 'unit   (cargo test --bins)'; run = { cargo test --bins } })
}
if ($Replay) { $steps.Add(@{ name = 'replay (集成：自起网关 + mock，回放 fixtures)'; run = { cargo test --test replay } }) }
if ($Live) { $steps.Add(@{ name = 'live   (真实上游 #[ignore] 测试)'; run = { cargo test -- --ignored } }) }

$failed = @()
foreach ($s in $steps) {
  Write-Host "`n=== $($s.name) ===" -ForegroundColor Cyan
  & $s.run
  if ($LASTEXITCODE -ne 0) { $failed += "$($s.name) -> exit $LASTEXITCODE" }
}

if ($Smoke) {
  $smokeFile = Join-Path $PSScriptRoot '..\smoke.md'
  Write-Host "`n=== smoke（agent 做不到、必须人做的那半）===" -ForegroundColor Cyan
  if (Test-Path $smokeFile) {
    Get-Content $smokeFile | ForEach-Object { Write-Host "  $_" }
    $answer = Read-Host '逐项确认完了吗？(y/N)'
    if ($answer -ne 'y') { $failed += 'smoke 未确认' }
  }
  else {
    Write-Warning "还没有 $smokeFile（票 30 第 11 格负责写它）；这一步先跳过。"
  }
}

Write-Host ''
if ($failed.Count -gt 0) {
  Write-Host "不得通过，$($failed.Count) 项失败：" -ForegroundColor Red
  $failed | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
  exit 1
}
Write-Host '全部通过。' -ForegroundColor Green
exit 0
