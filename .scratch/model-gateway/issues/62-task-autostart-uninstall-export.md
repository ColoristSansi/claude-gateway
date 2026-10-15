# 自启动、卸载与导出

Type: task
Status: resolved
Blocked by: 45, 54

## Question

补齐分发的收尾三件：开机自启（默认关）、卸载流程（保留数据 + 一次还原询问）、导出 / 导入。

依据：spec §12 M4、[票 18](../issues/18-grilling-packaging-and-updates.md)、spec §9、spec §13（DPAPI 换机不可解密）。

## 交付

- 开机自启：默认**关闭**；开启时写 HKCU Run 键（避开卸载残留），托盘 / 设置页可切。
- 卸载：默认**保留数据**（原生「Delete app data」勾选）；卸载时弹一次「还原客户端配置 / 保持现状」（**默认还原**）；卸载器**只删自己那两个 identifier 目录**，`%APPDATA%\Claude`、`~/.claude`、`~/.codex` 一律不动。
- 导出 / 导入（设置页）：默认**不含明文密钥**；可选带密码的加密导出；导入能识别版本与重复上游。
- DPAPI 换机不可解密的界面提示（导出时明确说清）。

## 收口验收

- 自启开关实际生效（重启或手工验证 HKCU Run 值），关闭后无残留。
- 卸载流程实测：选「还原」→ 三个客户端回官方态、数据目录保留；选「保持现状」→ 客户端配置不动。
- 导出文件里反查不到明文密钥；导入回来网关可用。

## Answer

2026-10-15 完成。基于集成分支 tip `2dfe335`（已含票 39–57 里已合的），实现 commit `9d4aeb2`，合并 commit `23064c0`（把票 53 日志面 / 票 57 结构化输出的 tip 合进来，冲突就地解决）。

### 做了什么

新增内核模块 **`crates/gateway-core/src/dist/`**（不依赖 Tauri，裸跑可测）：`autostart.rs` / `bundle.rs` / `cli.rs` / `restore.rs` / `mod.rs`；外壳侧 **`src-tauri/src/maint.rs`**（无界面动作分发）与 **`src-tauri/nsis/hooks.nsh`**（卸载钩子）。

| 交付 | 落点 |
| --- | --- |
| 开机自启开关（默认关、HKCU Run、托盘可切） | `dist::autostart`（`HkcuRun` 直写 `HKCU\...\Run`）+ `shell.rs` 托盘项「开机自启：已关闭（点一下开启）」 |
| 卸载还原客户端配置 | `dist::restore::restore_clients` = **复用** `writer::restore` / `writer::codex::CodexWriter::restore`，不另写还原逻辑；卸载器 `ExecWait` 调 `claude-gateway.exe --restore-clients --silent` |
| 导出 / 导入 | `dist::bundle`（JSON 形状见下）+ `--export` / `--import` 命令行 + `/api/export` `/api/import` |
| 设置页要的后端接口（页面归票 59） | `ingress/api.rs`：`GET/PUT /api/settings/autostart`、`POST /api/export`、`POST /api/import`、`GET /api/dist/info` |
| DPAPI 换机提示文案 | `dist::DPAPI_MIGRATION_WARNING`（导出回执与 `/api/dist/info` 都带原文） |

依赖只加了两处：`ring 0.17`（本来就在 `Cargo.lock` 里，用它的 PBKDF2 + AES-256-GCM，不手搓密码学）、`windows-sys` 的 `Win32_System_Registry` feature（读写 Run 值，避开 Tauri autostart 插件默认的 `Dynamic` 模式——以管理员运行时它会写 HKLM，而卸载器只清 HKCU 同名值）。`src-tauri` 加了 `anyhow`（与内核同一套错误类型）。

**自启三条判据**：① 默认关——不写就不写，`is_enabled` 只看注册表；② 值名固定 `Claude Gateway`（= `productName`），因为 tauri-bundler 的卸载段按这个名字 `DeleteRegValue HKCU ...\Run "${PRODUCTNAME}"`，名字一改卸载就留残留；③ 值内容 `"<exe 全路径>" --startup`（带空格路径加引号，`--startup` 让外壳直接进托盘不弹窗）。注册表是这一项**唯一真相**，不进 `settings` 表——用户勾了「Delete app data」把库删了，也不该顺手改掉自启状态。

