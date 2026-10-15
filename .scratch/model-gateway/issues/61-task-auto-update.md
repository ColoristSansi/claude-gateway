# 自动升级

Type: task
Status: open
Blocked by: 54

## Question

把升级闭环做起来：公开 GitHub Releases + `latest.json` + 应用内检查 + 签名校验 + passive 静默安装 + 自动重启。**升级不改端口、不改数据、不改客户端配置。**

依据：spec §12 M4、[票 18](../issues/18-grilling-packaging-and-updates.md)。

## 交付

- tauri updater 配置：https 强制、签名校验不可关、`installMode` passive/quiet 可静默装、装完 `exit(0)` 由 updater 拉起。
- 签名密钥对：公钥进应用配置；**私钥进 GitHub Secret，另留一份交付用户保管**（说明它丢了以后再也发不了升级）。
- 应用内「检查更新」与提示；用户确认后静默安装并自动重启。
- CI 打 tag 产出签名产物与 `latest.json`。
- 数据目录另留一份上一版包 + 回退指引（写在设置页「关于」或帮助里）。

## 收口验收

- 本机装 v1 → 发一个 v2 release → 应用内升级成功、重启后版本为 v2、端口与数据与客户端配置都没变。
- 签名校验故意用错公钥时拒绝安装并给出可读错误。
