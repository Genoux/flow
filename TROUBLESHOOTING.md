# Troubleshooting

Three commands answer most of it:

```bash
offhand logs          # what the daemon has been saying
offhand retry         # re-run your last dictation: raw, cleaned
offhand logs -f       # watch it live while you reproduce the problem
```

`offhand retry` is the one people forget — and the one that needs turning on
before it can help. Set `record_debug = true` in `~/.config/offhand/config.toml`
and Offhand keeps each dictation's audio, so you can see exactly what the
recogniser heard and what refining did to it rather than guessing from the text
that landed. It is off by default because the files add up.

For more detail than the journal normally carries, run the daemon by hand:

```bash
systemctl --user stop offhand.service
OFFHAND_DEBUG=1 offhand daemon
```

That turns on device enumeration, chord internals and llama.cpp's own output,
all of which are silenced by default so `offhand logs` shows your dictations
instead of a tensor dump.

## Nothing happens when I hold the key

| Cause | Check | Fix |
|---|---|---|
| Daemon isn't running | `systemctl --user status offhand.service` | `systemctl --user start offhand.service` |
| Not in the `input` group | `offhand logs` says *chord watcher disabled* | `sudo usermod -aG input $USER`, then log out and back in |
| Another app owns the chord | Your compositor's binds | Change `hotkey` in `config.toml` |
| Held under half a second | `offhand logs` says *too short to be a deliberate hold* | Hold longer — the 500 ms floor is what stops a frustrated double-tap injecting a stray "Yeah." |
| Another key was pressed during the hold | *another key turned the hold into a shortcut* | Expected. Offhand assumes you meant the shortcut. |

## It recorded, but no text arrived

**Your text is almost certainly on the clipboard.** Press Ctrl+V. Offhand stages
the transcript on the clipboard *before* it tries to paste, so every failure
after that point is recoverable.

| Cause | Check | Fix |
|---|---|---|
| A modifier was still held | *A held key swallowed the paste* | Let go of the chord fully before the text is ready, or just press Ctrl+V |
| Offhand does not recognise your terminal | Paste works elsewhere but not here | Offhand asks the compositor what has focus and sends Ctrl+Shift+V to a terminal. Add your terminal's window class to `TERMINALS` in `src/inject.rs`; `hyprctl activewindow -j` prints it |
| `/dev/uinput` not writable | `offhand logs` shows an inject error | Re-run the udev step printed by `packaging/install.sh` |

## It heard nothing, or the wrong thing

| Symptom | Cause | Fix |
|---|---|---|
| *Offhand heard nothing* | The system default input is not the microphone you spoke into, or it is muted | Set the default source in your sound settings. Offhand follows the system default and nothing else — that is deliberate. |
| *Offhand lost the microphone* | The device was unplugged or a Bluetooth headset dropped | Reconnect it, then `systemctl --user restart offhand.service` |
| Silence after a mic reconnect | The capture stream was torn down and rebuilt | Handled automatically. If it persists, restart the service. |
| Names come out wrong | The recogniser has no idea what your project is called | Add the terms to `~/.config/offhand/vocabulary.txt`, one per line |
| Vocabulary changes nothing | Vocabulary is applied by the refining model | It has no effect with `refine = false` or `--raw` |

## The text is unpunctuated

Refining is off or its model is missing. `offhand logs` says *refining disabled* at
startup. Run `offhand install`.

Short utterances are skipped on purpose — "Yeah." needs no refining and running
the model on it would only add latency.

## It's slow

| Symptom | Cause | Fix |
|---|---|---|
| ~13 s once, then fine | Vulkan recompiled its pipelines after the GPU choice changed | Expected. Don't chase it. |
| Consistently 4–9 s | Refining landed on an integrated GPU | Set `gpu = <index>` in `config.toml`. `offhand logs` prints the card it chose. |
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

If that says `inactive`, start `offhand.service` from your compositor's own
autostart instead of enabling the unit.

## I dictated French and got English

That is a bug, not a setting — the detected language is named in the refining
prompt and a translated result is discarded so the raw transcript survives. If
it still happens, `offhand retry` shows which stage did it, and that output is
worth an issue.