### 卸载器怎么挂还原动作

`src-tauri/nsis/hooks.nsh`（UTF-8 **带 BOM**，否则 makensis 按 ANSI 读中文会花屏）里只做一件事：

```nsis
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1                                  ; 升级（/UPDATE）一个字节都不动（票 18 §3）
    MessageBox MB_YESNO|MB_ICONQUESTION "…还原…/保持现状…" /SD IDYES IDNO cg62_keep_clients
      ExecWait '"$INSTDIR\claude-gateway.exe" --restore-clients --silent' $0
      ${If} $0 <> 0  →  "请手动还原"（带退出码）  ${Else}  →  "已还原；Claude Desktop 需完全退出重启"  ${EndIf}
    cg62_keep_clients:
  ${EndIf}
!macroend
```

- **为什么在 PREUNINSTALL**：还原要用 `$INSTDIR` 里的 exe，而这一段的下一步就把 `$INSTDIR` 删了；POSTUNINSTALL 时程序已不在。
- **默认还原**：`MB_YESNO` 的默认按钮是「是」，再叠 `/SD IDYES` → 静默 / 被动卸载（没有界面可选）也走还原。
- **保持现状**：答「否」直接跳到 `cg62_keep_clients`，**不调用** exe；卸载段其余代码只删 `$INSTDIR` / 快捷方式 / 注册表 / 两个 identifier 目录，没有任何一行碰客户端配置。
- **数据默认保留**：用 NSIS 原生的「Delete app data」复选框（`installer.nsi` 的 `un.ConfirmShow`，`BS_AUTOCHECKBOX` 不含 `BS_CHECKED` → 默认未勾选；静默卸载跳过确认页 → `$DeleteAppDataCheckboxState=0`）。只有勾了才 `RmDir /r "$APPDATA\${BUNDLEID}"` 与 `$LOCALAPPDATA\${BUNDLEID}`，**只删这两个 identifier 目录**。
- `tauri.conf.json`：`bundle.windows.nsis.installerHooks = "nsis/hooks.nsh"`（其余字面值一字未动）。

### 导出文件格式与加密方式

明文导出（默认，**不含明文密钥**）：

```json
{ "format": "claude-gateway-export", "version": 1, "appVersion": "0.1.0",
  "exportedAt": "2026-10-15T14:29:48Z", "secrets": false,
  "data": { "upstreams": [ { "id": "openrouter", "name": "…", "baseUrl": "…",
            "authJson": "…", "pathTemplatesJson": "…", "headerPassthroughJson": "…", "quirksJson": "…",
            "credentials": [ { "wire": "anthropic", "masked": "sk-…LEAK" } ] } ],
            "upstreamModels": [ { "upstreamId": "…", "modelId": "…", "supportedWires": "…",
                                  "source": "manual", "priceInputUsd": 0.28 } ],
            "virtualModels": [ … ], "clientProfiles": [ … ], "settings": { "port": "8787" } } }
```

带口令的加密导出（`--password`，**可跨机**，与 DPAPI 无关）：

```json
{ "format": "claude-gateway-export", "version": 1, "secrets": true,
  "encrypted": { "cipher": "aes-256-gcm", "kdf": "pbkdf2-hmac-sha256", "iterations": 210000,
                 "salt": "<hex16>", "nonce": "<hex12>", "payload": "<hex(密文‖16 字节 tag)>" } }
```

