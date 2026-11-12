# Rules

The standard every project follows, whatever its stack. Agents read this file
through the global `~/.claude/CLAUDE.md` import (`jb install`), so a project
never has to restate it. A project's own `AGENTS.md` may add rules; a deliberate
exception is one line `R<id>: reason` in the project's `.jb-exceptions`
(`R3` covers `R3.2`), which `jb check` reports instead of failing. An
exception without a reason is itself a problem.

Rules are numbered so a review or a commit can cite them (`R4.2`).

Enforcement lives in git hooks, `scripts/check` and CI, which work with any
agent. Agent hooks (`jb guard`, `jb stop`, `jb session`) are optional
adapters; every agent reaches these rules through the project's `AGENTS.md`,
which points to `.toolkit/RULES.md`.

## R0. The agent runs the toolkit

The owner pitches and decides; the agent does every toolkit step. Never ask
the owner to run `jb` or any procedure below: run it yourself and report.
`jb` is on PATH (`~/.local/bin/jb`); `jb help` lists its commands.

1. **New project.** When the owner pitches or names something new to build,
   follow the `jb-new-project` skill (it runs `jb new`).
2. **Every session in a project.** The SessionStart hook runs `jb session`
   and puts any departure from these rules in your context. Fix them as part
   of the session, on a branch: `jb sync` re-applies managed files; the rest
   (`scripts/check`, docs, workflows) you edit by hand. Run `jb check` again
   to confirm, and tell the owner in one line what changed.
3. **Project not on the standard yet.** Offer once to adopt it. On yes,
   follow the `jb-adopt` skill.
4. **Release.** When the owner asks to release or ship, follow the
   `jb-release` skill.
5. **Showing something.** Follow the `jb-demo` skill (R2).
6. **Before saying work is done.** Run `scripts/check` and report its result.
7. **Commits** go through the hooks; never bypass them (`--no-verify`). If a
   hook refuses, fix the cause. The agent hooks `jb guard` and `jb stop`
   enforce this and R0.6.

## R1. Working with the owner

1. Commit only when the owner asks, or when the step is part of a toolkit
   procedure the owner started (a new project, an adoption, a release).
   Push only when the owner asks; asking for a release counts.
2. Ask before anything leaves this machine: publishing, deploying, opening a
   pull request, creating a repo, posting to any service.
3. When a choice is the owner's (product, money, security, data, anything
   hard to reverse), ask. Otherwise pick the sensible default, say which, and go.
4. Report what was actually run and what it showed. A check that was skipped
   is reported as skipped.
5. The `branch-pr` and `issue-creation` skills (Gentle AI's own contribution
   rules: an approved issue, type labels) do not apply in the owner's
   projects; R3 governs branches, commits and pull requests.

## R2. Demos, previews, prototypes: local only

1. Never publish work as a claude.ai Artifact, Claude Doc, Gist, pastebin,
   Vercel preview or any account-bound page. The owner switches between
   several Claude accounts and one of them is shared with other people.
   In a toolkit project the Artifact and Claude Docs tools are denied in
   `.claude/settings.json`; a denial is the rule working, not a problem to
   route around.
2. A throwaway demo is a self-contained HTML file in the project's `.demo/`
   folder (gitignored): `.demo/<yyyy-mm-dd>-<slug>.html`. Inline the CSS and
   JS; no build step. Open it with `open .demo/<file>` or serve the folder with
   `python3 -m http.server -d .demo 4173`.
3. A prototype worth keeping goes in `prototype/` and is committed, with a
   line in `AGENTS.md` saying what it shows.
