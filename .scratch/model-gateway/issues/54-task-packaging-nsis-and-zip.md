# 打包：NSIS per-user + 免安装 zip

Type: task
Status: open
Blocked by: 43

## Question

把应用打成能装的东西：NSIS **per-user** 安装包（装进 `%LOCALAPPDATA%`、不需要管理员）+ 手工打包的免安装 zip。不签名（[票 18](../issues/18-grilling-packaging-and-updates.md)：官方无 portable bundle，zip 不参与自动更新）。

依据：spec §12 M2、[票 18](../issues/18-grilling-packaging-and-updates.md)。

## 交付

- `tauri.conf.json` 的 bundle 段：NSIS target、per-user 安装模式、图标、`productName = Claude Gateway`、identifier `com.claudegateway.desktop`（一经发布不得改）。
- 免安装 zip 的打包脚本（手工，进 `check.ps1 -Smoke` 的产物清单或单独脚本）。
- 数据目录 `%APPDATA%\com.claudegateway.desktop\`：安装版与 zip 版共用；设置页只读路径 + 打开目录，不提供搬家。
- CI 打 tag 出包：NSIS + zip + `latest.json` 占位（真签名与升级在 [票 61](../issues/61-task-auto-update.md)）。

## 收口验收

- 本机装 NSIS（不弹 UAC）→ 启动 → 能用网关（此时功能按已完成的票）。
- zip 解压即跑，且与安装版读到同一份数据目录。
- 卸载**保留数据**（原生「Delete app data」勾选，默认不删）；还原客户端配置的动作归 [票 62](../issues/62-task-autostart-uninstall-export.md)。
