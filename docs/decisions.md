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

### 2026-10-26 · The benchmark pins its Python to 3.14

**Decision.** `eval/.python-version` holds `3.14`: uv runs the benchmark, its hooks and CI on the latest 3.14, never
on whatever Python is newest.

**Evidence.** `requires-python = ">=3.13"` let CI's uv pick CPython 3.15.1 the day it shipped (2026-10-21); under it
polars 1.44.2 returns `None` from `Series.drop_nulls()` and `pytest (eval)` failed on `main` from that day on.
Reproduced in a clean container with managed Pythons only: 3.15.1 fails, 3.14.9 and 3.13.5 pass.

**Rejected.** polars 2.1.0 (passes on 3.15, but a major bump the benchmark's tests do not cover end to end; Renovate
proposes it on its own terms); capping `requires-python` (the floor is a contract, the pin is the runtime).

### 2026-11-18 · The services bound memory per request; the proxy bounds the number of requests

**Decision.** No number a client sends sizes an allocation in stt or tts: sample rate (384 kHz), decoded duration
of an upload (`[stt.api] duration`, 4 h by default), WebSocket message size (2 MiB on the audio sockets, 16 MiB on
OpenAI Realtime, 64 KiB on the TTS text socket), TTS text (4096 characters). The number of connections a client
opens, rate limiting and authentication are not the services' concern: they belong in front of them.

**Evidence.** A rate of `4294967295` reserved about 2.3 GB and aborted the process; a 7 MB FLAC of ten hours of
silence decoded to 2.3 GB, now 10 MiB at peak; a legal 16 MiB frame cost the native socket 126 MB, a legal 2 MiB
frame now costs 11.6 MiB. A 61-min meeting still goes through whole at the default (438 segments, none failed).
The default of 4 h costs up to 922 MB of decoded audio per upload: the price of letting long meetings through.

**Rejected.** A fixed duration cap in code (a deployment's choice, and a low one would break long uploads); a
connection limit inside the services (a second, weaker copy of what the proxy does); completing the WebSocket
closing handshake after an oversized frame by upgrading below axum (a large change for a misbehaving client's
courtesy — the close 1009 is sent and the server lingers one second instead).
