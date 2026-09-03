# AGENTS.md

Notes on why parts of Flow are built the way they are. Everything here was
measured or debugged rather than assumed, and most of it is guarding a
mistake that has already been made once.

Flow is a Rust local dictation daemon. Dictation is hold-to-talk: recording
starts the instant the binding is pressed and stops on release, including
fast taps, never a toggle that can get stuck. Flow watches the physical
chord itself and stops on release, because Hyprland release binds drop
modifier chords.

Speech-to-text and refining stay fully local, with no API and no per-use
cost. `flow install` auto-selects a model for the machine's hardware.

## Inference

- Inference is on-device: Parakeet TDT 0.6B v3 int8 ONNX for STT (CPU, ~23x realtime) and Qwen3 4B Instruct Q4_K_M via llama.cpp (Vulkan) for refining, under `~/.local/share/flow/models/`. `flow install` fetches both, pinned to commit + sha256 in `src/install.rs`.
- Both model choices are measured, not assumed — don't swap them without rerunning the numbers. STT on CPU is what keeps the GPU free for refining. 4B is a deliberate floor: refining's failure mode is paraphrasing instead of punctuating, which is instruction-following, and commit `03085c6` shows this model already straining at it.
- Language preservation is enforced, not requested. `refine::language` (whatlang, reliable detections only) names the detected language in the system prompt for that input, and `changed_language` treats a translated refining as a failure so the raw transcript survives. The prompt alone failed twice (03085c6 and again after) — code-switched speech is what tips the model. Do not "simplify" this back to a prompt rule.
- GPU choice is `refine::choose_device`: discrete before integrated, then most free memory, filtered by whether the model fits. Ranking by free memory alone is WRONG — an iGPU reports shared system RAM (16.9 GB vs 4.4 GB free on the 3060 Ti), so it wins on paper and loses in practice. `tests/device.rs` pins that topology as a regression case.
- Changing the llama.cpp device config triggers a one-time ~13s Vulkan pipeline recompile on the next start; it returns to ~135ms after. Not a regression, don't chase it.
- `refine::needs_refining` skips the model for short, already-punctuated, filler-free text (~20% of real dictations are "Yeah."/"Mm-hmm."). It is deliberately biased towards running the model: a needless pass costs ms, a wrongly skipped one ships unpunctuated text.
- Long dictations are transcribed in pieces during the hold (`audio::split_at_silence` + `Capture::take_prefix`, driven from `daemon`). Only ever cuts inside genuine silence — `tests/chunking.rs` proves a between-sentence cut gives identical words, and continuous speech gets no cut and the old behaviour. Ordering rule: take the engine lock BEFORE the audio and push the piece while still holding it, or the tail can be read before an in-flight piece lands. Any abandoned recording clears `early` (see `begin`).
- Measured before building it: STT was 45% of the wait on dictations ≥10s with a healthy GPU (51s dictation = 2.2s of 3.9s). Refining's own cost is token generation and irreducible; `new_context` is only 17ms, so there is nothing left to win there.

## Audio capture

