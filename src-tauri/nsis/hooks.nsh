; 票 62（自启动、卸载与导出）的卸载钩子：删程序目录**之前**问一次要不要还原客户端配置。
;
; 为什么必须在这里（PREUNINSTALL）：还原动作就是调 `$INSTDIR\claude-gateway.exe --restore-clients
; --silent`（复用配置写入引擎的摘键还原，票 45/46/47/48），而下一步 $INSTDIR 里的 exe 就被删了；
; POSTUNINSTALL 那时已经没有可调的程序。卸载器只删自己那两个 identifier 目录
; （`$APPDATA\${BUNDLEID}` / `$LOCALAPPDATA\${BUNDLEID}`，由确认页原生的「Delete app data」决定，
; 默认不勾 = 保留数据），`%APPDATA%\Claude`、`~/.claude`、`~/.codex` 一个字节都不碰。
;
; 默认答案 = 还原：`MB_YESNO` 的默认按钮是「是」，再叠一个 `/SD IDYES`，静默 / 被动卸载
; （没有界面可选）也走「还原」。升级（`/UPDATE`）整段跳过——票 18 §3 的判据是
; 「升级前后不重写任何客户端配置」。

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    MessageBox MB_YESNO|MB_ICONQUESTION \
      "卸载前要把三个客户端的配置还原成官方状态吗？$\n$\nRestore the three clients (Claude Desktop / Claude Code / Codex) to their official configuration before uninstalling?$\n$\n默认：还原（点「是」/ Yes）" \
      /SD IDYES IDNO cg62_keep_clients

    ExecWait '"$INSTDIR\claude-gateway.exe" --restore-clients --silent' $0
    ${If} $0 <> 0
      MessageBox MB_OK|MB_ICONEXCLAMATION \
        "客户端配置没能自动还原（退出码 $0）。请手动把 Claude Desktop / Claude Code / Codex 切回官方配置。$\n$\nClient configuration could not be restored automatically. Please restore it manually." \
        /SD IDOK
    ${Else}
      MessageBox MB_OK|MB_ICONINFORMATION \
        "已把三个客户端的配置还原成官方状态。Claude Desktop 需要完全退出后重新打开才会读到官方配置。$\n$\nClients restored. Restart Claude Desktop completely for it to take effect." \
        /SD IDOK
    ${EndIf}

    cg62_keep_clients:
  ${EndIf}
!macroend
