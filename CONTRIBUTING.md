# Contributing

## 开发环境

- Windows 11；
- PowerShell 7；
- Rust 1.88+ stable MSVC toolchain；
- `rustfmt` 与 `clippy`；
- Git。

推荐统一构建缓存：

```powershell
$env:CARGO_TARGET_DIR = Join-Path $env:TEMP "codex-desktop-companion-target"
$env:RELEASE_OUTPUT_ROOT = Join-Path $env:TEMP "codex-desktop-companion-release"
```

## 本地门禁

```powershell
cargo fmt --all -- --check
cargo check --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
.\scripts\build-release.ps1 -CargoTargetDir $env:CARGO_TARGET_DIR -OutputRoot $env:RELEASE_OUTPUT_ROOT
# Use the packageDirectory reported by build-release.ps1 with verify-package.ps1 and test-installer.ps1.
git diff --check
```

## 修改边界

- 使用合成 child 测试启动边界；
- 真实 PaneForge 只执行只读 probe 和 doctor；
- 保持官方 EXE、数据库、配置和项目 Session 原样；
- 新增依赖时同步审查许可证和 `Cargo.lock`；
- 发布行为遵循 `docs/RELEASE.md`。
