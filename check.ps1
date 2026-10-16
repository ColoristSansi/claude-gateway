# 回归门禁的单一入口（票 19 §7、票 29 第 6 条；票 39 从原型搬进正牌骨架）。
#
#   pwsh -File .\check.ps1            # 全跑：fmt + clippy + 单元测试 + 前端构建 + 回放集成测试
#   pwsh -File .\check.ps1 -Quick     # 秒级：fmt + clippy + 单元测试 + 前端构建
#   pwsh -File .\check.ps1 -Replay    # 回放集成测试（测试自己起网关与 mock 两个进程）
#   pwsh -File .\check.ps1 -Live      # 打真实上游的 #[ignore] 手动测试（读 .scratch/model-gateway/prototype-upstream.env）
#   pwsh -File .\check.ps1 -Smoke     # 打印客户端冒烟清单，逐项等人签字
#
# 本脚本在仓库根运行，覆盖 workspace 全部成员。
# 票 41 起回放套件与 fixtures 在产品侧 `crates/gateway-core/tests/`：mock 上游与回放 harness
# 是两个**测试专用 bin**（`required-features = ["mock"]`），所以跑回放必须带 `--features mock`；
# 产品构建（`cargo build -p gateway-core -p claude-gateway`）不带它，产物里没有 mock 的痕迹。
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
  # 测试专用件（mock 上游 bin + 回放 harness + 回放测试）只在 mock 特性下才编，所以单独 lint 一遍。
  $steps.Add(@{ name = 'clippy (mock 特性：两个测试 bin + 回放测试)'; run = { cargo clippy -p gateway-core --features mock --all-targets -- -D warnings } })
  # 前端（4430 行 TSX）的类型检查 + 构建：`app/package.json` 的 build = `tsc --noEmit && vite build`。
  # 从前它只在 `pack.ps1`（tag / 打包）里跑，票 60b 的整页白屏就是这类漏网；离线可跑（依赖装过即可）。
  $steps.Add(@{ name = 'web    (pnpm --dir app build: tsc --noEmit + vite build)'; run = {
      if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
        Write-Host 'pnpm 不在 PATH 上：门禁要求前端类型检查（CI 会装）。装法：npm i -g pnpm，然后 pnpm --dir app install' -ForegroundColor Red
        $global:LASTEXITCODE = 1
        return
      }
      if (-not (Test-Path (Join-Path $PSScriptRoot 'app\node_modules'))) {
        Write-Host 'app\node_modules 不在：先跑 pnpm --dir app install（CI 里排在门禁之前）' -ForegroundColor Red
        $global:LASTEXITCODE = 1
        return
      }
      pnpm --dir app build
    } })
}
if ($Replay) {
  $steps.Add(@{ name = 'replay (集成：自起网关 + mock 两个进程，回放 fixtures)'; run = { cargo test -p gateway-core --features mock --test replay } })
  $steps.Add(@{ name = 'replay-unit (回放器切法单测，mock 特性下的 bin)'; run = { cargo test -p gateway-core --features mock --bins } })
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
