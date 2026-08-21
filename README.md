# Flow

Hold a key, talk, let go. The text appears where your cursor already was.

Flow is a voice dictation daemon for Linux. Speech recognition and refining both
run on your machine — no account, no API key, no per-word cost, nothing leaves
the computer. There is no window to focus and no button to press: the only
interface is a key you hold and a small island that appears while you speak.

## Requirements

| | |
|---|---|
| Session | Wayland (wlroots — Hyprland, Sway) |
| Audio | PipeWire or ALSA |
| Disk | ~3 GB for the two models |
| GPU | Optional. Vulkan is used for refining if a card can hold the model, CPU otherwise |
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

The first build takes 10–15 minutes: llama.cpp is compiled from source.

Then open **Flow** from your launcher, or `flow-console` from a terminal. The
first run is a setup screen: it downloads the two models and starts the daemon
at the end. They are about 3 GB together and Flow needs both, so there is
nothing to choose — but you can stop the download and pick it up later from
where it left off, and **Run setup again** on the About screen refetches them
from scratch if one ever goes bad.

Then hold **Super+Shift+D** and talk.

Scripted installs that would rather not wait for a window can fetch the models
up front with `./packaging/install.sh --models`, or `flow install` at any time.

Updating is the same script — `git pull && ./packaging/install.sh` — which
restarts the daemon onto the new build if it was already running.

Removing it is `./packaging/uninstall.sh`. That leaves your config, history and
the models alone, and prints how to delete those if you want them gone.

## Daily use

| | |
|---|---|
| `Super+Shift+D` (hold) | Dictate. Release to paste. |
| **Flow** in your launcher | Settings, history and vocabulary in a window |
| `flow-console` | The same window, from a terminal |
| `flow logs` | What the daemon has been saying |
| `flow retry [n]` | Re-run a saved dictation through the pipeline (needs `record_debug`) |
| `flow start` / `flow stop` | Trigger dictation without the chord, for a compositor bind |
| `flow probe` | Which GPU refining will run on, and why |
| `flow help` | Every command and flag |

## Configuration

Everything lives in `~/.config/flow/config.toml`, and the file is optional —
every key has a working default. `packaging/config.template.toml` documents all
of them. The ones people actually change:

```toml
hotkey = "super+shift+d"   # the combination to hold
duck = 50                  # volume of other apps while recording, in percent
refine = true              # run the transcript through the local refining model
terminal = false           # type key by key instead of pasting
```

Word fixes go next door in `~/.config/flow/vocabulary.txt` — one term per line,
for names the recogniser mishears. Note that vocabulary is applied *by the
refining model*, so it does nothing when `refine = false`.

## How it works

Two models, both local:

- **Parakeet TDT 0.6B v3** (int8 ONNX, CPU) turns audio into text at roughly
  23× realtime. Running it on the CPU is deliberate — it keeps the GPU free.
- **Qwen3 4B Instruct** (Q4_K_M via llama.cpp, Vulkan) punctuates and removes
  filler. It is told the language it just heard, and a result that comes back
  in a different language is discarded, so speaking French gets French back.

Long dictations are transcribed in pieces *during* the hold, split only inside
real silence, so releasing the key does not start a long wait.

Both model choices are measured rather than assumed. If you swap them, rerun
the numbers.

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
