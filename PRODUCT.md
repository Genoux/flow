# Product

<!-- impeccable:product-schema 1 -->

## Platform

linux

## Users

Linux / Wayland users (wlroots: Hyprland, Sway) who want to dictate into whatever already has focus, without creating an account or sending audio anywhere.

They hold a chord, speak, and release. They are not looking for a recorder, a notes app, or a second window to paste from.

## Product Purpose

Hold a key, talk, let go. The text appears where the cursor already was.

Success is words landing in the focused field — punctuated, in the language spoken — without switching windows, paying per use, or waiting until the whole take is over.

## Positioning

Push-to-talk dictation that runs entirely on the user's machine. Speech recognition and refining are local. There is no account, no API key, no per-word cost, and nothing leaves the computer.

The daily interface is a held key and a small island that appears while they speak. A neighbouring product that needs a window, a cloud key, or a toggle cannot truthfully copy that.

Linux-only. No second platform is in the product.

## Operating Context

Two surfaces, one product:

- The **island** — a Wayland overlay that appears only while recording. This is the daily interface.
- The **console** (`flow-console`, launcher name **Flow**) — Overview, History, Dictation, Audio, Vocabulary, Models, About. Settings, history and vocabulary live here. The daemon never carries this window.

On a machine with no speech model the console opens on **setup** instead of the rail: one centred page that downloads both models and starts the daemon at the end. It names the card refining will use in a single quiet line and says nothing else about hardware — a person installing a dictation tool is not shopping for an inference backend. About has a Run setup control that re-enters the same screen to verify the models and repair anything missing. The installer script no longer fetches the models; the window does, so a 3 GB download happens somewhere it can be watched and skipped rather than in a terminal.

The daemon is the systemd user unit `flow.service`. Dictation is hold-to-talk: default chord `super+shift+d`, watched directly from evdev. A compositor bind calling `flow start` / `flow stop` is a second path, not the only one. Release stops; this is never a toggle.

Audio follows the system default input (PipeWire or ALSA). Other apps duck while recording. Config is optional and lives at `~/.config/flow/config.toml`; vocabulary is `~/.config/flow/vocabulary.txt`, one term per line, applied by the refining model. History is a local transcript log.

The console can be laid out on a machine that cannot run the daemon (`FLOW_CONSOLE_DEMO`). That is a design aid, not a second product.

## Capabilities and Constraints

**Does:**

- Transcribe on hold and inject at the cursor, with the paste chord chosen per injection from whatever window has focus.
- Refine punctuation and filler locally; skip the model for short, already-punctuated, filler-free text.
- Preserve language: the detected language is named in the refining prompt, and a translated refining is discarded so the raw transcript survives.
- Transcribe long takes in pieces during the hold, cutting only inside genuine silence.
- Show recent transcripts, a daily/weekly overview, vocabulary edits, model install, and daemon start/restart.
- Set itself up on first run: download both models with real progress, name the card it picked, let the optional refining model be skipped, and start the daemon when it finishes. Re-runnable from About.
- Warn (not die) if native PTT cannot read `/dev/input`; the signal path still works.

**Must not lose:**

- Fully local: no API, no account, no per-use cost. Install auto-selects local models.
- Hold-to-talk, including fast taps. Never a toggle that can get stuck. `MIN_HOLD` applies to every binding.
- Linux / Wayland only. No port, no second platform in the record.
- Two binaries: iced/wgpu stay in the console. The daemon records audio and must not take that tree.
- Capture follows the OS default input and nothing else. Choosing the source is the desktop's job.
- STT stays on CPU so the GPU is free for refining. Model choices are measured; do not swap them without rerunning the numbers.
- Language preservation is enforced in code, not only requested in a prompt.
- Native PTT failing is a warning, not a fatal error.

**Terminology:** island (the overlay), console (the window), refining (the local punctuation pass), chord / push-to-talk, duck, vocabulary, daemon.

**Undecided:** accessibility standard — none confirmed.

## Brand Commitments

Name: **Flow**. MIT.

Voice is the product's own copy: short, concrete, spoken as sentences. The one-line claim is "Push-to-talk dictation that runs entirely on your own machine." The one-line use is "Hold a key, talk, let go."

No separate logo, wordmark, or brand kit is on hand. The island and the console must read as one product.

## Evidence on Hand

- README and TROUBLESHOOTING at the repo root; config keys in `packaging/config.template.toml`.
- Island implementation: `src/overlay.rs`.
- Console: `crates/console/` (iced). Demo states via `FLOW_CONSOLE_DEMO`.
- Desktop entry: `packaging/flow-console.desktop`.

No testimonials, customers, benchmarks for marketing, or press. Future work must not invent them. Model speed and hardware notes in the repo are engineering measurements, not claims to quote as social proof.

## Product Principles

1. The cursor is the interface. Dictation must not ask for a window.
2. Nothing leaves the machine.
3. Hold means hold. A toggle is a bug.
4. The island and the console are one product.
5. Linux / Wayland is the product, not a first port.
