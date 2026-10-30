# Codex + CC-Panes Companion 0.1.0-beta.3

独立仓库首个可验收 Windows 测试版。

## 主要能力

- 从 Codex Desktop 环境安全启动官方 CC-Panes；
- 精确清理任务级 Codex 私有变量并保留正常用户配置；
- 自动发现并适配官方 CC-Panes 更新；
- 提供 probe、doctor、构建目录审计和配置迁移；
- 提供版本化用户安装、开始菜单入口、回退基础和精确卸载；
- 提供 SHA-256 全包覆盖、依赖清单及篡改拒绝。

## 本版本变化

- 从 LiveAgent 子目录迁移为独立仓库；
- CI、构建和发布路径改为仓库根；
- 增加维护入口、交接、安全、贡献和依赖更新策略；
- 保持 beta.2 的运行行为和兼容边界。

## 实机兼容

- Codex CLI：`0.146.0`
- CC-Panes：`0.11.10`、`0.11.11`
- Windows：x64

## 完整性

Release 附件包括：

- `CodexCCPanes-0.1.0-beta.3-win-x64.zip`
- `CodexCCPanes-0.1.0-beta.3-win-x64.zip.sha256`
- artifact JSON

本测试版二进制状态为 unsigned。公开生产发布以 Authenticode 门禁通过为准。