4. Screenshots for review go in `.demo/shots/` (headless Chrome or the
   project's own preview command).

## R3. Git

1. `main` is always releasable and only moves through pull requests.
   `.githooks/pre-commit` (or the project's hook manager, which `jb sync`
   leaves in charge) refuses a commit on `main` (the first commit of a new
   repo excepted).
2. Branches: `<type>/<short-kebab-slug>`, type one of `feat fix chore docs
   refactor perf test ci build release experiment`. Example:
   `feat/hands-free-dictation`. `.githooks/pre-push` enforces it.
3. Commits are [Conventional Commits](https://www.conventionalcommits.org):
   `<type>(<optional scope>): <subject>`.
   - Types: `feat fix chore docs refactor perf test ci build style revert`.
   - Subject: lower case, no trailing period, at most 72 characters, says what
     the change does for the user or the code, in plain words:
     `fix: give a tap time to become a double-tap`.
   - Breaking change: `feat!: …` plus a `BREAKING CHANGE:` line in the body.
   - The body (optional) says why, not what.
   `.githooks/commit-msg` enforces the header.
4. No agent attribution, ever: no `Co-authored-by:` trailers, no "Generated
   with" lines, no tool signatures, no agent-named branches. Agent tooling
   folders (`.claude`, `.cursor`, `.codex`, `.serena`, `.engram`, `.atl`) stay
   untracked. The hooks refuse both.
5. Merge by rebase: history stays linear and each commit message is the
   record. Delete the branch after merging.
6. One concern per pull request. A pull request passes CI before it merges.
7. Never rewrite `main`'s published history without the owner asking.

## R4. Versions and releases

1. [SemVer](https://semver.org). `0.y.z` while the product is finding its
   shape. Pre-releases: `X.Y.Z-alpha.N`, `-beta.N`, `-rc.N`, `-experimental.N`.
2. The version lives in the stack's manifest (`Cargo.toml`, `package.json`,
   `pyproject.toml`) or, for projects without one, a `VERSION` file.
   `.toolkit/version` reads it and fails if two manifests disagree.
3. Release flow (the agent runs it, `jb-release` skill):
   1. On a branch: `jb bump X.Y.Z` (commits `chore: bump version to X.Y.Z`).
   2. Merge the pull request once CI passes.
   3. On an up-to-date `main`: `jb tag` (creates annotated tag `vX.Y.Z`),
      then `git push origin vX.Y.Z`.
   4. `.github/workflows/release.yml` refuses a tag that disagrees with the
      manifest, runs `scripts/check`, runs `scripts/build` if present, and
      drafts a GitHub release with generated notes and everything in `dist/`.
      A service (R8) also publishes its images (`image.yml`).
   5. Publishing the draft is the owner's decision.
4. Release notes come from the commit subjects, which is why R3.3 matters.

## R5. Project layout

Every project has:

| Path | Holds |
|---|---|
| `README.md` | What it is, how to run it, how to check it. For a stranger. |
| `AGENTS.md` | Why it is built the way it is: stack and why, layout, decisions with what was measured or the mistake each guards, project-specific rules. For the next agent. |
| `CLAUDE.md` | `@AGENTS.md` only. One source of truth for every agent. |
| `PRODUCT.md` | Who it is for, what it must do, what it must not. Written before code. |
| `scripts/check` | The one command that says the project is healthy: format, lint, types, tests. CI runs exactly this. |
| `scripts/build` | Optional. Produces release files in `dist/`. |
| `compose.yaml` | Services only: the deploy file (R8). |
| `.toolkit/` | Managed by `jb sync`. Do not edit. |
| `.githooks/` | Managed by `jb sync`; unused when the project has its own hook manager. Do not edit. |
| `.jb-exceptions` | Optional. Deliberate departures from these rules, `R<id>: reason` per line. |
| `.demo/` | Throwaway demos (R2). Gitignored. |

Optional, when the project has them: `DESIGN.md` (UI system),
`TROUBLESHOOTING.md` (user-facing fixes), `docs/adr/YYYY-MM-DD-<slug>.md`
(decisions too big for a bullet in `AGENTS.md`), `prototype/`.

## R6. Stack and code

1. The stack is chosen per project and recorded in `AGENTS.md` › Stack with
   the reason. The toolkit does not pick frameworks or UI libraries.
2. Pin toolchains (`rust-toolchain.toml`, `.tool-versions`, `packageManager`)
   and commit the lockfile. Add a dependency only when it earns its place;
   say why in the commit body.
3. Strict mode on: TypeScript `strict`, Rust `clippy -D warnings`, linters at
   error. Formatting is automatic and checked in `scripts/check`.
4. Tests sit next to what they test or in the stack's usual place; a bug fix
   comes with the test that would have caught it, when one can be written.
5. Secrets live in `.env` (gitignored) or the OS keychain. Commit
   `.env.example` with names and no values.
6. Record measurements, not beliefs: when a decision rests on a number (speed,
   size, accuracy), `AGENTS.md` keeps the number, the date and how it was taken.

## R7. Hosting

1. Repos are private on the owner's personal GitHub account unless the owner
   says otherwise. Never an organisation account without being asked. An
   app (R9) is public, because installed copies fetch its releases without
   credentials; making a repo public is still the owner's go-ahead (R1.2).
2. Merge settings: rebase only, delete branch on merge (`jb new --github`
   sets these; for an existing repo, `gh repo edit --enable-rebase-merge
   --enable-squash-merge=false --enable-merge-commit=false --delete-branch-on-merge`).
3. Shared work owned by another of the owner's projects (a design system, a
   library) is used as a dependency when the owner asks for it; a project
   never copies and edits it locally.

## R8. Services

1. A service is a repo with a `Dockerfile` at the root or one folder down. It
   runs on the home server (ZimaOS) from a published image, never from source.
2. CI builds the image on a release tag and publishes it to
   `ghcr.io/<owner>/<repo>` (root Dockerfile) or `ghcr.io/<owner>/<repo>-<folder>`
   (`<folder>/Dockerfile`), tagged `X.Y.Z`, and `latest` unless it is a
   pre-release. `.github/workflows/image.yml` does this, only after the
   release workflow has passed for that tag, and is managed by `jb sync`.
   Images are public: they hold code, never secrets.
3. `compose.yaml` at the root is the deploy file: images only (no `build:`),
   configuration through environment variables, state in named volumes (no
   host bind mounts). It is what gets pasted into Portainer or a
   ZimaOS custom app.
4. Secrets reach a service only as environment variables set on the server;
   `.env.example` lists them (R6.5).
5. Making a newly published package public is a one-time step in the GitHub
   package settings (the owner's decision, R1.2).

## R9. Apps update themselves

1. An app is anything installed on a machine (desktop app, CLI). It never
   updates by pulling and rebuilding source: it updates from its repo's
   latest published GitHub Release, so publishing the draft (R4.3.5) is what
   ships an update.
2. `scripts/build` puts in `dist/`, for each platform the app ships to, the
   bundle, its signature, and one `latest.json` in the
   [Tauri updater format](https://v2.tauri.app/plugin/updater/#static-json-file)
   (`version`, `notes`, `pub_date`, `platforms.<os-arch>.{url,signature}`),
   whatever the stack. URLs point at
   `https://github.com/<owner>/<repo>/releases/download/vX.Y.Z/<file>`.
   A Tauri app keeps Tauri's base64-wrapped signature; any other app may put
   the minisign `.sig` text as it is, since it reads its own feed.
3. The app checks
   `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`
   at startup. A desktop app shows "Update available" with an Update button;
   a CLI prints one line and offers `<app> update`. It verifies the signature
   against the public key built into it before replacing itself, and never
   installs an unsigned or unverified update. The new files are written
   beside the old ones and renamed into place before anything is stopped:
   copying over a running binary fails ("Text file busy" on Linux) and
   leaves the app stopped.
4. The signing key lives only in the repo's Actions secrets
   `UPDATER_SIGNING_KEY` and `UPDATER_SIGNING_KEY_PASSWORD`; `release.yml`
   hands them to `scripts/build`. Losing the key strands every installed
   copy, so the owner keeps a copy in their password manager.
5. Pre-releases never reach installed apps: `releases/latest` skips them.

## R10. App icons

1. An app has two icons and they are different drawings: the app icon
   (launcher, dock, installer) and the tray icon. The tray never shows the
   app icon.
2. The app icon on Linux is a rounded tile on a transparent 1024x1024 canvas:
   896x896 at x=64 y=64 (87.5% of the canvas), corner radius 22.5% of the tile
   (201). Measured 2026-11-12 on the owner's machine as the tile's alpha
   bounding box: the icon theme draws Slack, Chrome and Zen at this size, so
   an app on another grid sits larger or smaller than everything beside it.
3. The app icon on macOS (`.icns`) uses Apple's grid: 824x824 at x=100 y=100
   (80.5%), radius 185. It is the same drawing; only the tile's share of the
   canvas changes. Linux sizes are rendered from the same source, larger by
   896/824.
4. The tile has a top-to-bottom gradient, lighter at the top, and a soft
   shadow underneath that stays inside the canvas. The glyph is centred and
   spans about 60 to 65% of the tile. A flat tile that fills the canvas is
   wrong: Offhand and Monotub shipped one and looked oversized beside the
   others.
5. The tray icon is the glyph alone: one colour, no tile, no gradient, no
   shadow. White on Linux, for a dark bar; a template image on macOS, so the
   menu bar tints it.
6. The tray glyph is the brand mark as drawn: it is scaled and centred on a
   square canvas, never redrawn or thickened. Every tray glyph carries the
   same ink: its opaque area is 22% of the canvas. A bold mark is scaled
   down and a thin one up until they weigh the same; a mark that reaches
   the canvas edge first stops there. Measured 2026-11-12: with ink at 29%
   (Monotub, Noted), 25% (Offhand) and 15% (Squiggle) the n looked oversized
   and the squiggle small in the same tray. 22% is what Squiggle reaches at
   the full width of its canvas, so the others come down to it. A dense,
   compact mark may go up to 25%: at 22% Noted's n looked small beside the
   others, and reads right at 24.5%.
7. Each icon has one vector source in the repo. Every exported size is
   rendered from it by a script and never edited by hand; `AGENTS.md` names
   the sources and the command.
8. Favicons, `apple-touch-icon` and maskable web icons are outside this
   grid: square and full bleed, because the platform masks them.
