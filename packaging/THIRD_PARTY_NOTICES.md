# Third-Party Notices

Codex + CC-Panes Companion is distributed under the MIT license.

The executable contains Rust dependencies recorded in `DEPENDENCIES.json` and locked by
`Cargo.lock`. Each dependency's exact name, version, declared license and source are included
in the generated manifest. The redistributable package also includes the corresponding
license texts under `THIRD_PARTY_LICENSES/`, with an auditable `MANIFEST.json` mapping each
external crate to the files shipped for it. Package verification fails if a dependency has
no declared license or no license text.

This package does not redistribute Codex App, Codex CLI or CC-Panes. Those remain separate
official installations governed by their respective licenses.
