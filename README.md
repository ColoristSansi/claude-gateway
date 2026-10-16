# Claude Gateway

本机跑一个模型网关（Tauri 2 + Rust + React），把第三方上游聚合成一份**本地模型目录**，
让 Claude Desktop（3P 模式）、Claude Code、Codex 三个客户端都能用上第三方模型。

- 转发面 `http://127.0.0.1:8787/v1/*`，管理面 `/api/*`；界面在应用窗口里，**没有网页版**。
- 四个家族槽位：`claude-fable` / `claude-opus` / `claude-sonnet` / `claude-haiku`；
  协议随**虚拟模型**走，不随上游走。
- 客户端配置是**显式接管键清单 + 原子替换**写入，随时可一键还原成官方配置；
  凭据只以 DPAPI 密文入库，响应里只有掩码。

## 下载与安装

到 [Releases](../../releases/latest) 拿两个产物（同一份数据目录，选一个即可）：

| 产物 | 装法 |
| --- | --- |
| `Claude Gateway_<版本>_x64-setup.exe` | per-user 安装（**不需要管理员**），装到 `%LOCALAPPDATA%\Claude Gateway\` |
| `Claude-Gateway_<版本>_x64-portable.zip` | 解压即用（**不要**在压缩包里直接跑） |

数据目录 `%APPDATA%\com.claudegateway.desktop\`（`gateway.db` / `backups\` / `logs\` / `updates\`），
安装版与免安装版**共用**它。

### 首次运行会看到 SmartScreen

安装包与 exe **没有代码签名证书**（本项目的既定取舍），所以 Windows 会拦一次：

> **Windows 已保护你的电脑** → 点「**更多信息**」→ 点「**仍要运行**」。

升级走的是**应用内签名校验**（Ed25519，公钥内置在应用里、私钥不入库），与这条 SmartScreen 提示无关。

## 升级与卸载

- 升级：「设置 → 关于与版本 → 检查更新」（或顶栏横幅）；下载地址与签名来自本仓库 Releases 的 `latest.json`。
  升级**不重写**客户端配置、不动数据目录。
- 卸载：`%LOCALAPPDATA%\Claude Gateway\uninstall.exe`。确认页的「**Delete app data**」**默认不勾**（数据保留）；
  之后会问一次「**卸载前要把三个客户端的配置还原成官方状态吗？**」——**默认「是（还原）」**。
  三个客户端都要**完全退出后重开**才会读到新配置。

## 从源码构建 / 开发

```powershell
pnpm --dir app install            # 前端依赖（门禁的 web 步与 tauri 的 beforeBuildCommand 都要）
pwsh -File .\check.ps1            # 全量门禁：fmt + clippy + 单元测试 + 前端构建 + 回放集成测试
pwsh -File .\check.ps1 -Quick -Replay   # CI 跑的那一组（单元 + 前端 + 回放）
pwsh -File .\pack.ps1             # 打包 NSIS + 免安装 zip（-Sign 出签名产物与 latest.json）
pwsh -File .\serve-updates.ps1    # 本地更新源（配合 pack.ps1 验证升级闭环）
```

回放集成测试自己拉起网关与 mock 上游两个进程，**不打真实上游**；打真上游的是 `#[ignore]` 的手动项
（`check.ps1 -Live`，读 `.scratch/model-gateway/prototype-upstream.env`）。

## 文档

| 文件 | 内容 |
| --- | --- |
| [`START-HERE.md`](START-HERE.md) | 交接单：现在在哪、哪些坑别再踩 |
| [`GLOSSARY.md`](GLOSSARY.md) | 术语表（请求目标一律叫「上游」） |
| [`docs/adr/`](docs/adr/) | 七条硬决策（协议转换策略、槽位、外壳、供应商表粒度、回归策略、接管键、实施顺序） |
| [`.scratch/model-gateway/spec.md`](.scratch/model-gateway/spec.md) | 技术选型与架构设计 |
| [`.scratch/model-gateway/issues/`](.scratch/model-gateway/issues/) | 逐票实现记录（每张票末尾有 `## Answer`） |

## License

[MIT](LICENSE)
