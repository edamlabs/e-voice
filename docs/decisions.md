# Decisions

### 2026-10-26 · The project moves to EDamLabs

**Decision.** `damvolkov/e-voice` is transferred to `edamlabs/e-voice`; documentation moves to
`edamlabs.github.io/e-voice`, the images to `ghcr.io/edamlabs/` from the next release.

**Evidence.** EDamLabs gathers the eager series, the bindings and the services under one organisation, one GitHub
App and one maintenance engine (esteward); the release workflow already names the images after the repository owner.

**Rejected.** Keeping the images under `damvolkov` (a second home for the same artefacts).

### 2026-10-26 · Identity in Cargo.toml

**Decision.** `[workspace.metadata.edamlabs]` declares `kind = "service"` and `family = "eager"`; esteward's
`drift` module checks it against the fleet.

**Evidence.** The repository ships Docker images (no PyPI package) and is one of EDamLabs' own e- products. It is a
Cargo workspace with no root `[package]`, so the table goes under `[workspace.metadata]`, which esteward reads as
well; Cargo ignores `metadata` tables.

**Rejected.** `[package.metadata.edamlabs]` in one member crate (the identity is the repository's, not a crate's);
a `pyproject.toml` only for the declaration (`eval/` is a benchmark tool, not the product).

### 2026-10-26 · Images without a dash: evoice-stt, evoice-tts, evoice-full

**Decision.** The published images become `evoice-stt`, `evoice-tts` and `evoice-full`; the repository keeps its
dash (`e-voice`).

**Evidence.** EDamLabs names import names, binaries, services and images without a dash. The move already changes
every image path (`ghcr.io/damvolkov/…` → `ghcr.io/edamlabs/…`), so a consumer edits the reference once either way:
the rename breaks nothing the move does not.

**Rejected.** Keeping `e-voice-*` (a second rename later, a second break); renaming the crates, binaries and Docker
volumes in the same change (a separate contract: the volumes hold installed models).

### 2026-10-26 · Renovate replaces Dependabot; routine updates merge themselves

**Decision.** `.github/renovate.json` replaces `.github/dependabot.yml`. Images, the toolchain with its cargo-chef
image, actions, lock files, the benchmark's Python tooling and Rust patch releases merge themselves once every
required check passes; Rust minor and major updates wait for a human.

**Evidence.** The Rust tree is the service runtime: CI's hooks job (clippy, unit, property and integration tests)
and the image builds gate every PR, which is enough for a patch, not for an API change. Actions and images stay
pinned by digest. Renovate groups, schedules and automerges in one file, and is what the rest of EDamLabs runs.

**Rejected.** Dependabot (no automerge policy of its own, a second tool across the organisation); a human merging
every green routine PR.
