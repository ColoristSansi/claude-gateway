# 本地更新源（票 61）：把 <target>\release\bundle\updates\ 挂在 http://127.0.0.1:8791/ 上。
#
#   pwsh -File .\serve-updates.ps1                 # 默认端口 8791、默认目录见下
#   pwsh -File .\serve-updates.ps1 -Port 8791 -Root D:\somewhere
#
# 只给**本机闭环验收**用（生产更新源是公开 GitHub Releases，见 src-tauri/tauri.conf.json）。
# 装在 127.0.0.1 上、只读、只认 GET/HEAD；用 TcpListener 而不是 HttpListener —— 后者在
# 非管理员会话里注册 URL 前缀会 Access is denied，而验收不该要求提权。
#
# 起来之后应用（用 `--config src-tauri/tauri.conf.test.json` 打的包）会 GET 这两个：
#   /latest.json                            更新清单（版本 / 平台 / 签名 / 下载地址）
#   /Claude%20Gateway_<版本>_x64-setup.exe  安装包本体
[CmdletBinding()]
param(
  [int]$Port = 8791,
  [string]$Root,
  [string]$LogFile
)
$ErrorActionPreference = 'Stop'

if (-not $Root) {
  if ($env:CARGO_TARGET_DIR) { $targetDir = $env:CARGO_TARGET_DIR }
  else {
    $meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata 失败（exit $LASTEXITCODE）" }
    $targetDir = $meta.target_directory
  }
  $Root = Join-Path $targetDir 'release\bundle\updates'
}
$Root = (Resolve-Path $Root).Path
if (-not $LogFile) { $LogFile = Join-Path $Root 'serve.log' }

function Write-Log([string]$line) {
  $stamped = "[{0}] {1}" -f (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'), $line
  Write-Host $stamped
  Add-Content -Path $LogFile -Value $stamped
}

$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
$listener.Start()
Write-Log "本地更新源已起：http://127.0.0.1:$Port/ → $Root"

try {
  while ($true) {
    $client = $listener.AcceptTcpClient()
    try {
      $stream = $client.GetStream()
      $stream.ReadTimeout = 5000
      $reader = [System.IO.StreamReader]::new($stream, [System.Text.Encoding]::ASCII, $false, 4096, $true)
      $requestLine = $reader.ReadLine()
      if (-not $requestLine) { continue }
      # 头读完就够（本服务只发固定长度的整体响应，不读 body）
      while ($true) {
        $line = $reader.ReadLine()
        if ($null -eq $line -or $line -eq '') { break }
      }

      $parts = $requestLine.Split(' ')
      $method = $parts[0]
      $rawTarget = if ($parts.Length -gt 1) { $parts[1] } else { '/' }
      $path = ($rawTarget -split '\?')[0]
      $name = [uri]::UnescapeDataString($path.TrimStart('/'))
      $name = ($name -split '/')[-1]           # 只认文件名：不许用 ../ 走出 Root
      $file = if ($name) { Join-Path $Root $name } else { $null }

      if ($file -and (Test-Path -LiteralPath $file -PathType Leaf) -and $file.StartsWith($Root, 'OrdinalIgnoreCase')) {
        $bytes = [System.IO.File]::ReadAllBytes($file)
        $ext = [System.IO.Path]::GetExtension($file).ToLowerInvariant()
        $type = switch ($ext) {
          '.json' { 'application/json' }
          '.exe' { 'application/octet-stream' }
          '.sig' { 'text/plain' }
          default { 'application/octet-stream' }
        }
        $head = "HTTP/1.1 200 OK`r`nContent-Type: $type`r`nContent-Length: $($bytes.Length)`r`nConnection: close`r`n`r`n"
        $headBytes = [System.Text.Encoding]::ASCII.GetBytes($head)
        $stream.Write($headBytes, 0, $headBytes.Length)
        if ($method -ne 'HEAD') { $stream.Write($bytes, 0, $bytes.Length) }
        $stream.Flush()
        Write-Log "$method $path -> 200 ($($bytes.Length) 字节) $name"
      }
      else {
        $body = [System.Text.Encoding]::UTF8.GetBytes("not found: $path`n")
        $head = "HTTP/1.1 404 Not Found`r`nContent-Type: text/plain`r`nContent-Length: $($body.Length)`r`nConnection: close`r`n`r`n"
        $headBytes = [System.Text.Encoding]::ASCII.GetBytes($head)
        $stream.Write($headBytes, 0, $headBytes.Length)
        $stream.Write($body, 0, $body.Length)
        $stream.Flush()
        Write-Log "$method $path -> 404"
      }
    }
    catch { Write-Log "请求处理失败：$($_.Exception.Message)" }
    finally { $client.Close() }
  }
}
finally {
  $listener.Stop()
  Write-Log '本地更新源已停'
}