- 字段全 camelCase；`version` 现在是 1，导入只拒「比本程序新」的（报错让用户先升级），`version = 0` 也拒。
- 载荷范围 = **配置**：上游表（含 `upstream_models` 的手工四项价）、虚拟模型 / 家族槽位、客户端档案、`settings`、凭据。**不含**请求日志 / body（`requests` / `request_bodies`）与拉取 / 自检的运行态时间。
- 默认导出先**摘掉密钥再序列化**：`secrets:false` 时 `credentials[].secret` 根本不出现在结构里，只留 `masked`（界面同款掩码，DPAPI 解不开时连掩码都没有）。
- `include_secrets: true` 必须配口令，否则报错（宁可报错也不把明文密钥写成明文文件）。
- 加密是 `PBKDF2-HMAC-SHA256(210000, salt)` → 32 字节密钥 → `AES-256-GCM`，密文与 tag 拼接（ring 的 `seal_in_place_append_tag` 语义），全用十六进制（不引 base64）。
- 导入写库一律 `ON CONFLICT DO UPDATE`：**不用 `INSERT OR REPLACE`**——REPLACE 是先删后插，会顺着外键把用户已有的 `credentials` / `upstream_models` 连带删掉（已写成断言）。
- 重复上游（同 id 已在库）逐条列进回执；默认**跳过**、显式 `--overwrite` 才覆盖。

### 收口验收（逐条真跑，输出为原文摘录）

**1. 自启开关实际生效（`pwsh -File %TEMP%\cg62\acc1-autostart.ps1`）**

```
=== 1. --autostart status（默认应当是 off）===            off                                              (exit 0)
=== 2. --autostart on ===
开机自启：已开启（HKCU\Software\Microsoft\Windows\CurrentVersion\Run ·
  "C:\…\cg-target\62\debug\claude-gateway.exe" --startup）                                                  (exit 0)
=== 3. reg query（真注册表）===
HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run
    Claude Gateway    REG_SZ    "C:\…\cg-target\62\debug\claude-gateway.exe" --startup                      (exit 0)
=== 4. --autostart status ===                             on + 同一命令行                                 (exit 0)
=== 5. --autostart off ===                                开机自启：已关闭                                 (exit 0)
=== 6. reg query（关闭后必须查不到 = 无残留）===           ERROR: unable to find…                   (exit 1，符合预期)
=== 7. 还原成跑之前的状态 ===                             跑之前是关着 → 保持关闭；收工后 reg query exit=1
```

跑之前机器上没有这个值，跑完也没留下（`/api/settings/autostart` 的 `PUT` 走同一段代码，`tests/dist_api.rs` 里用带进程号的临时值名做了真注册表往返，不留残留）。

**2. 卸载流程实测**

（a）**引擎级三客户端往返（临时根，`cargo test -p gateway-core --test dist`）**：`uninstall_restore_puts_all_three_clients_back_to_official_state` / `restore_leaves_unrelated_files_and_settings_alone` / `a_second_restore_is_a_no_op` 全过 —— 三个客户端先按网关写入，再还原，`claude-desktop profile` / `claude-code settings.json` / `codex config.toml` 逐字节回到写入之前，网关新建的 Codex 目录文件被整个删掉，用户自己的 `env.OTHER`、`[windows]`、别的 provider 原样保留，数据目录与密钥不动，第二次还原是空操作。

（b）**真实配置根上的实测（临时库 + 真文件，跑完无条件放回）**：`pwsh -File %TEMP%\cg62\acc2b-real-roots.ps1`

```
写入之前： desktop profile 4 字节 / cc 37 字节 / codex 2660 字节，网关接管痕迹 = []（`_meta.json` 例外：带着 prototype 时代的 9f1c2b3a… 残留）
写入之后： desktop profile 436 字节 [inferenceGatewayApiKey, inferenceModels]
           cc 365 字节 [ANTHROPIC_BASE_URL, ANTHROPIC_AUTH_TOKEN, ANTHROPIC_DEFAULT_OPUS_MODEL]
           codex config.toml 3137 字节 [claudegateway, # >>> claudegateway managed (top-level) >>>]
           codex 目录文件 5089 字节
还原回执： 已还原：claude-desktop（2 个文件）、claude-code（1 个文件）、codex（2 个文件）
还原之后： 全部长度/痕迹回到「写入之前」；codex 目录文件不存在
           desktop profile 与写入之前逐字节一致 ✓ ；cc 逐字节一致 ✓ ；codex config.toml 逐字节一致 ✓
           `_meta.json`：写之前就带着 prototype 时代残留，这次顺手清掉了（175 字节、无网关痕迹）
放回后四个文件的 sha256 与跑之前**逐字相同**（E3566B3A… / 2BC66EB2… / C0EB3361… / A70E7321…）
```

