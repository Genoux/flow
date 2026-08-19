# Troubleshooting

Three commands answer most of it:

```bash
flow logs          # what the daemon has been saying
flow retry         # re-run your last dictation: raw, denoised, cleaned
flow logs -f       # watch it live while you reproduce the problem
```

`flow retry` is the one people forget — and the one that needs turning on
before it can help. Set `record_debug = true` in `~/.config/flow/config.toml`
and Flow keeps each dictation's audio, so you can see exactly what the
recogniser heard and what refining did to it rather than guessing from the text
that landed. It is off by default because the files add up.

For more detail than the journal normally carries, run the daemon by hand:

```bash
systemctl --user stop flow.service
FLOW_DEBUG=1 flow daemon
```

That turns on device enumeration, chord internals and llama.cpp's own output,
all of which are silenced by default so `flow logs` shows your dictations
instead of a tensor dump.

## Nothing happens when I hold the key

| Cause | Check | Fix |
|---|---|---|
| Daemon isn't running | `systemctl --user status flow.service` | `systemctl --user start flow.service` |
| Not in the `input` group | `flow logs` says *push-to-talk disabled* | `sudo usermod -aG input $USER`, then log out and back in |
| Another app owns the chord | Your compositor's binds | Change `hotkey` in `config.toml` |
| Held under half a second | `flow logs` says *too short to be a deliberate hold* | Hold longer — the 500 ms floor is what stops a frustrated double-tap injecting a stray "Yeah." |
| Another key was pressed during the hold | *another key turned the hold into a shortcut* | Expected. Flow assumes you meant the shortcut. |

## It recorded, but no text arrived

**Your text is almost certainly on the clipboard.** Press Ctrl+V. Flow stages
the transcript on the clipboard *before* it tries to paste, so every failure
after that point is recoverable.

| Cause | Check | Fix |
|---|---|---|
| A modifier was still held | *A held key swallowed the paste* | Let go of the chord fully before the text is ready, or just press Ctrl+V |
| Terminal ignores the clipboard | Paste works elsewhere but not here | `terminal = true` in `config.toml` — types key by key instead |
| `/dev/uinput` not writable | `flow logs` shows an inject error | Re-run the udev step printed by `packaging/install.sh` |

## It heard nothing, or the wrong thing

| Symptom | Cause | Fix |
|---|---|---|
| *Flow heard nothing* | The system default input is not the microphone you spoke into, or it is muted | Set the default source in your sound settings. Flow follows the system default and nothing else — that is deliberate. |
| *Flow lost the microphone* | The device was unplugged or a Bluetooth headset dropped | Reconnect it, then `systemctl --user restart flow.service` |
| Silence after a mic reconnect | The capture stream was torn down and rebuilt | Handled automatically. If it persists, restart the service. |
| Names come out wrong | The recogniser has no idea what your project is called | Add the terms to `~/.config/flow/vocabulary.txt`, one per line |
| Vocabulary changes nothing | Vocabulary is applied by the refining model | It has no effect with `refine = false` or `--raw` |

## The text is unpunctuated

Refining is off or its model is missing. `flow logs` says *refining disabled* at
startup. Run `flow install`.

Short utterances are skipped on purpose — "Yeah." needs no refining and running
the model on it would only add latency.

## It's slow

| Symptom | Cause | Fix |
|---|---|---|
| ~13 s once, then fine | Vulkan recompiled its pipelines after the GPU choice changed | Expected. Don't chase it. |
| Consistently 4–9 s | Refining landed on an integrated GPU | Set `gpu = <index>` in `config.toml`. `flow logs` prints the card it chose. |
| Slower than the log claims | Refining exceeded its deadline and the raw transcript shipped | Working as designed — a rougher sentence beats a nine-second wait |

Check where the time actually goes rather than guessing: every dictation logs
its own breakdown.

```
12.5s peak 0.985 rms 0.0581 -> 500ms stt, 501ms clean, 74ms paste, 1.07s total
```

## It doesn't start with my session

The unit is `WantedBy=graphical-session.target`. Desktops that never activate
that target will never start it, and `systemctl --user enable` will look like
it did nothing.

Check first:

```bash
systemctl --user is-active graphical-session.target
```

If that says `inactive`, start `flow.service` from your compositor's own
autostart instead of enabling the unit.

## I dictated French and got English

That is a bug, not a setting — the detected language is named in the refining
prompt and a translated result is discarded so the raw transcript survives. If
it still happens, `flow retry` shows which stage did it, and that output is
worth an issue.