- Two different device-change cases, do not conflate them. (1) The default source *changes* while both sources exist: PipeWire relinks the live stream by itself, no code needed — verified by watching `pw-link -l` move a recording from the webcam to `phone_mic:capture_MONO` mid-capture. (2) The linked source is *destroyed* (mic unplugged, Bluetooth headset drops): PipeWire tears the stream down, **cpal reports no error**, and the callback simply never fires again. Every later recording returns the stale 0.2s pre-roll and is skipped as silence, forever. `Capture::ensure_live()` handles only case 2, called from `begin()`.
- Dead-stream detection is "no callback for 3s", not an error callback, because there is no error. Measured headroom: a healthy idle capture's worst gap is 64ms over 8s, so ~47x. Reopen takes ~15ms. `tests/capture_health.rs` measures both, and would catch a threshold that fires on a working mic.
- On reopen the stream is rebuilt but `live`/`samples`/`pre_roll` are deliberately the same `Arc`s, so the overlay's Monitor and any in-flight recording survive the swap. The new stream is installed only after `play()` succeeds, so a failed rebuild leaves the old one in place.
- Capture follows the system default input and nothing else: `open_device()` is just `default_input_device()`. Do not reintroduce a preferred-device match — `tests/audio.rs` fails if you do. `Capture::attach` hardcodes 16kHz mono f32 and only gets away with it because ALSA `default` routes through PipeWire, which converts from the hardware's real 48kHz; a raw `hw:` PCM makes `build_input_stream` fail outright. That is why choosing a mic is NOT a device lookup.
- Picking a microphone is per-app routing instead: `pactl move-source-output` moves Flow's own live stream (`Capture::set_source`, config key `input_device`), leaving `pactl get-default-source` untouched. No cpal changes, no second audio path, and PipeWire's stream-restore remembers it. Applied per dictation from `daemon::begin`, and re-applied after `ensure_live` — a rebuilt stream is a new source-output id, so the routing dies with the old one.
- The stream is found by the **running binary's name**, read from `current_exe()`, because the ALSA plugin publishes `alsa_capture.<binary>` and no `application.process.id` to match on instead (pipewire-pulse clients beside it in the same listing do publish one, which is misleading). Hardcoding `"flow"` silently does nothing under any other name — caught because the test binary is not called flow, so the feature appeared to work while moving no stream at all. `tests/routing.rs` (ignored) drives the real graph and would catch it again.
- Wake the source you are actually on, not `@DEFAULT_SOURCE@`. A pinned mic is by definition not the default, so waking the default leaves the pinned one suspended and it opens with a burst of zeros — the same "hold and nothing" failure as the phone mic. `Capture::wake_current` is used by both `warmup` and `begin_inner`.
- Unpinned costs nothing: with nothing pinned and nothing previously routed, `apply_source` returns before it shells out at all, because PipeWire already follows the default by itself. Only "was pinned, now Automatic" has to name the default explicitly.
- The console filters `*.monitor` out of the picker — 8 of this machine's 13 sources are monitors of output devices, which record playback rather than speech. If the *system default* is a monitor, Flow still captures playback, and that remains the user's choice to make rather than a bug to work around.

## Overlay

- The island maps on the keypress, at the same moment as the duck, with no delay on show. A delay was once added there by mistake while fixing a complaint about the *hide*; press-to-visible is the one thing that must never wait.
- The sweep is armed by knowledge, not by a clock. `overlay.working()` is called only once recognition has produced words, so silence, nothing-recognised and hesitation-only hide without ever drawing a spinner. Timing was tried first and disproved: a cough on 7s of silence came back "nothing recognised" in 280ms, past the 200ms `SWEEP_DELAY`, and flashed the sweep. `SWEEP_DELAY` now only holds a spinner back from work that finishes quickly.
- A recording starting while a sweep is pending cancels it, so it cannot appear over the fresh bars. The bars also fall to rest before the sweep starts, or the island jumps between the two shapes.

## Hotkey