（c）**真装真卸（NSIS，静默 = 默认还原）：`pwsh -File %TEMP%\cg62\acc2c-nsis-silent.ps1`**

```
当前会话是管理员: False
装： Start-Process "…\Claude Gateway_0.1.0_x64-setup.exe" -ArgumentList '/S' -Wait → 退出码 0
     %LOCALAPPDATA%\Claude Gateway\ = claude-gateway.exe(17498624) + uninstall.exe(81919)
     HKCU\…\Uninstall\Claude Gateway → InstallLocation="C:\Users\ZHANQUAN\AppData\Local\Claude Gateway"
种： ~/.claude/settings.json 写上网关的 6 个 env 键（模拟已接到网关）
卸： Start-Process "$env:LOCALAPPDATA\Claude Gateway\uninstall.exe" -ArgumentList '/S' -Wait → 退出码 0
卸后： 程序目录 False（程序删了）
     数据目录**还在**：gateway.db 4096 / -shm 32768 / -wal 148352 / logs\gateway.log 2426（四个文件大小一字未变）
       多出来的只有引擎自己的备份与日志：backups\claude-code\settings.json.2026…、logs\writer-2026….log
     claude-code 网关键 BASE_URL=False AUTH_TOKEN=False，自有键 autoUpdatesChannel=True（留着）
     HKCU\…\Run 的 Claude Gateway 值查不到；HKCU\…\Uninstall\Claude Gateway 已消失
放回： 数据目录 + 两个客户端文件恢复原样（claude-code sha256 = C0EB3361… 与原件逐字相同）；HKCU\Software\claudegateway 根键已清
```

**3. 导出文件里反查不到明文密钥；导入回来网关可用（`pwsh -File %TEMP%\cg62\acc3-export.ps1`）**

用**独立实现**（PowerShell/.NET 的 `Rfc2898DeriveBytes` + `AesGcm`，与本程序 ring 那套互相独立）造一份带口令的加密导出并导入真实数据目录（真实现场没有上游，先备份整份数据目录、跑完放回）：

```
1. .NET 造的加密导出：pw-export.json（1882 字节，cipher=aes-256-gcm / kdf=pbkdf2-hmac-sha256 / iterations=210000）
2. --import pw-export.json --password 口令-62-验收
   → 已导入（格式 v1）：新建上游 1，覆盖 0，跳过 0，重复上游 0，模型 1，绑定 1，设置 1，密钥 1          (exit 0)
3. --export default-export.json → 1909 字节；明文密钥：没有；加密：否                              (exit 0)
4. 拿已知密钥串反查（sk-or-v1-CG62-REAL-CHECK-…-SHOULD-NOT-LEAK）：
   文件里含完整密钥串 : False
   文件里含密钥片段   : False
   文件里含掩码       : True
   secrets 标记       :   "secrets": false,
5. --export --password … --with-secrets → 2765 字节；加密导出里含完整密钥串 : False
6. 重复导入同一份 → 新建 0，跳过 1，重复上游 1（openrouter），密钥 1（重复不覆盖）
7. 起应用读 /api/status → {"version":"0.1.0","port":8787,"running":true,…,"dbPath":"…\com.claudegateway.desktop\gateway.db"}
   GET /api/settings/autostart → {"enabled":false,"valueName":"Claude Gateway",
        "runKey":"Software\\Microsoft\\Windows\\CurrentVersion\\Run",
        "commandLine":"\"C:\\…\\claude-gateway.exe\" --startup"}
8. 导入之后再导出一遍：上游还在 True / 手工价还在 True / 绑定还在 True / 明文密钥还在文件里 False
9. 恢复真实数据目录 → 「恢复后还有验收用的上游吗 : False」（机器回原状）
```

第 7 步的 `/api/status` 是**带着导入进来的配置**起的应用，所以「导入回来网关可用」这条既有人证（进程真起来了、管理面真答了）也有库证（第 8 步的上游 / 手工价 / 绑定都还在）。第 1 步那份 .NET 文件能被本程序 `--import` 读进去，等于**跨实现**验了导出格式与加密口径（同一份文件，两个独立实现互认）。

