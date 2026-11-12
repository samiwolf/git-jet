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
  **Abort merge** banner offers an escape hatch; a running **rebase** gets its
  own banner with Continue (after conflicts are staged), **Skip commit** and
  **Abort**, and a running **bisect** shows an **End bisect** escape hatch.
- **Merge editor** — clicking a conflicted file opens a JetBrains-style,
  three-pane dialog: current revision (ours) left, incoming (theirs) right
  and the merge result below, all aligned and scrolling together. Each
  conflict block offers "Accept ours"/"Accept theirs" (plus an undo chip on
  the choice), the header has bulk buttons for every block and a live
  "N unresolved" counter, and **Apply** writes the resolved file and stages
  it — disabled while any conflict is open. Cancel (Esc inside the dialog)
  leaves the merge untouched and warns first.
- **Log** — a colored commit graph with branch, tag and `HEAD` ref badges;
  "load more" pagination and an "all branches" toggle. Filters narrow the
  graph by message text, author (case-insensitive) and path, all executed by
  git itself. Right-click a commit to cherry-pick, revert, drop, undo,
  reword its message, squash it into its parent, reset the branch to it or
  create a tag. While a bisect session runs, the same menu offers
  **Mark good / Mark bad / Skip** for that commit. Rewrites use git plumbing
  (`hash-object` + `update-ref` with a compare-and-swap), so they preserve
  merges, never move the working tree and keep authors intact.
- **Rebase** — rebase the current branch onto any branch or commit
  (`git rebase <target>`), and pull `--rebase` as a third pull-dialog
  strategy. The branch context menu mirrors WebStorm's wording, naming the
  branch that gets **rewritten** first: on a non-current branch,
  **Rebase '&lt;current&gt;' onto '&lt;selected&gt;'** replays the current
  branch's commits on top of the selected one without switching, and
  **Checkout and Rebase onto Current** checks the selected branch out and
  replays it on top of the previously current branch. A stopped rebase
  surfaces the same conflict flow as a merge: resolve and stage, then
  Continue — or Skip the blocking commit / Abort entirely.
- **Cherry-pick / revert escape hatches** — a conflicted cherry-pick or
  revert can be **continued** once the files are resolved and staged, or
  **aborted** to restore the pre-operation state, mirroring the merge flow.
- **Incoming commits** — the `↓ N` pill in the branch bar (and the context
  menu) lists what a pull would bring in (`HEAD..@{upstream}`) after a
  fetch, with a pull shortcut.
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
- **Pull / Push dialogs** — the pull dialog picks merge, fast-forward-only
  or **rebase** and an optional explicit remote/branch; the push dialog
  lists the outgoing commits first, allows an explicit remote/target branch
  and offers set-upstream and force-push (`--force-with-lease`).
- **Remotes** — a manage-remotes dialog to add a remote, edit its URL, fetch
  a single remote and remove it (with confirmation). The start screen clones
  a repository from a URL into a picked destination folder and opens it.
- **Worktrees** — a manage-worktrees dialog listing every linked working
  tree (path, branch, HEAD) with add (`git worktree add -b <branch> <path>
  [<start-ref>]`), remove (with confirmation, `--force` after a second
  prompt) and prune.
- **Bisect** — a bisect dialog to start a session (`git bisect start
  [bad] [good]`), mark the current HEAD good/bad/skip, watch the `git bisect
  log` transcript grow and reset back to the original HEAD. Individual
  commits can be marked from the Log context menu; completion (the first
  bad commit) is reported in the status bar.
- **.gitignore** — untracked files gain an "Add to .gitignore" context-menu
  action (also for the whole Untracked section); patterns are deduplicated
  and `!`/`#` prefixes are escaped.
- **Tags** — a toolbar menu to list, create (at HEAD or any ref), delete and
  push tags.
- **Stashes** — push (with an optional message), pop, apply and drop.
- **Toolbar** — fetch (all), fetch **and prune** (drops remote-tracking
  branches the remote no longer has), pull/push dialogs, refresh (`F5`) and
  a status bar that keeps the last action visible. The status re-reads itself
  every 15 seconds while the window is focused — a cheap filesystem signature
  of the repository (index, `HEAD`, refs, merge/rebase/bisect state) skips
  the git work when nothing has changed, so external changes still show up
  without a manual refresh.
- **Op Log** — a viewer for the last 1000 git commands Git Jet ran this
  session (arguments, outcome and duration, newest first) with a
  clear-log button. A second tab, **Status messages**, lists every
  status-bar message of the session (newest first, errors highlighted) —
  handy for copying a long git error after the bar has moved on. The status
  bar itself has a copy button (⧉) that puts the current message, full and
  untruncated, on the clipboard. Both views are in-memory only; nothing is
  written to disk.

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
`src/core`. Git runs with `core.fsmonitor` (its filesystem-watch daemon) and
the untracked cache enabled per invocation, which keeps `status` fast on
large repositories without touching the repository's own configuration.

## Permissions and safety

- No shell is involved in any git invocation.
- Read-only commands: status, log, diff, branches, stashes, show, blame,
  remotes, worktree list, bisect log, reading a conflicted file's worktree
  content.
- Worktree-changing commands: stage, unstage, rollback (restore / checkout,
  `git clean -fd` for untracked, `git apply -R` for hunks), commit, switch,
  new-branch, stash push/pop/apply/drop, pull, merge, rebase (onto / continue
  / skip / abort), cherry-pick and revert (continue / abort), bisect
  start/mark/reset, worktree add/remove/prune, fetch, and writing a resolved
  merge result (+ `git add` on that one file).
- History rewrites (reword, squash into parent) go through `git
  hash-object`/`rev-list`/`update-ref` plumbing and never touch the index or
  working tree; the branch ref only moves when the rewritten tip has the
  identical tree.
- Network commands: fetch (optionally `--prune`), pull, push, clone.
- File writes are confined to the opened repository: paths are resolved
  against the repo root and `..`/absolute escapes are rejected. The one
  exception is `git worktree add`, whose destination folder you choose
  explicitly and which is executed by git itself, never through a shell.
- No telemetry, and no network access other than the Git commands you run.

## Tests

```sh
cargo test
```

The suite covers status porcelain v2 parsing (staged, unstaged, renames,
copies, conflicts, untracked, ignored, detached and unborn branches), log /
branch / stash parsing including the graph format with parents and ref
decorations, worktree-list porcelain parsing, rendering, and integration
tests that create real repositories and exercise status, diffs, staging,
commits, amend, branches, switches, stashes, blame, file- and hunk-wise
rollback, repository-error handling, merge + conflict detection + abort +
the three-pane resolve round trip (read, resolve, stage), commit details
with per-file diffs, tags, cherry-pick / revert / drop / undo with their
conflict abort/continue flows, soft/mixed/hard resets, log filters,
push/outgoing flows against a local bare remote, rebase onto a branch with
conflict abort and resolve-continue, `pull --rebase` linearization and its
conflict state, pruned fetches, remote management
(add / set-url / fetch / remove), incoming commits, cloning, .gitignore
edits, squash-into-parent / reword history rewrites, bisect sessions, and
worktree add/list/remove/prune.

## License

MIT. See [LICENSE](LICENSE).
