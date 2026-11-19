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

### 2026-11-18 · One daemon owns the microphone and the speaker

**Decision.** `evoiced` is the only process that opens the audio devices; `ecli say|stop|listen|talk|voice`, Claude
Code (MCP, hooks) and any script call it. It listens on loopback only, whatever `[server] host` says. «Sole owner» is a
documented convention, not a lock.

**Evidence.** Two clients opening the same device would race for the microphone and talk over each other on the
speaker; the daemon serialises speech (16 deep, never more than 2 s of audio ahead of the device), leases the
microphone to one taker at a time (a second answers 409) and has one stop for every door. PipeWire lets any process
open the same device, so a lock would be unenforceable. A `/speak` or `/listen` on a network would hand out the
host's speaker and microphone: hence loopback and an explicit `[daemon] host`.

**Rejected.** Each client opening the device itself (what `ecli stt` and `ecli tts` still do as testers); a lock file or
exclusive ALSA open (PipeWire defeats it); folding the daemon into the services (it must run on the host, beside
the audio devices, while the services may run in a container).

### 2026-11-18 · Diarization on sherpa-onnx's own pipeline, speakers on uploaded files only

**Decision.** The diarization node is sherpa-onnx's offline pipeline (pyannote segmentation 3.0, a speaker
embedding, clustering), off by default, run once over a whole uploaded file beside the transcription. Streaming
responses leave the speaker empty.

**Evidence.** On the crate already pinned: DER 0.330 on eight VoxConverse files with 9 to 12 speakers, RTF 0.09–0.11
on 8 CPU threads (about 6 minutes per hour), reproduced to 0.001 per file by our own test; the 61-minute meeting goes
through whole. No new runtime, no Python, no model outside sherpa, ort or Rust.

**Rejected.** pyannote community-1 and Canary v2 (not in sherpa, ort or Rust: re-weighed in `TODO_vehicle` step 5);
diart (abandoned since 2025-02, no path into sherpa); live clustering (an incremental clusterer over sherpa's
segmentation and embeddings, if a need appears).

### 2026-11-18 · The embedding default stays WeSpeaker ResNet34-LM

**Decision.** `[stt.pipeline.diarize] embedding` defaults to `wespeaker-resnet34-lm`, fp32, threshold 0.5; CAM++ and
ERes2Net are selectable.

**Evidence.** [The embedding bench](history/2026-11-diarization.md): on 12 held-out VoxConverse files CAM++ and
ERes2Net halve the error (+0.194 and +0.232, intervals clear of zero) at their own tuned threshold near 0.9, and the
int8 segmentation shows no difference. Yet on a real 61-minute Spanish meeting every tuned setting, the default
included, failed the 6–20 speaker gate: no setting is validated for Spanish meetings, and nothing measured justifies
moving the default.

**Rejected.** Making the English-broadcast winner the default (thresholds tuned on VoxConverse over-split the real
meeting); int8 (faster by 6 %, not shown no worse). Revisit with a Spanish reference (`TODO_vehicle` step 4).

### 2026-11-19 · Native Ubuntu packages, built with dpkg-deb, Qwen3 by default

**Decision.** Every release that includes `full` publishes a `.deb` and a tarball for Linux x86_64 with the four
binaries, the sherpa-onnx and ONNX Runtime libraries in `/usr/lib/evoice/lib`, the manifests and three systemd user
units, built on Ubuntu 22.04. Everything the installation writes lives under `~/.config/evoice/`. The package ships a
default config selecting Qwen3, copied on first start and never overwritten.

**Evidence.** The binaries already run from `bin/` with an `$ORIGIN/../lib` rpath, so the package adds only files:
the image's own additions are two manifests, data paths and a launcher. The code default, Pocket, has
`file://data/tts/ops/exports/…` sources (`evoice/tts/models.toml`) and needs a learned voice, so its `pull` fails on a
clean machine; Qwen3's weights are public (`pull` fetches only `qwen3-tts`, 3.6 GB), as the Docker images already
choose. Building on 22.04 sets the glibc floor for 22.04 and later. The units run as the user, because the daemon
needs the user's audio session.

**Rejected.** cargo-deb (one crate per package; ours bundles four binaries from three crates) and nfpm (a new tool for
what `dpkg-deb`, present on every Ubuntu runner, already does); publishing Pocket's exported bundles (a second
distribution to maintain, from a gated checkpoint); system-wide units running as a service user (no audio session);
an apt repository (a signed index to run, for a download that `apt install ./x.deb` already serves).
