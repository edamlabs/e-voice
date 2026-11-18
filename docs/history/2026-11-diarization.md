# 2026-11 · Diarization: the speaker-embedding bench

What was measured when speaker diarization entered the STT service (2026-11-18), on sherpa-onnx 1.13.8's offline
diarization, CPU only. The protocol — arms, material, metric and the threshold of practical difference — was
written before the first run; two amendments were written before the runs they governed.

## Question

Does another speaker-embedding model, or the int8 segmentation, beat the node's default on diarization error?

| Arm | Embedding | Segmentation |
|---|---|---|
| baseline | WeSpeaker ResNet34-LM | pyannote 3.0, fp32 |
| A | 3D-Speaker CAM++ zh_en advanced | fp32 |
| B | 3D-Speaker ERes2Net en_voxceleb | fp32 |
| C | WeSpeaker ResNet34-LM | int8 |

## Method

- **Material.** 20 VoxConverse files: 8 to tune each arm's cluster threshold, 12 held out for the verdict. A
  model's distances have their own scale, so each arm is tuned; comparing all at one threshold would measure the
  threshold.
- **Metric.** Diarization error rate per file, `pyannote.metrics` 4.2.0, collar 0, overlap scored. Arms are paired
  by file; the difference is a paired bootstrap (BCa, 95 %, 10 000 resamples). A difference counts above 0.02.
- **Runs.** 3 replicas × 4 arms × 12 files, one process at a time. The node is deterministic — the three replicas
  of every arm gave identical speaker turns — so the replicas served the timing only.

## Thresholds, tuned on the 8 files

| Arm | 0.3 | 0.4 | 0.5 | 0.6 | 0.7 | 0.8 | 0.9 | 1.0 |
|---|---|---|---|---|---|---|---|---|
| baseline | 0.344 | **0.289** | 0.330 | 0.333 | 0.349 | 0.391 | | |
| A | 0.412 | 0.318 | 0.237 | 0.188 | 0.173 | 0.170 | **0.161** | 0.238 |
| B | 0.489 | 0.371 | 0.315 | 0.203 | 0.181 | 0.167 | **0.144** | 0.155 |
| C | 0.345 | 0.316 | **0.282** | 0.329 | 0.361 | 0.434 | | |

The grid ended at 0.8; A and B were still falling there, so it was extended for them until the error rose.

## Result on the 12 held-out files

| Arm | Threshold | DER | Baseline − arm (95 %) | Verdict |
|---|---|---|---|---|
| baseline | 0.4 | 0.373 | | |
| A | 0.9 | 0.179 | +0.194 (0.105 to 0.268) | better |
| B | 0.9 | 0.141 | +0.232 (0.153 to 0.301) | better |
| C | 0.5 | 0.361 | +0.012 (−0.063 to 0.067) | no difference |

Arm C runs about 6 % faster (wall ratio 0.938, interval 0.917 to 0.967; most runs on a loaded machine), but its
error is not shown to be no worse: fp32 stays.

## Why the default did not change

VoxConverse is English broadcast speech; the service's use is Spanish meetings of 9 to 13 speakers, for which no
reference transcript with speakers exists yet. Before making a winner the default, each tuned arm diarized a real
61-min Spanish meeting once, and a count outside 6 to 20 speakers would veto it:

| Arm | Threshold | Speakers found | Share of the largest |
|---|---|---|---|
| baseline | 0.4 | 44 | 20.6 % |
| A | 0.9 | 35 | 22.9 % |
| B | 0.9 | 49 | 22.2 % |
| the default, untuned | 0.5 | 24 | 52.1 % |

All fail, the untuned default included. At the tuned thresholds the four or five largest speakers hold about 70 %
of the speech and the tail is fragments of under a second: thresholds tuned on VoxConverse over-split this
meeting. At 0.5 the default leaves a tail of fragments too, and one speaker holds 52 % of the speech where the
other settings give the largest 21 to 23 %: either one person spoke for half the meeting or the default merges
voices the others keep apart — without a reference this cannot be told. An earlier run of the same meeting through
the whole service labelled its transcript with 12 speakers; that counts the speakers that win a transcribed
segment, a different thing from the speakers of the turns.

**Verdict.** No setting is validated on Spanish meetings, the default included. The default stays WeSpeaker
ResNet34-LM, fp32, threshold 0.5, because nothing measured here justifies moving it. CAM++ and ERes2Net are
selectable (`[stt.pipeline.diarize] embedding`), and on English broadcast material they halve the error — with
their own threshold near 0.9. Which model serves Spanish meetings, and at what threshold, waits for a Spanish
reference.

## Limits

- 12 held-out files; one language; one Spanish meeting, judged by a speaker count, not by an error rate.
- The speaker count is a blunt gate: it can veto a threshold, it cannot rank models.
- Timing was taken on a shared machine; only the int8 ratio is reported, with that caveat.
