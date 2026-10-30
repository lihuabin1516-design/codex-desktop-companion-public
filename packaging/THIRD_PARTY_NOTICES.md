# Third-Party Notices

Codex + PaneForge Companion is distributed under the MIT license.

The executable contains Rust dependencies recorded in `DEPENDENCIES.json` and locked by
`Cargo.lock`. Each dependency's exact name, version, declared license and source are included
in the generated manifest. The redistributable package also includes the corresponding
license texts under `THIRD_PARTY_LICENSES/`, with an auditable `MANIFEST.json` mapping each
external crate to the files shipped for it. Package verification fails if a dependency has
no declared license or no license text. If a crate archive omits its license file, the
package builder includes the matching SPDX canonical text from the pinned
`spdx/license-list-data` v3.27.0 snapshot and records that provenance in the manifest.

This package does not redistribute Codex App, Codex CLI or PaneForge. Those remain separate
official installations governed by their respective licenses.
