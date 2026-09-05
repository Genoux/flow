# Application review — 5 September 2026

## Result

The review fixes concrete rendering, responsiveness, persistence, privacy, and calendar defects. Changes remain uncommitted. This is a source review and local validation, not a guarantee that every platform or hardware path is bug-free.

## Rendering and responsiveness

- Preallocate all three banner images through iced's renderer and retain their allocations for the window lifetime. Initial page reveal waits for allocation results, including when navigation happens during startup. Failures are logged and do not leave the page hidden forever.
- Use the existing 200 ms fade for page changes, preserve fade progress during rapid navigation, and remove the covering layer once settled. Navigation does not slide or alter layout.
- Cache banner corner geometry. Retaining all three decoded images costs approximately 20.4 MiB before renderer overhead.
- Run microphone discovery, history refresh, autostart changes, file opening, and setup-completion queries outside the UI thread. Coalesce overlapping refresh requests.
- Serialize settings saves in background tasks and coalesce intermediate slider values. Closing waits for the latest save; a failed save keeps the window open so the error remains available.
- Drain subprocess output while commands run, preventing large PipeWire listings from filling a pipe and timing out.
- Correct the activity calendar's first visible Sunday so dates align with weekday labels.

## Security and persistence

- Replace the shared temporary runtime fallback with a private per-user directory. Reject an existing fallback directory with unsafe ownership, permissions, or a symlink.
- Reject PID values at or below 1 before signalling, preventing process-group or broad signal delivery from malformed PID files.
- Create history and debug WAV files with mode 0600 and tighten existing files when next written. History trimming preserves private permissions. Existing recordings are not bulk-modified.
- Save settings and vocabulary through a shared atomic writer: exclusive private temporary file, sync, rename, directory sync. Preserve existing configuration symlinks and propagate read failures instead of overwriting unreadable content.
- Inspected process calls use argument arrays rather than shell interpolation. Model downloads retain pinned size and SHA256 validation before installation.

## Verification

- Daemon and shared-path tests: 211 passed, 22 ignored.
- Console tests: 71 passed, 2 ignored.
- Both workspaces pass clippy with all targets and warnings denied, formatting checks, and git diff whitespace checks.
- Actual local STT and refining fixture tests pass.
- Isolated X11 preview exercises every page, including first and repeated banner visits. Captured early and settled frames show the photograph present during the fade. No banner allocation error appears in the preview log.
- A settled three-second preview sample records zero process CPU ticks. This is a short idle observation, not a sustained performance benchmark.
- Visual checks use Xvfb without accelerated presentation; they do not establish frame-rate guarantees on physical Wayland displays.

## Remaining limitations

Cargo audit scans both lockfiles against RustSec database commit `5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5` (updated 2 September 2026). It reports zero advisories classified as vulnerabilities, but these informational warnings remain:

- `paste` 1.0.15 and `ttf-parser` 0.25.1 are unmaintained in both dependency trees.
- Console dependency `cryoglyph` 0.1.0 requires `lru` 0.16, affected by [RUSTSEC-2026-0253](https://rustsec.org/advisories/RUSTSEC-2026-0253.html). The published issue requires a panicking key destructor during `pop()`. The inspected renderer uses copyable `cosmic_text::CacheKey` keys with no destructor and does not call `pop()`; no matching trigger was found in this dependency path. The warning remains pending a compatible upstream upgrade; it is not suppressed.

Positive stale PID reuse can still signal an unrelated same-user process; the PID validation does not prove process identity. Startup retains two bounded 250 ms probes for stable initial layout, and occasional local vocabulary saves remain synchronous. Ignored tests cover explicit live hardware, input injection, network, and diagnostic scenarios. RustSec does not comprehensively audit bundled native inference libraries.

The development console is rebuilt for review. These source fixes do not replace or restart the installed dictation daemon.
