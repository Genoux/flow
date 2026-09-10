# Flow

Hold a key, talk, let go. The text appears where your cursor already was.

Flow is a dictation app for Linux. Speech recognition runs locally; refining uses
the selected OpenRouter model. There is no window to
focus and no button to press: the only interface is a key you hold and a small
island that appears while you speak.

## Requirements

| | |
|---|---|
| Session | Wayland (wlroots — Hyprland, Sway) |
| Audio | PipeWire or ALSA |
| Network | Only for refining, and not even that at `cleanup = "none"` |
| Account | An [OpenRouter](https://openrouter.ai) key, for the cleanup levels above `none` |
| Access | Your user in the `input` group, and `/dev/uinput` writable |

## Install

```bash
git clone https://github.com/Genoux/flow
cd flow && ./packaging/install.sh
```

Or download a release tarball, unpack it, and run the same `packaging/install.sh`
from inside — it uses the binaries it finds there instead of building them.

That builds both binaries into `~/.local/bin` and installs the systemd user unit
and the desktop entry. Nothing is written outside your home directory, and
nothing runs as root — except one udev rule, which the script prints for you to
run yourself rather than doing behind your back.

Then open **Flow** from your launcher, or `flow-console` from a terminal. The first
launch fetches the speech model itself — there is a progress screen and nothing to
type — and Nemotron then runs on-device for every dictation. If a model file ever
goes missing or turns up damaged, the window notices at launch and offers **Repair**
on the same screen; no terminal is needed for either case, though `flow install`
does the same fetch if you would rather run it yourself. Cleanup above
`cleanup = "none"` needs an OpenRouter key, pasted into **Settings → OpenRouter**:
that is what Gemini 3.1 Flash-Lite is reached through.

Then hold **Super+Shift+D** and talk.

Overview says **Connected** once refining has reached OpenRouter, and
**Disconnected** when one could not — a daemon that is up with a dead network or
a rejected key still transcribes, but a cleanup level above `none` will not run.

Updating is the same script — `git pull && ./packaging/install.sh` — which
restarts the daemon onto the new build if it was already running.

There are two release channels. **Stable** is for everyday use. **Experimental**
lets you try upcoming changes before they reach stable and may be less reliable.
Enable **Settings → Build → Experimental build** to opt in, or disable it to return
to stable. Flow downloads and verifies the selected release and restarts to apply
it. Both builds, your models, settings and history stay on disk. Updates follow
the selected channel. A release installer refuses to install under the wrong channel.

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
| `flow probe` | Whether OpenRouter answers, and which model refining would use |
| `flow install` | Re-fetch any speech-model file that is missing or fails its hash |
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

One model on-device, one through OpenRouter:

- **Nemotron 3.5 ASR streaming multilingual 0.6B** turns audio into text, on the
  CPU. `flow install` fetches it once (~2.6GB, ONNX, licensed OpenMDW-1.1 — the
  weights and origin notices travel with any redistribution). Audio is transcribed
  during recording in 560 ms chunks, preserving context across chunks. Release
  finishes the remaining audio. Optional denoising uses whole-recording inference;
  it is off by default.
- **Gemini 3.1 Flash-Lite** punctuates and removes filler, above `cleanup =
  "none"`. It is told the language it just heard, and a result that comes back
  in a different language is discarded, so speaking French gets French back.

`cleanup = "none"` is a local passthrough: nothing is sent anywhere, and no
OpenRouter key is needed to use it. Expect rough text from it — the recogniser
punctuates short utterances but not long ones, and leaves every "um" and
repeated word where it was. Every level above it is one refining
request per dictation, guarded the same way regardless of which channel built
the binary.

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