- Push-to-talk is a native chord, default `super+shift+d`, configurable via `hotkey = ...`. Flow watches it directly, so a compositor bind (via SIGUSR1/2) is a redundant second path, not the only one. The default was once Right Ctrl, which is a bad universal default: Apple and compact boards often have no right-hand modifiers at all.
- Native PTT failing (no readable `/dev/input`, user not in `input` group) is a warning, not a fatal error: the SIGUSR1/2 signal path is independent, so the daemon keeps running and a compositor bind still dictates. `ptt` is set to `None` so the ready line does not advertise a chord that is not being watched.
- Reading evdev is passive and does NOT consume the event. A compositor bind on the same chord consumes it; Flow alone does not, so with no compositor bind the chord also reaches the focused window. That is the only behavioural difference between the two paths.
- `MIN_HOLD` (500ms) applies to EVERY binding. Exempting chords let a frustrated double-tap inject whatever the recogniser made of 200ms of pre-roll — that is where stray "Yeah."/"Mm." came from. Measured gap: stray taps held ≤400ms, real dictations ≥2.2s. `Chord::deliberate()` now only decides whether a stray key cancels.
- evdev letter codes follow QWERTY rows, NOT the alphabet (`KEY_A`=30, `KEY_S`=31, `KEY_D`=32), and F11/F12 sit apart from F1–F10. Both directions go through the `LETTERS`/`FUNCTION_KEYS` tables in `hotkey.rs`; never do arithmetic from `KEY_A`. A round-trip test caught exactly this bug.
- `keyboards()` probes for `KEY_A`, not a modifier — testing for `KEY_RIGHTCTRL` would skip boards that lack it.
- `tests/chord_live.rs` (ignored) creates its own uinput keyboard and presses itself, which is the only check that the reader, the keycodes and the chord all agree. Stop `flow.service` first or it dictates into the focused window.

## Text injection

- Modifier state comes from EVENTS, not `get_key_state`. `EVIOCGKEY` goes stale: a Keychron board reports LALT+LSHIFT held with nothing pressed and keyd mirrors it as LMETA+LSHIFT — two thirds of the chord — so device state answered "held" forever and every paste timed out with the text left on the clipboard. Typing still worked, which proves the compositor disagreed, so the devices were simply the wrong oracle. `observed()` starts empty each start; device state is the fallback only when nothing is watching events (`WATCHING == false`).
- Injection is timed in the log line on purpose. It was once the biggest term (~500ms) because `wait_for_modifiers_released` re-probed every input device's capabilities per paste.

## Console

- `crates/console` is a separate binary from the daemon on purpose: iced brings wgpu with it and the daemon has no business carrying that to record audio. It talks over the status socket the daemon already publishes and otherwise reads the same files the daemon uses.
- First run fetches both models by shelling out to `flow install --porcelain` (sizes from `--plan`), never a second copy of the download logic — the pinned revisions and sha256s in `install.rs` are the reason an install can be trusted, and a second downloader is a second place for them to be wrong. Leaving the cleanup model out and offering it later was worse: it put a 2.4 GB decision in front of someone who had not dictated a word yet.
- The layout must never shift. A status line appearing, a step finishing, or a control going away has to leave everything else where it was — reserve the space instead. This has been reported repeatedly and is the single most common complaint about the window.
- Transitions fade, they do not slide, and slow reads better than accurate: a progress bar that jumps to the true percentage reads as a hiccup, so it is walked there. `theme::FADE` (200ms) is the shared unit; anything appearing without one looks like a glitch.

## Runtime

- The tray is its own `flow-tray.service`, started from `default.target`, not a
  child of `flow.service`. It must remain available while dictation is stopped:
  that is when its Open and Start controls are most useful. `show_tray = false`
  unregisters only the StatusNotifierItem; the lightweight controller keeps
  watching the config so the icon can return live. Opening `flow-console` with
  a complete install attempts to start the dictation daemon by default.
- Runs as the systemd user unit `flow.service`. On compositors that never activate `graphical-session.target` the unit's `WantedBy=` never fires, so it stays `disabled` and is started from the compositor's own autostart instead.
- Config is three layers: `Config::default()`, then `~/.config/flow/config.toml`, then CLI flags. Absent file is normal; a broken one is fatal by design. Keep `flow.service` flag-free so the config file is the single place behaviour lives.
- The git history carries no agent attribution: no `Co-authored-by:` trailers, no tool signatures in messages, no leftover agent branches. Agent tooling directories (`.claude`, `.cursor`, `.serena`) stay untracked. This has had to be rewritten out of the history more than once — do not reintroduce it in a single commit.