**4. 门禁（合并集成分支 tip 之后重跑）**

`pwsh -File ./check.ps1 -Quick -Replay` → **`全部通过。`**（exit 0）：

```
unit：shell 7 + core lib 136（含 dist:: 15 条）+ dist 6 + dist_api 5 + provider_table 19 + 结构化输出 14 + …
replay：22 用例全过；回放器 5 单测全过
我自己的用例在输出里逐条可见：
  dist::autostart::tests::default_is_off_and_only_an_explicit_switch_writes_a_value
  dist::autostart::tests::real_hkcu_run_value_round_trips_and_leaves_no_residue
  dist::bundle::tests::password_encryption_round_trips_and_rejects_wrong_password_or_tampering
  dist::restore::tests::{a_client_root_that_does_not_exist_is_not_a_failure, missing_roots_are_reported_as_errors_not_silent_success, configured_port_reads_settings_and_falls_back_to_default}
  dist::cli::tests::…（6 条）
  default_export_leaks_no_plaintext_secret_and_import_skips_credentials / encrypted_export_round_trips_secrets_so_the_imported_gateway_can_call_out
  import_reports_version_duplicates_and_overwrites_only_when_asked / uninstall_restore_puts_all_three_clients_back_to_official_state
  restore_leaves_unrelated_files_and_settings_alone / a_second_restore_is_a_no_op
  autostart_endpoint_reads_writes_and_clears_the_run_value / export_endpoint_writes_a_file_without_plaintext_secrets_then_import_reads_it_back
  dist_info_hands_the_settings_page_the_warning_and_the_usage / maintenance_endpoints_are_503_when_the_shell_did_not_wire_them
  shell::tests::{autostart_label_tells_the_truth_in_both_states, startup_flag_is_recognised_in_real_command_lines}
```

### 给票 59（设置页）的接口与文案

| 接口 | 请求 / 响应 |
| --- | --- |
| `GET /api/settings/autostart` | `{"enabled":bool,"valueName":"Claude Gateway","runKey":"Software\\Microsoft\\Windows\\CurrentVersion\\Run","commandLine":"\"<exe>\" --startup"}` |
| `PUT /api/settings/autostart` | body `{"enabled":true|false}` → 同上响应（写 / 删 HKCU Run 值） |
| `POST /api/export` | body `{"path":"D:\\x.json","password":null,"includeSecrets":false}` → `{"path":…,"bytes":n,"secrets":bool,"encrypted":bool,"dpapiWarning":"…"}` |
| `POST /api/import` | body `{"path":"D:\\x.json","password":null,"overwrite":false}` → 导入回执 `{format,version,appVersion,exportedAt,duplicates[],upstreamsCreated,upstreamsReplaced,upstreamsSkipped,upstreamModelsImported,virtualModelsImported,virtualModelsDropped,settingsImported,credentialsImported,credentialsSkipped}` |
| `GET /api/dist/info` | `{exportFormat,exportVersion,dpapiWarning,usage,autostartDefault:false,restoreCommand}` —— 页面上的常量格子直接从这拿，别硬编码 |

错误体与日志面**同一形状**（票 53 定的）：`{"error":{"type":"api_error","message":"人话"}}`；维护面没挂上（外壳没给库与数据目录）时一律 **503**，不是空表。

界面文案（原文，可直接显示）：

