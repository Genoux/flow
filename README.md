# Flow

Hold a key, talk, let go. The text appears where your cursor already was.

Flow is a voice dictation daemon for Linux. Speech recognition and refining use
the selected OpenRouter models. There is no window to focus and no button to
press: the only interface is a key you hold and a small island that appears
while you speak.

## Requirements

| | |
|---|---|
| Session | Wayland (wlroots — Hyprland, Sway) |
| Audio | PipeWire or ALSA |
| Network | Required. Transcription and refining are OpenRouter requests |
| Account | An [OpenRouter](https://openrouter.ai) key, which you pay per dictation |
| Access | Your user in the `input` group, and `/dev/uinput` writable |

## Install

```bash
git clone https://github.com/Genoux/flow && cd flow && ./packaging/install.sh
```

Or download a release tarball, unpack it, and run the same `packaging/install.sh`
from inside — it uses the binaries it finds there instead of building them.

That builds both binaries into `~/.local/bin` and installs the systemd user unit
and the desktop entry. Nothing is written outside your home directory, and
nothing runs as root — except one udev rule, which the script prints for you to
run yourself rather than doing behind your back.

Then open **Flow** from your launcher, or `flow-console` from a terminal, and
paste an OpenRouter key into **Settings → OpenRouter**. Nothing dictates without
one: MAI-Transcribe-2 and Gemini 3.1 Flash-Lite are reached through that key.

Then hold **Super+Shift+D** and talk.

Overview says **Connected** once a dictation has reached OpenRouter, and
**Disconnected** when one could not — a daemon that is up with a dead network or
a rejected key is running and useless, so the word says which.

Updating is the same script — `git pull && ./packaging/install.sh` — which
restarts the daemon onto the new build if it was already running.

Two builds can be installed side by side — `./packaging/install.sh --channel
experimental` puts one in without touching the stable binary. A symlink decides
which one runs, and **Settings → Build** repoints it for the next restart. The
experimental channel is the opt-in release lane for changes that need feedback;
stable remains available for daily use.

Removing it is `./packaging/uninstall.sh`. That leaves your config and history
alone, and prints how to delete those if you want them gone.

## Daily use

| | |
|---|---|
| `Super+Shift+D` (hold) | Dictate. Release to paste. |
| **Flow** in your launcher | Settings, history and vocabulary in a window |
| `flow-console` | The same window, from a terminal |
| `flow logs` | What the daemon has been saying |
| `flow retry [n]` | Re-run a saved dictation through the pipeline (needs `record_debug`) |
| `flow start` / `flow stop` | Trigger dictation without the chord, for a compositor bind |
| `flow probe` | Whether OpenRouter answers, and which models it would use |
| `flow help` | Every command and flag |

## Configuration

Everything lives in `~/.config/flow/config.toml`, and the file is optional —
every key has a working default. `packaging/config.template.toml` documents all
of them. The ones people actually change:

```toml
hotkey = "super+shift+d"   # the combination to hold
duck = 50                  # volume of other apps while recording, in percent
cleanup = "light"          # none, light or medium
openrouter_key = "sk-or-…" # easier to paste in Settings than to type here
```

The key is a billable credential. Saving it from the window writes the file
`0600`; if you put it there by hand, do the same.

Word fixes go next door in `~/.config/flow/vocabulary.txt` — one term per line,
for names the recogniser mishears. Note that vocabulary is applied *by the
refining model*, so it does nothing at `cleanup = "none"`.

## How it works

Two models, both through OpenRouter:

- **MAI-Transcribe-2** turns audio into text.
- **Gemini 3.1 Flash-Lite** punctuates and removes filler. It is told the
  language it just heard, and a result that comes back in a different language
  is discarded, so speaking French gets French back.

Both choices are fixed in the current release: MAI-Transcribe-2 handles speech
and Gemini 3.1 Flash-Lite applies the selected cleanup level. The prompt, cleanup
levels and guards around the model's answer are shared by stable and experimental
builds; the experimental channel is for future product changes, not a silent
model change in a stable install.

A dictation longer than 45 seconds is split before it is sent, cut inside real
silence rather than at a stopwatch, because the provider times out at 60
seconds of processing per request. A stretch of speech with no pause in it is
sent whole: an oversized request that may fail beats a transcript with a word
sliced in half.

Refining is bounded. Past its budget the raw transcript is pasted instead of a
late one, and every guard that made the local refiner safe still runs on the
reply — an answer instead of an edit, a question turned into a statement, a
dictation that lost most of its words, or a translation, all fall back to what
you actually said.

## When something goes wrong

Start with [TROUBLESHOOTING.md](TROUBLESHOOTING.md). The short version:

```bash
flow logs                      # the last 50 lines
flow retry                     # what it heard, denoised, and refined
FLOW_DEBUG=1 flow daemon       # the chatty version, run in a terminal
```

`flow retry` needs `record_debug = true` in your config — it replays saved
audio, and Flow keeps none by default.

## Building without the installer

```bash
cargo build --release                                    # the daemon
cargo build --release --manifest-path crates/console/Cargo.toml   # the window
cargo test --workspace --all-targets
```

The console is a separate workspace on purpose: it pulls in iced and wgpu, and
the daemon has no business carrying those to record audio.

Build dependencies: `libasound2-dev`, `libvulkan-dev`, `glslang-tools`,
`libwayland-dev`, `libclang-dev`, `cmake`, `pkg-config`.

## Licence

MIT — see [LICENSE](LICENSE).
