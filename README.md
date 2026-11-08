# Git Jet

A JetBrains-style Git tool window, as a standalone desktop application for
Windows, macOS and Linux, built with [Tauri](https://tauri.app).

```
╭─ main ↑2 · origin/main · 6 changes · 1 stash ────────────────────────────╮
│ Local Changes          │  Log / Diff                                    │
│   Staged (2)           │  * a1b2c3d (HEAD -> main) Add feature (Alice)  │
│   Unstaged (1)         │  * 91c0d2e Fix parser          · 2 hours ago   │
│   Untracked (2)        │                                                │
├────────────────────────┴────────────────────────────────────────────────┤
│ [Commit message…]                                        [ Commit ]     │
╰──────────────────────────────────────────────────────────────────────────╯
```

- **Local Changes** — staged / unstaged / untracked / conflict trees with
  per-file staging and one-click "stage all". Every file outside of conflicts
  also has a one-click rollback (↩). While a merge is being resolved an
  **Abort merge** banner offers an escape hatch.
- **Log** — a colored commit graph with branch, tag and `HEAD` ref badges;
  "load more" pagination and an "all branches" toggle. Filters narrow the
  graph by message text, author (case-insensitive) and path, all executed by
  git itself. Right-click a commit to cherry-pick, revert, drop, undo,
  reset the branch to it or create a tag.
- **Commit details** — selecting a commit shows a metadata line (author,
  date, hash, ref badges, merge marker) plus the list of changed files;
  clicking a file loads its patch alone.
- **Diff** — unified diff with JetBrains coloring and a **side-by-side**
  layout toggle; click a file in Local Changes for its diff, or a commit in
  the Log for its patch. While a staged or unstaged diff is shown, each hunk
  (and the whole file) can be rolled back: staged hunks are unstaged,
  worktree hunks are discarded with `git apply -R`.
- **Branches** — one menu listing local and remote branches with tracking
  status; switch or create branches inline. The context menu adds merge into
  current, checkout as a new branch from here — remote branches too —
  compare with current (two-way ahead/behind list), rename, delete (safe `-d`
  with a `-D` confirmation, remote via `push --delete`) and upstream
  tracking setup.
- **Pull / Push dialogs** — the pull dialog picks merge or fast-forward-only
  and an optional explicit remote/branch; the push dialog lists the outgoing
  commits first, allows an explicit remote/target branch and offers set-upstream
  and force-push (`--force-with-lease`).
- **Tags** — a toolbar menu to list, create (at HEAD or any ref), delete and
  push tags.
- **Stashes** — push (with an optional message), pop, apply and drop.
- **Toolbar** — fetch (all), pull/push dialogs, refresh (`F5`) and a status
  bar that keeps the last action visible. The status re-reads itself every
  15 seconds while the window is focused, so external changes show up
  without a manual refresh.

## Requirements

- [Node.js](https://nodejs.org) ≥ 18 (for the Tauri CLI)
- [Rust](https://rustup.rs) — Tauri installs the toolchain targets it needs
- Git on `PATH`
- On Windows: [WebView2](https://developer.microsoft.com/microsoft-edge/webview2)
  (preinstalled on Windows 11)

## Run it

```sh
cd app
npm install
npm run tauri dev
```

The window opens, remembers the last repository you opened, and asks for a
folder on first start.

## Build an installer

```sh
cd app
npm run tauri build
```

Installers land in `app/src-tauri/target/release/bundle/`.

## Architecture

```
src/core/       Platform-independent Git engine (parsers + orchestration)
src/lib.rs      Engine facade: StdExecutor backed by std::process
app/
  src-tauri/    Tauri backend — adapts the engine to typed commands
  frontend/     Vanilla JS + CSS UI (no build step)
  icon/         Procedural icon generator (node icon/make-icon.mjs)
tests/          Integration tests against real temporary repositories
```

The desktop app never touches a shell: every Git invocation goes through the
engine's direct process API with arguments passed verbatim, so paths and
commit messages with spaces or quotes are safe. History comes from
`git status --porcelain=v2`, `git log --pretty=format:...` with `%P`/`%d`,
`git branch --format=...` and `git stash list --format=...`, parsed in
`src/core`.

## Permissions and safety

- No shell is involved in any git invocation.
- Read-only commands: status, log, diff, branches, stashes, show, blame.
- Worktree-changing commands: stage, unstage, rollback (restore / checkout,
  `git clean -fd` for untracked, `git apply -R` for hunks), commit, switch,
  new-branch, stash push/pop/apply/drop, pull.
- Network commands: fetch, pull, push.
- No telemetry, and no network access other than the Git commands you run.

## Tests

```sh
cargo test
```

The suite covers status porcelain v2 parsing (staged, unstaged, renames,
copies, conflicts, untracked, ignored, detached and unborn branches), log /
branch / stash parsing including the graph format with parents and ref
decorations, rendering, and integration tests that create real repositories
and exercise status, diffs, staging, commits, amend, branches, switches,
stashes, blame, file- and hunk-wise rollback, repository-error handling,
merge + conflict detection + abort, commit details with per-file diffs,
tags, cherry-pick / revert / drop / undo, soft/mixed/hard resets, log
filters and push/outgoing flows against a local bare remote.

## License

MIT. See [LICENSE](LICENSE).
