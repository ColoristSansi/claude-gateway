# 回归门禁的单一入口（票 19 §7、票 29 第 6 条；票 39 从原型搬进正牌骨架）。
#
#   pwsh -File .\check.ps1            # 全跑：fmt + clippy + 单元测试 + 回放集成测试
#   pwsh -File .\check.ps1 -Quick     # 秒级：fmt + clippy + 单元测试
#   pwsh -File .\check.ps1 -Replay    # 回放集成测试（测试自己起网关与 mock）
#   pwsh -File .\check.ps1 -Live      # 打真实上游的 #[ignore] 手动测试（读 .scratch/model-gateway/prototype-upstream.env）
#   pwsh -File .\check.ps1 -Smoke     # 打印客户端冒烟清单，逐项等人签字
#
# 本脚本在仓库根运行，覆盖 workspace 全部成员。回放测试与 fixtures 随票 41 搬入，
# 在此之前 -Replay / -Live 只占位、不算失败。
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
  $steps.Add(@{ name = 'fmt    (cargo fmt --all --check)'; run = { cargo fmt --all --check } })
  $steps.Add(@{ name = 'clippy (cargo clippy --workspace -D warnings)'; run = { cargo clippy --workspace --all-targets -- -D warnings } })
  $steps.Add(@{ name = 'unit   (cargo test --workspace)'; run = { cargo test --workspace } })
}
if ($Replay) {
  $steps.Add(@{ name = 'replay (集成：自起网关 + mock，回放 fixtures)'; run = {
      $replayTest = Get-ChildItem -Path 'crates' -Filter 'replay.rs' -Recurse -File -ErrorAction SilentlyContinue
      if ($replayTest) { cargo test --workspace --test replay }
      else { Write-Host '还没有回放测试目标（票 41 搬入 crates/gateway-core/tests/replay.rs）；这一步先跳过。' -ForegroundColor Yellow }
    } })
}
if ($Live) {
  $steps.Add(@{ name = 'live   (真实上游 #[ignore] 测试)'; run = { cargo test --workspace -- --ignored } })
}

$failed = @()
foreach ($s in $steps) {
  Write-Host "`n=== $($s.name) ===" -ForegroundColor Cyan
  $LASTEXITCODE = 0   # 每步一个基准：占位步骤不跑原生命令，别继承上一步的码
  & $s.run
  if ($LASTEXITCODE -ne 0) { $failed += "$($s.name) -> exit $LASTEXITCODE" }
}

if ($Smoke) {
  # 票 30 第 11 格负责写它；位置随规划目录，不搬进代码树。
  $smokeFile = Join-Path $PSScriptRoot '.scratch\model-gateway\smoke.md'
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
