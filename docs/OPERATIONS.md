# 运行与故障处理

## 推荐启动

安装后从开始菜单打开 **Codex + PaneForge**。该快捷方式指向 Companion GUI launcher，
由它启动当前官方 PaneForge。

Codex App 可以在 Companion 之前或之后启动，两者没有启动顺序要求。

## 健康检查

```powershell
codex-paneforge doctor
codex-paneforge doctor --json --output "$HOME\Desktop\codex-paneforge-doctor.json"
```

状态：

- `pass`：必要依赖与边界正常；
- `warn`：可以继续使用，但存在任务级环境、遗留进程或版本信息缺口；
- `fail`：官方 PaneForge 未发现或核心探测失败。

## 高 CPU / 磁盘增长

Companion 本身不运行 `cargo`、`rustc`、Tauri dev server 或 Workflow runner。

只读扫描：

```powershell
codex-paneforge audit-builds "$env:TEMP" --threshold-gb 5
```

该命令只报告目录，不执行清理。

## 配置迁移

Companion 默认不需要配置。只有官方安装位于非标准路径，或需要从旧 launcher
迁移显式路径时才执行：

```powershell
# 只预览，不写入
codex-paneforge migrate-config --from .\legacy.json --json

# 写入前会记录原文件大小和 SHA-256；已有配置自动备份
codex-paneforge migrate-config --from .\legacy.json --apply
```

支持旧 JSON 字段 `paneforgeExecutable`、`paneforgeExe`、
`officialPaneForgeExecutable`、`target`，也支持只包含路径的文本文件和
`LIVEAGENT_PANEFORGE_EXE` 环境变量。

迁移只写入 `%LOCALAPPDATA%\CodexPaneForge\config.json`，不修改 Codex 或
PaneForge 配置。

## 回退

Companion 失效时直接从开始菜单启动官方 PaneForge，Codex App 和已有数据不受影响。
安装器保留上一版本目录；重新运行上一版本安装包即可切回。
