# Start Here

## For users

This repository currently provides source code only; it does not yet publish an installable release. External Windows-machine acceptance is still pending. Read the project status in the root `README.md` before building or relying on beta-era reports.

## For contributors

Start from the repository root and inspect the current branch and working tree before making changes:

```powershell
git status --short --branch
git log -1 --oneline
```

Install the prerequisites listed in [`CONTRIBUTING.md`](../CONTRIBUTING.md). Keep Rust build output outside the checkout:

```powershell
$env:CARGO_TARGET_DIR = Join-Path $env:TEMP "codex-desktop-companion-target"
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
```

## Reference documents

- [`AUTHORITY.md`](AUTHORITY.md): product scope and safety boundaries.
- [`ARCHITECTURE.md`](ARCHITECTURE.md): code and runtime architecture.
- [`OPERATIONS.md`](OPERATIONS.md): user-facing diagnosis and recovery.
- [`COMPATIBILITY.md`](COMPATIBILITY.md): host integration assumptions.
- [`RELEASE.md`](RELEASE.md): build and release checks; not evidence that a release has shipped.
- [`EXTERNAL_WINDOWS_ACCEPTANCE.md`](EXTERNAL_WINDOWS_ACCEPTANCE.md): pending external-machine acceptance contract.
- [`coordination-plane/README.md`](coordination-plane/README.md): implementation status for the coordination components.