- DPAPI 换机提示（`DPAPI_MIGRATION_WARNING`，导出对话框里必须显示）：「数据库里的上游密钥是用 Windows DPAPI 按「当前用户」加密的，换一台机器（或换一个 Windows 用户）就解不开。要跨机迁移，请用「带口令的加密导出」：导出文件里的密钥用你设的口令重新加密，在新机器上凭同一口令导入即可；不带口令的导出默认**不含明文密钥**，导入后需要重新填一次密钥。」
- 自启开关：「开机自启」副标题「默认关闭。开启后写 HKCU 的 Run 键（`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，值名 `Claude Gateway`），开机时直接进托盘。」开关行文案与托盘一致：`开机自启：已关闭（点一下开启）` / `开机自启：已开启（点一下关闭）`。
- 导出选项：「不含密钥（默认，导入后需重填）」/「带口令加密（可跨机迁移密钥）」。
- 导入回执要显示：版本（`version` + `appVersion` + `exportedAt`）、重复上游清单（`duplicates`）、跳过的凭据数（`credentialsSkipped` → 「这些凭据没有跟着来，请重新填写」）。

### 给票 63（M4 收口）的人眼检查项

1. **交互式卸载**：确认页上「Delete app data」默认**未勾选**；点「卸载」后**只弹一次**询问框、默认按钮是「是（还原）」；选「是」→ 程序目录没了、客户端回官方态、数据目录还在；选「否」→ 客户端配置文件**一个字节都不动**（本次会话跑不出交互式点击，见遗留）。
2. 勾上「Delete app data」卸载 → 只少 `%APPDATA%\com.claudegateway.desktop` 与 `%LOCALAPPDATA%\Claude Gateway`，`%APPDATA%\Claude`、`~/.claude`、`~/.codex` 一个字节不少。
3. 托盘「开机自启」点开 → `reg query HKCU\...\Run /v "Claude Gateway"` 能查到；点关 → 查不到；托盘文案跟着变。
4. `--startup` 拉起（把 Run 值手动 `reg add` 成带 `--startup` 的命令行，注销再登录，或直接跑 `claude-gateway.exe --startup`）→ 只出托盘、不弹窗口。

### 遗留与坑

- **交互式卸载点击没跑成**：本会话没有可交互桌面（`Start-Process uninstall.exe` 起来后窗口标题为空、`AppActivate`/`SendKeys` 打不进去），所以「选保持现状」这半只有**代码事实 + 引擎级字节证据**（钩子里答「否」直接跳过 `ExecWait`，卸载段其余代码不碰客户端配置），真点一次留给票 63。静默卸载那条走的是同一条 `ExecWait`（`/SD IDYES`），已真跑。
- **`--startup` 的窗口闪一下**：隐藏主窗口发生在 Tauri `setup` 里（窗口在此之前已按配置创建）。要彻底不闪得把 `visible` 改成 `false` 再在普通启动时 show，那会动到正常启动路径，没做。
- **端口仍读 `config.json`**：`shell.rs::load_port` 那笔 shortcut（`settings` 表里已有 `port`）还没销账，归票 59 的设置页；`dist::restore::configured_port` 读的是 `settings` 表，两者目前都指向 8787。
- **导出不含请求日志**：`requests` / `request_bodies` 不在导出范围（那是“日志与用量”，不是配置）；诊断包归票 59。
- **导入的设置里含 `port`**：跨机导入会把端口一起带过去，需要重启才生效——设置页（票 59）该在导入回执里提示。
- **`ring` 与 `anyhow` 是新增的直接依赖**（都在 `Cargo.lock` 里，没扩依赖面）；`windows-sys` 多开了一个 `Win32_System_Registry` feature。
- **验收期间真实机器状态已全部还原**：`%APPDATA%\com.claudegateway.desktop`（含 WAL 字节数）、`~/.claude/settings.json`、`~/.codex/config.toml`、`%LOCALAPPDATA%\Claude-3p\configLibrary\*.json` 四个文件 sha256 与跑之前**逐字相同**；`HKCU\...\Run` 无 `Claude Gateway` 值；`HKCU\Software\claudegateway` 已删；安装目录不存在；没有跑着的 gateway 进程、8787 空闲。`~/.codex` 里 `config.toml.bak-claudegateway`（票 48 验收留的）没动。
- **并行票会抢运行资源**：本次门禁第一次 `replay` 失败（回显里多出一条请求、协议头缺失）是并行的别票在跑同一套固定端口（`tests/common/mod.rs` 用 `19000 + seed*2`），重跑即过；同一时间另一个 cargo 进程还把 `debug\mock-upstream.exe` 占住过一阵（`Access denied`），删掉重编即恢复。这些都不是本票改动引起。

