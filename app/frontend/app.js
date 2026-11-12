/* Git Jet frontend.
 *
 * Vanilla JS talking to the Tauri backend via `window.__TAURI__`
 * (enabled with `withGlobalTauri`). No build step required.
 */

"use strict";

/* Surface every error visibly inside the window, so a broken startup is
 * never mistaken for a blank page. */
function showBootError(message) {
  const box = document.getElementById("boot-error");
  if (box) {
    box.textContent = message;
  } else {
    document.body.textContent = message;
  }
}

window.addEventListener("error", (event) => {
  report(`Script error: ${event.message}\n${event.filename}:${event.lineno}`);
});
window.addEventListener("unhandledrejection", (event) => {
  report(`Unhandled rejection: ${errorText(event.reason)}`);
});

/* Boot-mode errors go to the red overlay; once a repository is open they go
 * to the status bar instead. */
function report(message) {
  if (typeof state === "undefined" || !state.root) {
    showBootError(message);
  } else {
    setStatus(message, true);
  }
}

/* ------------------------------------------------------------------ tauri */

function tauri() {
  return window.__TAURI__ || null;
}

async function invoke(cmd, args = {}) {
  const api = tauri();
  if (!api) throw new Error("Tauri API is not available.");
  return await api.core.invoke(cmd, args);
}

async function pickFolder() {
  const api = tauri();
  const dialog = api && api.dialog;
  if (dialog && typeof dialog.open === "function") {
    const picked = await dialog.open({ directory: true, multiple: false });
    return typeof picked === "string" ? picked : null;
  }
  return null;
}

/* -------------------------------------------------------------- constants */

const LOG_PAGE = 100;

const KIND_CLASS = {
  changed: "k-changed",
  added: "k-added",
  deleted: "k-deleted",
  renamed: "k-renamed",
  copied: "k-copied",
  unmerged: "k-unmerged",
  untracked: "k-untracked",
  ignored: "k-muted",
};

const KIND_LETTER = {
  changed: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
  copied: "C",
  unmerged: "U",
  untracked: "?",
  ignored: "!",
};

const LANE_COLORS = [
  "#3574f0", "#499c54", "#dcb167", "#d570b6",
  "#6a9dc5", "#c9684d", "#8a7ce0", "#4aa9a4",
];
/* Horizontal distance between two graph lanes (px). */
const LANE_W = 14;

/* ------------------------------------------------------------------ state */

const state = {
  root: null,
  repoName: "",
  logCount: LOG_PAGE,
  logAll: false,
  logCommits: [],     // accumulated log rows (append-only pagination)
  lastStamp: null,    // repo_stamp signature at the last status refresh
  selectedFile: null,   // { path, staged, unstaged, untracked }
  selectedCommit: null, // commit hash shown in the diff tab
  commitFile: null,     // single file selected inside a commit detail
  diffMode: "all",
  diffLayout: localStorage.getItem("gitjet.diffLayout") || "unified",
  logFilters: { text: "", author: "", path: "" },
  commitDetail: null,   // last loaded CommitDetail DTO
  loadedStatus: null,
  mergeFile: null,      // conflict entry open in the merge editor
  mergeSegments: null,  // parsed conflict segments of that file
  mergeTrailingNewline: false,
};

const ROLLBACK_LABEL = {
  staged: "Rollback staged changes (revert to HEAD)",
  unstaged: "Rollback unstaged changes",
  untracked: "Rollback (delete untracked file)",
};

/* ------------------------------------------------------------- dom helpers */

const $ = (id) => document.getElementById(id);

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function show(node) { node.classList.remove("hidden"); }
function hide(node) { node.classList.add("hidden"); }

let statusTimer = null;
function setStatus(text, isError) {
  const bar = $("status-bar") || $("status-text").parentElement;
  const node = $("status-text");
  node.textContent = text;
  if (isError) {
    bar.classList.add("error");
    clearTimeout(statusTimer);
    statusTimer = setTimeout(() => bar.classList.remove("error"), 6000);
  } else {
    bar.classList.remove("error");
  }
}

function truncate(text, limit = 600) {
  const lines = text.trimEnd().split("\n");
  if (lines.length <= limit) return text.trimEnd();
  return lines.slice(0, limit).join("\n") + `\n… (+${lines.length - limit} more lines)`;
}

/* Disable a button while its async work is in flight so repeated clicks
   are no-ops. The button always re-enables, even when the work throws. */
async function guardButton(btn, fn) {
  if (!btn || btn.disabled) return undefined;
  btn.disabled = true;
  btn.classList.add("busy");
  try {
    return await fn();
  } finally {
    btn.disabled = false;
    btn.classList.remove("busy");
  }
}

async function runAction(label, fn, refreshAfter = true, btn = null) {
  return guardButton(btn, async () => {
    try {
      setStatus(`${label}…`);
      const output = await fn();
      setStatus(truncate(String(output || `${label} done`).trim(), 3));
      if (refreshAfter) {
        await Promise.all([refreshStatus(), refreshLog(), refreshBranches()]);
        await loadDiff().catch(() => {});
      }
    } catch (error) {
      setStatus(`${label} failed: ${errorText(error)}`, true);
    }
  });
}

function errorText(error) {
  const message = typeof error === "string" ? error : (error && error.message) || String(error);
  return message.replace(/^command /, "").trim();
}

/* ------------------------------------------------------------ empty state */

const RECENT_KEY = "gitjet.recentRepos";
const RECENT_LIMIT = 10;

function loadRecent() {
  try {
    const raw = JSON.parse(localStorage.getItem(RECENT_KEY));
    if (Array.isArray(raw)) {
      return raw.filter((entry) => entry && typeof entry.root === "string");
    }
  } catch { /* ignore corrupt value */ }
  return [];
}

/* Keep at most RECENT_LIMIT entries, newest first. */
function recordRecent(root, name) {
  const rest = loadRecent().filter((entry) => entry.root !== root);
  rest.unshift({ root, name, lastOpened: Date.now() });
  localStorage.setItem(RECENT_KEY, JSON.stringify(rest.slice(0, RECENT_LIMIT)));
}

function forgetRecent(root) {
  localStorage.setItem(RECENT_KEY, JSON.stringify(loadRecent().filter((entry) => entry.root !== root)));
}

function recentName(entry) {
  return entry.name || entry.root.replace(/[\\/]+$/, "").split(/[\\/]/).pop();
}

function renderRecent() {
  const wrap = $("recent-repos");
  const list = $("recent-list");
  const entries = loadRecent();
  list.replaceChildren(...entries.map((entry) => {
    const row = el("button", "recent-item");
    row.type = "button";
    row.title = entry.root;
    row.appendChild(el("span", "recent-name", recentName(entry)));
    row.appendChild(el("span", "recent-path", entry.root));

    const forget = el("span", "recent-forget", "✕");
    forget.title = "Remove from this list";
    forget.addEventListener("click", (event) => {
      event.stopPropagation();
      forgetRecent(entry.root);
      renderRecent();
    });
    row.appendChild(forget);

    row.addEventListener("click", () => openRepository(entry.root));
    return row;
  }));
  if (entries.length) show(wrap); else hide(wrap);
}

function enterApp(root, name) {
  state.root = root;
  state.repoName = name || root.replace(/[\\/]+$/, "").split(/[\\/]/).pop();
  localStorage.setItem("gitjet.lastRoot", JSON.stringify({ root, name }));
  recordRecent(root, state.repoName);
  const box = $("boot-error");
  box.textContent = "";
  $("status-repo").textContent = state.repoName;
  hide($("empty-state"));
  show($("app"));
  refreshAll();
}

function leaveApp(message) {
  state.root = null;
  localStorage.removeItem("gitjet.lastRoot");
  renderRecent();
  hide($("app"));
  show($("empty-state"));
  const error = $("empty-error");
  if (message) {
    error.textContent = message;
    show(error);
  } else {
    /* An old failure (e.g. a previous open attempt) must not linger once
     * the user comes back to this screen without new news. */
    error.textContent = "";
    hide(error);
  }
  $("status-repo").textContent = "";
}

async function openRepository(path) {
  try {
    const info = await invoke("open_repository", { path });
    enterApp(info.root, info.name);
  } catch (error) {
    /* A repository that no longer opens has no business staying in the
     * recents list; leaveApp re-renders the (updated) list. */
    forgetRecent(path);
    leaveApp(
      errorText(error).includes("Not a Git repository")
        ? `"${path}" is not a Git repository.`
        : errorText(error)
    );
  }
}

async function browseForRepository() {
  try {
    const path = await pickFolder();
    if (path) await openRepository(path);
  } catch (error) {
    setStatus(`Could not open a folder picker: ${errorText(error)}`, true);
  }
}

/* -------------------------------------------------------------- clone flow */

let cloneParent = null;

function suggestRepoName(url) {
  const last = url.trim().replace(/[\\/]+$/, "").split(/[\\/:]/).pop() || "";
  return last.replace(/\.git$/i, "");
}

function updateCloneDest() {
  const name = $("clone-name").value.trim() || suggestRepoName($("clone-url").value);
  $("clone-dest").textContent = cloneParent
    ? `Destination: ${cloneParent.replace(/[\\/]+$/, "")}${name ? "/" + name : ""}`
    : "Pick a destination folder first.";
}

function openCloneDialog() {
  cloneParent = null;
  $("clone-url").value = "";
  $("clone-name").value = "";
  $("clone-error").textContent = "";
  updateCloneDest();
  show($("clone-dialog"));
  $("clone-url").focus();
}

async function browseCloneDestination() {
  const picked = await pickFolder();
  if (picked) {
    cloneParent = picked;
    updateCloneDest();
  }
}

async function submitCloneDialog() {
  const url = $("clone-url").value.trim();
  const name = $("clone-name").value.trim() || suggestRepoName(url);
  $("clone-error").textContent = "";
  if (!url) {
    $("clone-error").textContent = "Enter a repository URL first.";
    return;
  }
  if (!name) {
    $("clone-error").textContent = "The URL does not suggest a folder name — type one.";
    return;
  }
  if (!cloneParent) {
    $("clone-error").textContent = "Pick a destination folder first.";
    return;
  }
  $("clone-ok").disabled = true;
  try {
    setStatus(`Cloning ${url}…`);
    const info = await invoke("clone_repository", { url, parent: cloneParent, name });
    hide($("clone-dialog"));
    setStatus(String(info.root || "Clone complete"));
    await openRepository(info.root);
  } catch (error) {
    $("clone-error").textContent = errorText(error);
    setStatus(`Clone failed: ${errorText(error)}`, true);
  } finally {
    $("clone-ok").disabled = false;
  }
}

/* ----------------------------------------------------------------- status */

async function refreshStatus() {
  if (!state.root) return;
  const status = await invoke("get_status");
  state.loadedStatus = status;
  state.selectedFile = matchSelection(status);
  renderBranchBar(status);
  renderChanges(status);
  // Remember the repository's filesystem signature so the auto-refresh
  // timer can skip the next full re-read while nothing changed.
  state.lastStamp = await invoke("repo_stamp").catch(() => null);
}

function renderBranchBar(status) {
  const branch = status.branch;
  const parts = [el("span", "summary", branch.display)];
  if (branch.upstream) parts.push(el("span", "muted", `↔ ${branch.upstream}`));
  if (branch.behind > 0) {
    const pill = el("span", "count-pill pull", `↓ ${branch.behind}`);
    pill.title = `Behind upstream — ${branch.behind} incoming commit(s). Click to view.`;
    pill.addEventListener("click", () => void openIncomingDialog());
    parts.push(pill);
  }
  if (branch.ahead > 0) {
    const pill = el("span", "count-pill push", `↑ ${branch.ahead}`);
    pill.title = `Ahead of upstream — ${branch.ahead} commit(s) to push`;
    parts.push(pill);
  }
  if (status.stashCount > 0) {
    const pill = el("span", "count-pill stash", `◫ ${status.stashCount}`);
    pill.title = `${status.stashCount} stash(es)`;
    parts.push(pill);
  }

  const info = el("span");
  if (status.conflicts.length > 0) {
    info.textContent = `⚠ ${status.conflicts.length} conflict(s)`;
    info.style.color = "var(--conflict)";
    parts.push(info);
  }

  const bar = $("branch-bar");
  bar.replaceChildren(...parts);
  $("branch-label").textContent = branch.name;

  const behind = branch.behind || 0;
  const ahead = branch.ahead || 0;
  $("pull-btn").textContent = behind > 0 ? `Pull ↓${behind}` : "Pull";
  $("push-btn").textContent = ahead > 0 ? `Push ↑${ahead}` : "Push";
  $("stash-button").replaceChildren(
    document.createTextNode("Stash "),
    status.stashCount > 0 ? el("span", "count-badge", status.stashCount) : document.createTextNode(""),
    document.createTextNode("▾")
  );
}

function fileRow(entry, section, onSelect) {
  const row = el("div", "file-row");
  const [dir, base] = splitPath(entry.displayPath);
  const kind = el("span", `file-kind ${KIND_CLASS[entry.kind] || ""}`, KIND_LETTER[entry.kind] || "?");
  const path = el("span", "file-path");
  if (dir) {
    path.append(el("span", "file-dir", dir + "/"), base);
  } else {
    path.textContent = base;
  }
  const action = el("span", "file-action", stageLabel(section));
  action.title = section === "staged" ? "Unstage" : "Stage";
  action.addEventListener("click", (event) => {
    event.stopPropagation();
    const command = section === "staged" ? "unstage" : "stage";
    runAction(action.title, () => invoke(command, { paths: [entry.path] }), true, action);
  });
  row.append(kind, path, action);
  if (ROLLBACK_LABEL[section]) row.append(rollbackButton(entry, section));
  row.title = entry.displayPath;
  row.dataset.section = section;
  row.addEventListener("click", () => onSelect(row));
  row.addEventListener("contextmenu", (event) => showFileContextMenu(event, entry, section, row));
  return row;
}

function rollbackButton(entry, section) {
  const button = el("span", "file-action rollback-action", "↩");
  button.title = ROLLBACK_LABEL[section];
  button.addEventListener("click", (event) => {
    event.stopPropagation();
    void guardButton(button, () => rollbackFiles([entry.path], section, entry.displayPath));
  });
  return button;
}

async function rollbackFiles(paths, level, display) {
  const description = ROLLBACK_LABEL[level] || "Rollback";
  const question = Array.isArray(display) ? display.join("\n") : (display || paths.join("\n"));
  if (!confirm(`${description}\n\n${question}\n\nDiscarded changes cannot be undone. Continue?`)) return;
  await runAction("Rollback", () => invoke("rollback_files", { paths, level }));
}

/* Hunk rollback: reverse-apply one diff hunk. Only offered while the shown
 * diff maps cleanly onto one base — staged (index) or unstaged (worktree). */
function hunkRollbackTarget() {
  if (state.selectedCommit) return null;
  if (state.diffMode === "staged") return "staged";
  if (state.diffMode === "unstaged") return "unstaged";
  return null;
}

async function rollbackHunk(file, chunk) {
  const target = hunkRollbackTarget();
  if (!target) return;
  const patch = buildPatch(file, chunk);
  if (!patch) return;
  const effect = target === "staged"
    ? "Unstage this hunk?"
    : "Discard this hunk from the working tree?";
  if (!confirm(`${effect}\n\nDiscarded changes cannot be undone. Continue?`)) return;
  const label = target === "staged" ? "Unstage hunk" : "Rollback hunk";
  await runAction(label, () => invoke("discard_hunk", { patch, staged: target === "staged" }));
}

/* Rebuilds the exact `git diff` fragment for one hunk from the raw lines the
 * parser kept, so `git apply -R` sees byte-identical input. */
function buildPatch(file, chunk) {
  const header = file.header || [];
  const raw = chunk.raw || [];
  if (header.length === 0 || raw.length === 0) return "";
  return header.join("\n") + "\n" + raw.join("\n") + "\n";
}

function stageLabel(section) {
  if (section === "staged") return "−";
  return "→"; // stage
}

function splitPath(display) {
  const index = Math.max(display.lastIndexOf("/"), display.lastIndexOf("\\"));
  return index === -1 ? ["", display] : [display.slice(0, index), display.slice(index + 1)];
}

/* -------------------------------------------------------- context menus */

/* One floating menu reused for every right-click. Each surface builds its own
 * item list: file rows, section heads, the changes-list background, commits,
 * branches and stashes; everything else falls back to repository actions.
 * Text fields and the diff view keep the native menu for copy/paste. */

function contextMenu() {
  let menu = $("context-menu");
  if (!menu) {
    menu = el("div", "dropdown context-menu hidden");
    menu.id = "context-menu";
    document.body.append(menu);
  }
  return menu;
}

function closeContextMenu() {
  hide(contextMenu());
}

function openContextMenu(event, items) {
  event.preventDefault();
  event.stopPropagation();
  const menu = contextMenu();
  const nodes = [];
  let actionable = 0;
  for (const item of items) {
    if (item.sep) { nodes.push(el("div", "context-sep")); continue; }
    if (item.head) { nodes.push(el("div", "dropdown-head", item.head)); continue; }
    const row = el("div", "context-item", item.label);
    if (item.hint) row.append(el("span", "context-hint", item.hint));
    if (item.danger) row.classList.add("danger");
    row.addEventListener("click", () => {
      closeContextMenu();
      item.run();
    });
    nodes.push(row);
    actionable++;
  }
  if (actionable === 0) return;
  menu.replaceChildren(...nodes);
  show(menu);
  const rect = menu.getBoundingClientRect();
  menu.style.left = Math.max(4, Math.min(event.clientX, window.innerWidth - rect.width - 8)) + "px";
  menu.style.top = Math.max(4, Math.min(event.clientY, window.innerHeight - rect.height - 8)) + "px";
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    setStatus(`Copied: ${text}`);
  } catch (error) {
    setStatus(`Copy failed: ${errorText(error)}`, true);
  }
}

function selectFileRow(entry, section, row) {
  document.querySelectorAll("#changes-list .file-row.selected").forEach((n) => n.classList.remove("selected"));
  if (row) row.classList.add("selected");
  state.selectedFile = { entry, section };
  state.selectedCommit = null;
  switchTab("diff");
  loadDiff();
}

function showFileContextMenu(event, entry, section, row) {
  const items = [{ head: entry.displayPath }];
  if (section === "conflict") {
    items.push({ label: "Resolve in merge editor…", run: () => openMergeEditor(entry) });
    items.push({ label: "Stage (mark resolved)", hint: "→", run: () =>
      runAction("Stage file", () => invoke("stage", { paths: [entry.path] })) });
  } else if (section === "staged") {
    items.push({ label: "Unstage", hint: "−", run: () =>
      runAction("Unstage file", () => invoke("unstage", { paths: [entry.path] })) });
    items.push({ label: "Open staged diff", run: () => selectFileRow(entry, section, row) });
  } else if (section === "untracked") {
    items.push({ label: "Stage", hint: "→", run: () =>
      runAction("Stage file", () => invoke("stage", { paths: [entry.path] })) });
    items.push({ label: "Add to .gitignore", run: () =>
      runAction(`Ignore ${entry.displayPath}`, () => invoke("add_to_gitignore", { paths: [entry.path] })) });
  } else {
    items.push({ label: "Stage", hint: "→", run: () =>
      runAction("Stage file", () => invoke("stage", { paths: [entry.path] })) });
    items.push({ label: "Open diff", run: () => selectFileRow(entry, section, row) });
  }
  if (ROLLBACK_LABEL[section]) {
    items.push({ sep: true });
    items.push({
      label: "Rollback (discard changes)…",
      danger: true,
      hint: section === "untracked" ? "deletes the file" : undefined,
      run: () => rollbackFiles([entry.path], section, entry.displayPath),
    });
  }
  items.push({ sep: true });
  items.push({ label: "Copy path", run: () => copyText(entry.path) });
  openContextMenu(event, items);
}

function showSectionContextMenu(event, title, section, files) {
  const items = [{ head: `${title} (${files.length})` }];
  if (files.length > 0) {
    if (section === "staged") {
      items.push({ label: `Unstage all (${files.length})`, run: () =>
        runAction("Unstage all", () => invoke("unstage", { paths: files.map((f) => f.path) })) });
    } else if (section === "untracked") {
      items.push({ label: `Stage all (${files.length})`, run: () =>
        runAction("Stage all", () => invoke("stage", { paths: files.map((f) => f.path) })) });
      items.push({ label: `Add all to .gitignore (${files.length})`, run: () =>
        runAction("Add to .gitignore", () => invoke("add_to_gitignore", { paths: files.map((f) => f.path) })) });
    } else if (section !== "conflict") {
      items.push({ label: `Stage all (${files.length})`, run: () =>
        runAction("Stage all", () => invoke("stage", { paths: files.map((f) => f.path) })) });
    }
  }
  items.push({ sep: true });
  items.push(...repoMenuItems());
  openContextMenu(event, items);
}

function showChangesContextMenu(event) {
  const status = state.loadedStatus;
  const staged = (status && status.staged) || [];
  const stageable = status ? status.unstaged.concat(status.untracked) : [];
  const items = [{ head: "Local Changes" }];
  if (staged.length > 0) {
    items.push({ label: `Unstage all (${staged.length})`, run: () =>
      runAction("Unstage all", () => invoke("unstage", { paths: staged.map((f) => f.path) })) });
  }
  if (stageable.length > 0) {
    items.push({ label: `Stage all (${stageable.length})`, run: () =>
      runAction("Stage all", () => invoke("stage", { paths: stageable.map((f) => f.path) })) });
  }
  items.push({ sep: true });
  items.push(...repoMenuItems());
  openContextMenu(event, items);
}

/* Pull / push / stash counts come from the last loaded status, so the labels
 * always match what the branch bar shows. */
function repoMenuItems() {
  const status = state.loadedStatus;
  const branch = status ? status.branch : null;
  const behind = branch ? branch.behind || 0 : 0;
  const ahead = branch ? branch.ahead || 0 : 0;
  const stashes = status ? status.stashCount || 0 : 0;
  return [
    { label: `Pull (↓ ${behind})`, hint: "dialog", run: openPullDialog },
    { label: `Push (↑ ${ahead})`, hint: "dialog", run: openPushDialog },
    { label: `Stash all changes (${stashes} existing)`, hint: "stash push", run: () =>
      runAction("Stash push", () => invoke("stash_push", { message: $("stash-message").value.trim() })) },
    { sep: true },
    { label: "Fetch ⮁ All", run: () => runAction("Fetch", () => invoke("fetch", { all: true })) },
    { label: "Incoming commits…", hint: `↓ ${behind}`, run: openIncomingDialog },
    { label: "Manage remotes…", run: openRemotesDialog },
    { label: "Reset changes (hard)", danger: true, run: () =>
      { if (confirm("Hard reset of the current branch to HEAD?\n\nAll uncommitted staged and worktree changes will be deleted!")) {
        void runAction("Hard reset", () => invoke("reset_branch", { sha: "HEAD", mode: "hard" }));
      } } },
    { label: "Refresh", hint: "F5", run: refreshAll },
  ];
}

function showGeneralContextMenu(event) {
  if (!state.root) return;
  openContextMenu(event, [{ head: state.repoName || "Git Jet" }, ...repoMenuItems()]);
}

function rows() {
  return document.querySelectorAll(".file-row");
}function renderChanges(status) {
  const list = $("changes-list");
  list.replaceChildren();

  // While a merge is being resolved, offer an escape hatch next to the list.
  if (status.mergeInProgress) {
    const abort = el("div", "merge-abort");
    abort.append(
      el("span", "merge-abort-text", "Merging — resolve conflicts, stage and commit."),
      (() => {
        const btn = el("button", "btn small", "Abort merge");
        btn.addEventListener("click", () => {
          if (!confirm("Abort the merge?\n\nThe pre-merge state will be restored.")) return;
          void runAction("Abort merge", () => invoke("merge_abort"), true, btn);
        });
        return btn;
      })()
    );
    list.append(abort);
  }

  const sections = [];
  if (status.conflicts.length > 0) {
    sections.push(["Conflicts", status.conflicts, "conflict", null]);
  }
  sections.push(
    ["Staged", status.staged, "staged", "unstage_all"],
    ["Unstaged", status.unstaged, "unstaged", "stage_section"],
    ["Untracked", status.untracked, "untracked", "stage_section"]
  );

  let anyShown = false;
  for (const [title, files, section] of sections) {
    if (title === "Conflicts" || files.length > 0) anyShown = true;

    const head = el("div", "changes-section-head", `${title} (${files.length})`);
    const button = el("button", "btn", title === "Staged" ? "unstage all" : "stage all");
    if (files.length > 0) {
      const command = title === "Staged" ? "unstage" : "stage";
      const label = title === "Staged" ? "Unstage all" : "Stage all";
      button.addEventListener("click", () =>
        runAction(label, () => invoke(command, { paths: files.map((f) => f.path) }), true, button)
      );
    } else {
      hide(button);
    }
    head.append(button);
    head.addEventListener("contextmenu", (event) => showSectionContextMenu(event, title, section, files));
    list.append(head);

    if (files.length === 0) {
      list.append(el("div", "empty-hint", title === "Staged" ? "Nothing staged" : "Nothing here"));
    } else {
      for (const entry of files) {
        const row = fileRow(entry, section, (node) => {
          list.querySelectorAll(".file-row.selected").forEach((n) => n.classList.remove("selected"));
          node.classList.add("selected");
          state.selectedFile = { entry, section };
          state.selectedCommit = null;
          if (section === "conflict") {
            openMergeEditor(entry);
            return;
          }
          switchTab("diff");
          loadDiff();
        });
        if (
          state.selectedFile &&
          state.selectedFile.section === section &&
          state.selectedFile.entry.path === entry.path
        ) {
          row.classList.add("selected");
        }
        list.append(row);
      }
    }
  }

  if (!anyShown) {
    list.append(el("div", "empty-hint", "✔ Working tree is clean"));
  }
}

function allButton(title, files) {
  if (title === "Staged") return "unstage all";
  if (title === "Conflicts") return "";
  return "stage all";
}

/* Re-apply the current file selection to a freshly loaded status list. Returns
   the updated selection, or null when the file no longer has any changes —
   callers then fall back to the working-tree diff for the active mode. */
function matchSelection(status) {
  const sel = state.selectedFile;
  if (!sel) return null;
  const bySection = [
    ["conflict", status.conflicts],
    ["staged", status.staged],
    ["unstaged", status.unstaged],
    ["untracked", status.untracked],
  ];
  const own = bySection.find(([section]) => section === sel.section);
  const ownEntry = own && (own[1] || []).find((f) => f.path === sel.entry.path);
  if (ownEntry) return { entry: ownEntry, section: sel.section };
  for (const [section, files] of bySection) {
    const entry = (files || []).find((f) => f.path === sel.entry.path);
    if (entry) return { entry, section };
  }
  return null;
}

/* ------------------------------------------------------------------- diff */

function diffModeFor(entry, section) {
  const hasStaged = entry.index && entry.index !== "." && entry.index !== " ";
  const hasUnstaged = entry.worktree && entry.worktree !== "." && entry.worktree !== " ";
  if (section === "staged") return "staged";
  if (section === "unstaged" || section === "conflict") return hasStaged ? "all" : "unstaged";
  if (hasUnstaged && hasStaged) return "all";
  return hasStaged ? "staged" : "unstaged";
}

async function loadDiff() {
  if (!state.root) return;
  const view = $("diff-view");
  // Mode tabs (All/Unstaged/Staged) only apply to the working tree; hide them
  // while a commit diff is shown so they don't imply they'd switch anything.
  document.querySelector(".diff-mode").classList.toggle("hidden", Boolean(state.selectedCommit));
  try {
    if (state.selectedCommit) {
      const sha = state.selectedCommit;
      const detail = await invoke("commit_detail", { sha });
      state.commitDetail = detail;
      state.commitFile = null;
      const subjectNode = $("diff-subject");
      const checkoutBtn = el("button", "btn small commit-checkout", "Checkout ⤴");
      checkoutBtn.title = "Check out this commit (detached HEAD)";
      checkoutBtn.addEventListener("click", () => checkoutCommit(sha, detail.subject));
      subjectNode.replaceChildren(
        document.createTextNode(`commit ${sha.slice(0, 10)} · `),
        checkoutBtn
      );
      renderCommitDetail(detail);
      const patch = await invoke("get_commit", { sha });
      renderDiff(patch);
      return;
    }
    state.commitDetail = null;
    state.commitFile = null;
    hide($("commit-detail"));
    if (!state.selectedFile) {
      $("diff-subject").textContent = "working tree";
      setDiffMode(state.diffMode, false);
      const text = await invoke("get_diff", { mode: state.diffMode });
      renderDiff(text);
      return;
    }

    const { entry, section } = state.selectedFile;
    if (section === "untracked") {
      $("diff-subject").textContent = entry.displayPath;
      view.replaceChildren(
        el("div", "diff-empty", "Untracked file — stage it (→) to see the diff against the index.")
      );
      return;
    }

    const mode = diffModeFor(entry, section);
    const subjectLabels = { staged: "staged diff", unstaged: "unstaged diff", all: "all changes" };
    $("diff-subject").textContent = `${entry.displayPath} · ${subjectLabels[mode]}`;
    const text = await invoke("get_diff", { mode, paths: [entry.path] });
    setDiffMode(mode, false);
    renderDiff(text);
  } catch (error) {
    view.replaceChildren(el("div", "diff-empty", errorText(error)));
  }
}

function setDiffMode(mode, reload = true) {
  state.diffMode = mode;
  document.querySelectorAll(".diff-mode .tab").forEach((n) => {
    n.classList.toggle("active", n.dataset.mode === mode);
  });
  if (reload) {
    state.selectedFile = null;
    state.selectedCommit = null;
    loadDiff();
  }
}

function renderDiff(text) {
  const view = $("diff-view");
  if (!text.trim()) {
    view.replaceChildren(el("div", "diff-empty", "No differences."));
    return;
  }
  const files = parseUnifiedDiff(text);
  if (files.length === 0) {
    view.replaceChildren(el("div", "diff-empty", truncate(text)));
    return;
  }
  const target = hunkRollbackTarget();
  const fragment = document.createDocumentFragment();
  for (const file of files) {
    const card = el("div", "diff-file");
    const head = el("div", "diff-file-head", file.title);
    if (target && state.selectedFile) {
      const rollback = el("button", "btn small diff-rollback", "Rollback file");
      rollback.title = ROLLBACK_LABEL[target] || "Rollback";
      rollback.addEventListener("click", () =>
        void guardButton(rollback, () =>
          rollbackFiles([state.selectedFile.entry.path], target, state.selectedFile.entry.displayPath))
      );
      head.append(rollback);
    }
    card.append(head);
    for (const chunk of file.chunks) {
      const hunkHead = el("div", "diff-hunk");
      const label = el("span", "diff-hunk-text", chunk.header);
      hunkHead.append(label);
      if (target && (chunk.raw || []).length > 0) {
        const rollback = el("button", "hunk-rollback-btn", "↩");
        rollback.title =
          target === "staged"
            ? "Rollback hunk (unstage these changes)"
            : "Rollback hunk (discard these working tree changes)";
        rollback.addEventListener("click", () => void guardButton(rollback, () => rollbackHunk(file, chunk)));
        hunkHead.append(rollback);
      }
      card.append(hunkHead);
      if (state.diffLayout === "split") {
        card.append(renderChunkSplit(chunk));
      } else {
        const chunkNode = el("div");
        for (const line of chunk.lines) {
          chunkNode.append(el("div", line.class, line.text || " "));
        }
        card.append(chunkNode);
      }
    }
    fragment.append(card);
  }
  view.replaceChildren(fragment);
}

/* Side-by-side rendering: deleted and added runs are paired line-by-line so
 * the two columns stay in sync; missing sides stay blank. */
function splitPairs(lines) {
  const pairs = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (line.type === "del" || line.type === "add") {
      const dels = [];
      const adds = [];
      while (index < lines.length && (lines[index].type === "del" || lines[index].type === "add")) {
        (lines[index].type === "del" ? dels : adds).push(lines[index]);
        index++;
      }
      const rows = Math.max(dels.length, adds.length);
      for (let i = 0; i < rows; i++) {
        pairs.push({ left: dels[i] || null, right: adds[i] || null });
      }
    } else {
      pairs.push({ left: line, right: line });
      index++;
    }
  }
  return pairs;
}

function renderChunkSplit(chunk) {
  const node = el("div", "diff-split");
  for (const pair of splitPairs(chunk.lines)) {
    const row = el("div", "split-row");
    row.append(splitLine(pair.left, "split-left"));
    row.append(splitLine(pair.right, "split-right"));
    node.append(row);
  }
  return node;
}

function splitLine(line, className) {
  const cell = el("div", "split-cell " + className);
  if (!line) {
    cell.classList.add("blank");
    cell.textContent = " ";
    return cell;
  }
  const base = line.class.replace("diff-line", "").trim();
  cell.append(el("div", `diff-line ${base}`.trim() || "diff-line", line.text || " "));
  return cell;
}

/* Two-pane commit detail: metadata line plus the file list; clicking a file
 * shows only that file's patch in the diff view. */
function renderCommitDetail(detail) {
  const box = $("commit-detail");
  const meta = $("commit-meta");
  const parts = [
    el("span", "commit-detail-subject", detail.subject),
    el("span", "commit-meta", `${detail.author} · ${detail.date}`),
    el("span", "commit-detail-sha", detail.hash.slice(0, 10)),
  ];
  if (detail.refs && detail.refs.length > 0) {
    const refs = el("span", "commit-refs");
    for (const ref of detail.refs) {
      refs.append(el("span", `ref-badge ${refClass(ref)}`, prettyRef(ref)));
    }
    parts.push(refs);
  }
  if (detail.parents && detail.parents.length > 1) {
    parts.push(el("span", "commit-meta", "merge"));
  }
  meta.replaceChildren(...parts);

  const files = $("commit-files");
  files.replaceChildren();
  const label = el("div", "dropdown-head", `Files changed (${detail.files.length})`);
  files.append(label);
  for (const file of detail.files) {
    const row = el("div", "file-row");
    const letter = file.status[0] || "?";
    row.append(
      el("span", `file-kind ${statusClass(letter)}`, letter),
      el("span", "file-path", file.displayPath || file.path)
    );
    row.title = file.displayPath || file.path;
    row.addEventListener("click", () => {
      files.querySelectorAll(".file-row.selected").forEach((n) => n.classList.remove("selected"));
      row.classList.add("selected");
      state.commitFile = file.path;
      $("diff-subject").textContent = `${file.displayPath || file.path} · ${detail.hash.slice(0, 10)}`;
      void loadCommitFileDiff(file.path);
    });
    if (state.commitFile === file.path) row.classList.add("selected");
    files.append(row);
  }
  show(box);
}

async function loadCommitFileDiff(path) {
  const view = $("diff-view");
  try {
    setStatus("Loading diff…");
    const text = await invoke("commit_file_diff", { sha: state.selectedCommit, path });
    setStatus("Ready");
    renderDiff(text);
  } catch (error) {
    view.replaceChildren(el("div", "diff-empty", errorText(error)));
  }
}

function statusClass(letter) {
  if (letter === "M" || letter === "T") return "k-changed";
  if (letter === "A") return "k-added";
  if (letter === "D") return "k-deleted";
  if (letter.startsWith("R") || letter.startsWith("C")) return "k-renamed";
  return "k-muted";
}

function parseUnifiedDiff(text) {
  const lines = text.endsWith("\n") ? text.slice(0, -1).split("\n") : text.split("\n");
  const files = [];
  let current = null;
  let chunk = null;

  const startFile = (title, oldPath, newPath, line) => {
    current = {
      title: title || `${oldPath || ""} → ${newPath || ""}`,
      header: [line], // raw `diff --git …` fragment, reused for hunk patches
      chunks: [],
    };
    files.push(current);
    chunk = null;
  };
  const startChunk = (header) => {
    if (!current) startFile("diff", "", "", header);
    chunk = { header, raw: [header], lines: [] };
    current.chunks.push(chunk);
  };
  const push = (type, cls, content, rawLine) => {
    if (!chunk) {
      chunk = { header: "@@ … @@", raw: [], lines: [] };
      current.chunks.push(chunk);
    }
    if (rawLine !== undefined) chunk.raw.push(rawLine);
    chunk.lines.push({ type, class: cls, text: content });
  };
  const toHeader = (line) => {
    if (current) current.header.push(line);
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    // `git show` output begins with commit metadata and the (indented) message
    // before any `diff --git` line. Those message lines look like diff rows
    // (they start with spaces or +/-), so ignore everything until the first
    // real diff header. Plain `git diff` output starts with its own header.
    if (!current && !line.startsWith("diff --git ")) continue;
    if (line.startsWith("diff --git ")) {
      const target = line.slice("diff --git a/".length).replace(/ b\/.*$/, "");
      // Prefer the actual +++ header when it follows.
      let newPath = target;
      let oldPath = target;
      if (lines[i + 2] && lines[i + 2].startsWith("--- ")) oldPath = lines[i + 2].slice(4);
      if (lines[i + 3] && lines[i + 3].startsWith("+++ ")) newPath = lines[i + 3].slice(4);
      const name = shortPath(newPath !== "/dev/null" ? newPath : oldPath);
      const caption =
        oldPath !== newPath && oldPath !== "/dev/null" && newPath !== "/dev/null"
          ? `${shortPath(oldPath)} → ${name}`
          : name;
      startFile(caption, oldPath, newPath, line);
    } else if (line.startsWith("index ") || line.startsWith("old mode") ||
               line.startsWith("new mode") || line.startsWith("similarity ") ||
               line.startsWith("rename from") || line.startsWith("rename to") ||
               line.startsWith("new file mode") || line.startsWith("deleted file mode") ||
               line.startsWith("Binary files")) {
      toHeader(line);
      if (line.startsWith("Binary files")) push("meta", "diff-line", "\u2550 " + line);
      continue;
    } else if (line.startsWith("@@")) {
      startChunk(line);
    } else if (line.startsWith("--- ")) {
      toHeader(line);
    } else if (line.startsWith("+++ ")) {
      toHeader(line);
    } else if (line.startsWith("+")) {
      push("add", "diff-line added", line.slice(1), line);
    } else if (line.startsWith("-")) {
      push("del", "diff-line removed", line.slice(1), line);
    } else if (line.startsWith(" ") || line === "") {
      push("ctx", "diff-line", line.slice(1), line === "" ? "" : line);
    } else if (line.startsWith("\\")) {
      push("meta", "diff-line", line, line); // e.g. "\ No newline at end of file"
    }
  }
  return files;
}

function shortPath(path) {
  const clean = path.replace(/^(a|b)\//, "");
  return clean.startsWith("/dev/null") ? clean : clean.length > 60 ? "…" + clean.slice(-59) : clean;
}

/* -------------------------------------------------------------------- log */

async function refreshLog(reset) {
  if (!state.root) return;
  if (reset) state.logCount = LOG_PAGE;
  const filters = state.logFilters;
  const commits = await invoke("get_log", {
    count: state.logCount,
    skip: null,
    all: state.logAll,
    text: filters.text || null,
    author: filters.author || null,
    path: filters.path || null,
  });
  state.logCommits = commits;
  renderLog(commits);
  $("load-more-btn").classList.toggle("hidden", commits.length < state.logCount);
}

/* Fetch only the next page (`--skip`) and append its rows — the already
 * rendered commits are neither re-fetched nor re-rendered. */
async function loadMore() {
  const filters = state.logFilters;
  const page = await invoke("get_log", {
    count: LOG_PAGE,
    skip: state.logCount,
    all: state.logAll,
    text: filters.text || null,
    author: filters.author || null,
    path: filters.path || null,
  });
  if (page.length === 0) {
    $("load-more-btn").classList.add("hidden");
    return;
  }
  const from = state.logCommits.length;
  state.logCommits.push(...page);
  state.logCount = state.logCommits.length;
  renderLog(state.logCommits, from);
  $("load-more-btn").classList.toggle("hidden", page.length < LOG_PAGE);
}

function renderLog(commits, from = 0) {
  const list = $("log-list");
  if (from === 0) list.replaceChildren();
  if (commits.length === 0) {
    list.append(el("div", "no-commits", "No commits yet — make your first commit from Local Changes."));
    return;
  }
  const { rows: graph, maxLanes } = computeGraph(commits);
  const graphWidth = Math.max((maxLanes + 1) * LANE_W, LANE_W * 2);
  if (from > 0 && state.graphWidth !== graphWidth) {
    // The appended rows need more lanes than the ones already drawn —
    // redraw everything so the graph columns stay aligned.
    list.replaceChildren();
    from = 0;
  }
  state.graphWidth = graphWidth;
  const fragment = document.createDocumentFragment();
  for (let index = from; index < commits.length; index++) {
    const commit = commits[index];
    const row = graph[index];
    const node = el("div", "commit-row");

    const cell = el("span", "graph-cell");
    cell.style.width = graphWidth + "px";
    cell.append(drawGraphCell(row, graphWidth));
    node.append(cell);

    const sha = el("span", "commit-sha", commit.shortHash);
    node.append(sha);

    if (commit.refs.length > 0) {
      const refs = el("span", "commit-refs");
      for (const ref of commit.refs) {
        const badge = el("span", `ref-badge ${refClass(ref)}`, prettyRef(ref));
        badge.title = ref;
        refs.append(badge);
      }
      node.append(refs);
    }

    const subject = el("span", "commit-subject", commit.subject);
    const meta = el("span", "commit-meta", `${commit.author} · ${commit.date}`);
    node.append(subject, meta);

    node.addEventListener("click", () => selectCommit(commit, node));
    if (state.selectedCommit && state.selectedCommit === commit.hash) {
      node.classList.add("selected");
    }
    node.addEventListener("contextmenu", (event) => {
      const isMerge = (commit.parents || []).length > 1;
      const last = state.loadedStatus && state.loadedStatus.lastCommit;
      const isHead = Boolean(last && last.hash === commit.hash);
      openContextMenu(event, [
        { head: commit.subject },
        { label: "Show diff", hint: commit.shortHash, run: () => selectCommit(commit, node) },
        {
          label: "Checkout commit (detached HEAD)",
          hint: commit.shortHash,
          run: () => checkoutCommit(commit.hash, commit.subject),
        },
        { sep: true },
        ...(isMerge ? [] : [
          { label: "Cherry-pick commit", run: () =>
            runHistoryAction("Cherry-pick", () => invoke("cherry_pick", { sha: commit.hash })) },
          { label: "Revert commit", run: () =>
            runHistoryAction("Revert commit", () => invoke("revert_commit", { sha: commit.hash })) },
        ]),
        ...(isHead && !isMerge
          ? [{ label: "Undo commit (changes back to working tree)", run: () => confirmUndoCommit(commit) }]
          : []),
        { label: "Edit commit message…", run: () => rewordCommitFlow(commit) },
        ...(isMerge
          ? []
          : [{ label: "Squash into parent…", run: () => squashCommitFlow(commit) }]),
        { sep: true },
        ...(isMerge
          ? []
          : [{
              label: "Drop commit",
              danger: true,
              run: () => confirmDropCommit(commit),
            }]),
        {
          label: "Reset current branch to here…",
          danger: true,
          run: () => openResetDialog(commit),
        },
        { sep: true },
        { label: "Create tag here…", run: () => createTagAt(commit) },
        { label: "Copy commit hash", run: () => copyText(commit.hash) },
        { label: "Copy subject", run: () => copyText(commit.subject) },
      ]);
    });

    fragment.append(node);
  }
  list.append(fragment);
}

function selectCommit(commit, node) {
  document.querySelectorAll(".commit-row.selected").forEach((n) => n.classList.remove("selected"));
  node.classList.add("selected");
  state.selectedCommit = commit.hash;
  state.selectedFile = null;
  switchTab("diff");
  loadDiff();
}

/* Check out a commit directly (detached HEAD). The working tree moves to that
   commit; the Branch menu on the branch bar switches back to any branch. */
function checkoutCommit(sha, subject) {
  const head = subject ? `${sha.slice(0, 10)} — ${subject}` : sha.slice(0, 10);
  if (
    !confirm(
      `Check out commit ${head}?\n\nThe working tree moves to this commit and HEAD detaches.` +
        "\nNothing is lost: local changes are kept and no commits are deleted." +
        "\nUse the branch menu to switch back to a branch afterwards."
    )
  ) {
    return;
  }
  void runAction(`Checkout ${sha.slice(0, 10)}`, () => invoke("checkout_commit", { sha }));
}

function refClass(ref) {
  if (ref.startsWith("HEAD")) return "head";
  if (ref.startsWith("tag:")) return "tag";
  if (ref.includes("/")) return "remote";
  return "";
}

function prettyRef(ref) {
  return ref.startsWith("tag:") ? ref.slice(4).trim() : ref;
}

function drawGraphCell(row, width) {
  const CELL_H = 24;
  const RADIUS = 4;
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", width);
  svg.setAttribute("height", CELL_H);
  svg.style.verticalAlign = "middle";

  const cx = (lane) => lane * LANE_W + LANE_W / 2 + 2;

  // Straight-through edges.
  for (const lane of row.through) drawLine(svg, cx(lane), 0, cx(lane), CELL_H, row.colorOf(lane));
  // Incoming edges that converge into the node from other lanes.
  for (const from of row.incoming) {
    if (from === row.node) {
      // The edge to the parent below runs in the node's own lane: draw the
      // segment above the dot so the line is continuous through the row.
      drawLine(svg, cx(row.node), 0, cx(row.node), CELL_H / 2, row.colorOf(row.node));
    } else {
      drawCurve(svg, cx(from), 0, cx(row.node), CELL_H / 2, row.colorOf(row.node));
    }
  }
  // Outgoing edges to parents below.
  for (const to of row.outgoing) {
    if (to === row.node) {
      drawLine(svg, cx(row.node), CELL_H / 2, cx(row.node), CELL_H, row.colorOf(row.node));
    } else {
      drawCurve(svg, cx(row.node), CELL_H / 2, cx(to), CELL_H, colorForLane(to));
    }
  }

  const circle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
  circle.setAttribute("cx", cx(row.node));
  circle.setAttribute("cy", CELL_H / 2);
  circle.setAttribute("r", RADIUS);
  circle.setAttribute("fill", colorForLane(row.node));
  circle.setAttribute("stroke", "var(--panel)");
  svg.append(circle);
  return svg;

  function drawLine(svg, x1, y1, x2, y2, color) {
    const line = document.createElementNS("http://www.w3.org/2000/svg", "line");
    line.setAttribute("x1", x1); line.setAttribute("y1", y1);
    line.setAttribute("x2", x2); line.setAttribute("y2", y2);
    line.setAttribute("stroke", color);
    line.setAttribute("stroke-width", 2);
    svg.append(line);
  }
  function drawCurve(svg, x1, y1, x2, y2, color) {
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    const mid = (y1 + y2) / 2;
    path.setAttribute("d", `M ${x1} ${y1} C ${x1} ${mid}, ${x2} ${mid}, ${x2} ${y2}`);
    path.setAttribute("fill", "none");
    path.setAttribute("stroke", color);
    path.setAttribute("stroke-width", 2);
    svg.append(path);
  }
  function colorForLane(lane) {
    return LANE_COLORS[lane % LANE_COLORS.length];
  }
}

/* Lane allocation: children appear before parents in `git log`; every child
 * opens (or reuses) a lane for each pending parent. An edge stays open until
 * its parent's row consumes it. */
function computeGraph(commits) {
  const nodeLane = new Map();
  const open = []; // { target: parentHash, lane }
  const rows = [];

  for (const commit of commits) {
    const node = nodeLane.get(commit.hash) ?? firstFreeLane();
    nodeLane.set(commit.hash, node);

    const incomingLanes = [];
    const throughLanes = [];
    for (const edge of open) {
      if (edge.target === commit.hash) incomingLanes.push(edge.lane);
      else throughLanes.push(edge.lane);
    }
    // Consume the edges that ended here.
    for (let i = open.length - 1; i >= 0; i--) {
      if (open[i].target === commit.hash) open.splice(i, 1);
    }

    const colorOf = (lane) => LANE_COLORS[(lane ?? node) % LANE_COLORS.length];

    const outgoingLanes = [];
    for (const parent of commit.parents) {
      let lane = nodeLane.get(parent);
      if (lane === undefined) {
        lane = firstFreeLane();
        nodeLane.set(parent, lane);
        open.push({ target: parent, lane });
      }
      outgoingLanes.push(lane);
    }

    const lanesUsed = Math.max(node, ...incomingLanes, ...outgoingLanes, ...throughLanes, 0);
    rows.push({ node, incoming: incomingLanes, through: throughLanes, outgoing: outgoingLanes, lanesUsed, colorOf });
  }

  function firstFreeLane() {
    const taken = new Set(open.map((edge) => edge.lane));
    for (let lane = 0; ; lane++) if (!taken.has(lane)) return lane;
  }
  const maxLanes = rows.reduce((max, row) => Math.max(max, row.lanesUsed), 0);
  return { rows, maxLanes };
}

/* ------------------------------------------------------- history actions */

/* Generic prompt dialog used for any single-text-field operation. */
let promptHandler = null;
function promptDialog(title, placeholder, initial, mode, onSubmit) {
  const dialog = $("prompt-dialog");
  $("prompt-title").textContent = title;
  const input = $("prompt-input");
  input.value = initial || "";
  input.placeholder = placeholder || "";
  input.type = mode === "password" ? "password" : "text";
  $("prompt-error").textContent = "";
  promptHandler = async () => {
    const value = input.value.trim();
    if (!value) {
      $("prompt-error").textContent = "Enter a value first.";
      return;
    }
    try {
      await onSubmit(value);
      if (!$("prompt-error").textContent) hide(dialog);
    } catch (error) {
      $("prompt-error").textContent = errorText(error);
      setStatus(`${title} failed: ${errorText(error)}`, true);
    }
  };
  show(dialog);
  input.focus();
  input.select();
}

function closeDialogs() {
  // The merge editor confirms with the user while conflicts are unresolved.
  if (!$("merge-dialog").classList.contains("hidden")) {
    closeMergeEditor();
    return;
  }
  for (const id of [
    "pull-dialog", "push-dialog", "compare-dialog", "prompt-dialog",
    "incoming-dialog", "remotes-dialog", "message-dialog", "clone-dialog",
    "oplog-dialog",
  ]) {
    hide($(id));
  }
}

/* Runs a history-changing operation; after conflicts the tree guide tells
 * the user what to do (fix, stage, commit). */
async function runHistoryAction(label, fn) {
  await runAction(label, fn);
  const status = state.loadedStatus;
  if (status && status.conflicts.length > 0) {
    setStatus(`${label}: conflicts appeared. Edit the files, stage them (→), then commit.`, true);
  }
}

async function confirmUndoCommit(commit) {
  if (!confirm(`Undo commit ${commit.shortHash} ${commit.subject}?\n\n` +
      "The commit disappears from the current branch but its changes stay in the working tree as uncommitted edits.")) return;
  await runHistoryAction("Undo commit", () => invoke("undo_commit"));
}

async function confirmDropCommit(commit) {
  if (!confirm(`Drop commit ${commit.shortHash} ${commit.subject}?\n\n` +
      "This rewrites this commit out of the current branch history (git rebase --onto)." +
      "\nRequires a clean working tree; dropped commits stay reachable via reflog.")) return;
  await runHistoryAction("Drop commit", () => invoke("drop_commit", { sha: commit.hash }));
}

/* Multi-line variant of the prompt dialog, used for rewriting commit
   messages (squash and edit-message). Ctrl+Enter submits. */
let messageHandler = null;
function messageDialog(title, initial, onSubmit) {
  const dialog = $("message-dialog");
  $("message-title").textContent = title;
  const input = $("message-input");
  input.value = initial || "";
  $("message-error").textContent = "";
  messageHandler = async () => {
    const value = input.value;
    if (!value.trim()) {
      $("message-error").textContent = "Enter a message first.";
      return;
    }
    try {
      await onSubmit(value);
      if (!$("message-error").textContent) hide(dialog);
    } catch (error) {
      $("message-error").textContent = errorText(error);
      setStatus(`${title} failed: ${errorText(error)}`, true);
    }
  };
  show(dialog);
  input.focus();
}

/* Reword: keep the commit's tree/parents, replace its message. Works for any
   commit on the current branch, merge commits included. */
async function rewordCommitFlow(commit) {
  try {
    const current = await invoke("commit_full_message", { sha: commit.hash });
    messageDialog(`Edit message of ${commit.shortHash}`, String(current || "").trimEnd(), async (message) => {
      await runHistoryAction(`Reword ${commit.shortHash}`, () =>
        invoke("reword_commit", { sha: commit.hash, message }));
    });
  } catch (error) {
    setStatus(`Edit message failed: ${errorText(error)}`, true);
  }
}

/* Squash: fold the commit into its parent (parent keeps its author, gets the
   combined message and the commit's tree). The branch history is rewritten,
   so a later push needs force. */
async function squashCommitFlow(commit) {
  let preview;
  try {
    preview = await invoke("squash_preview", { sha: commit.hash });
  } catch (error) {
    setStatus(`Squash failed: ${errorText(error)}`, true);
    return;
  }
  messageDialog(`Squash ${commit.shortHash} into its parent`, String(preview || "").trimEnd(), async (message) => {
    if (!confirm(
      `Squash ${commit.shortHash} (${commit.subject}) into its parent?\n\n` +
      "The branch history is rewritten; descendant commits are replayed on top.\n" +
      "A later push to the upstream will need a force push."
    )) return;
    await runHistoryAction(`Squash ${commit.shortHash}`, () =>
      invoke("squash_commit", { sha: commit.hash, message }));
  });
}

/* Reset current branch to a commit: soft keeps index+worktree changes, mixed
 * keeps files but unstages them, hard discards everything. */
function openResetDialog(commit) {
  const items = [
    { head: `Reset branch (→ ${commit.shortHash})` },
    { label: "Soft — keep all changes staged", run: () => doReset(commit, "soft") },
    { label: "Mixed — keep changes, unstage", run: () => doReset(commit, "mixed") },
    { sep: true },
    { label: "Hard — delete uncommitted changes", danger: true, hint: "irreversible", run: () => doReset(commit, "hard") },
  ];
  const event = lastMouseEvent;
  if (event) openContextMenu(event, items);
}

let lastMouseEvent = null;
document.addEventListener("contextmenu", (event) => {
  lastMouseEvent = event;
}, true);

async function doReset(commit, mode) {
  const warnings = {
    soft: "",
    mixed: "\nUncommitted files stay in the working tree.",
    hard: "\nAll uncommitted changes in index and working tree will be deleted!",
  };
  const label = { soft: "Soft reset", mixed: "Mixed reset", hard: "Hard reset" }[mode];
  if (!confirm(`${label} current branch to ${commit.shortHash} (${commit.subject})?${warnings[mode]}`)) return;
  await runHistoryAction(label, () => invoke("reset_branch", { sha: commit.hash, mode }));
}

async function createTagAt(commit) {
  promptDialog("Create tag at " + commit.shortHash, "Tag name, e.g. v1.0", "", "", async (name) => {
    await runAction(`Tag ${name}`, () => invoke("create_tag", { name, rev: commit.hash }));
    await refreshTags();
    await refreshLog(false);
  });
}

/* ------------------------------------------------------------- merge editor */

const MERGE_MARKER = {
  start: /^<{7}/,
  base: /^\|{7}/,
  sep: /^\={7}/,
  end: /^>{7}/,
};

/* Splits worktree content into segments from the merge markers:
   {kind:"common", lines} and {kind:"conflict", ours[], base[], theirs[], choice}. */
function parseConflictSegments(content) {
  const lines = content.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  const segments = [];
  let common = null;
  let block = null;
  const flush = () => {
    if (common) {
      segments.push({ kind: "common", lines: common });
      common = null;
    }
  };
  for (const line of lines) {
    if (block) {
      if (block.side === "ours") {
        if (MERGE_MARKER.base.test(line)) block.side = "base";
        else if (MERGE_MARKER.sep.test(line)) block.side = "theirs";
        else block.ours.push(line);
      } else if (block.side === "base") {
        if (MERGE_MARKER.sep.test(line)) block.side = "theirs";
        else block.base.push(line);
      } else if (MERGE_MARKER.end.test(line)) {
        segments.push({ kind: "conflict", ours: block.ours, base: block.base, theirs: block.theirs, choice: null });
        block = null;
      } else {
        block.theirs.push(line);
      }
    } else if (MERGE_MARKER.start.test(line)) {
      flush();
      block = { ours: [], base: [], theirs: [], side: "ours" };
    } else {
      (common = common || []).push(line);
    }
  }
  // Git cuts off unterminated markers (e.g. checkout -m edge cases); keep
  // what we parsed rather than losing the content.
  if (block) {
    segments.push({ kind: "conflict", ours: block.ours, base: block.base, theirs: block.theirs, choice: null });
  }
  flush();
  return segments;
}

function mergeSegmentLines(segment) {
  if (segment.kind === "common") return segment.lines;
  if (!segment.choice) return null;
  return segment.choice === "ours" ? segment.ours : segment.theirs;
}

function mergeUnresolved() {
  if (!state.mergeSegments) return 0;
  return state.mergeSegments.filter((s) => s.kind === "conflict" && !s.choice).length;
}

/* The result being applied: fully joined when every conflict is resolved,
   null while any is left open. */
function mergeResolvedContent() {
  if (!state.mergeSegments) return null;
  const lines = [];
  for (const segment of state.mergeSegments) {
    const scoped = mergeSegmentLines(segment);
    if (scoped === null) return null;
    lines.push(...scoped);
  }
  const text = lines.join("\n");
  return state.mergeTrailingNewline ? text + "\n" : text;
}

function openMergeEditor(entry) {
  invoke("get_worktree_file", { file: entry.path })
    .then((content) => {
      state.mergeFile = entry;
      state.mergeTrailingNewline = content.endsWith("\n");
      state.mergeSegments = parseConflictSegments(content);
      $("merge-path").textContent = entry.displayPath;
      $("merge-error").textContent = "";
      renderMergeEditor();
      show($("merge-dialog"));
    })
    .catch((error) => setStatus(`Open merge editor failed: ${errorText(error)}`, true));
}

function mergeLine(num, text, cls) {
  const line = el("div", "merge-line" + (cls ? ` ${cls}` : ""));
  line.append(el("span", "merge-num", String(num)), el("span", "merge-text", text));
  return line;
}

function mergeButton(label, run) {
  const button = el("button", "btn small", label);
  button.addEventListener("click", () => void guardButton(button, run));
  return button;
}

/* One grid row per segment keeps all three panes aligned; scrolling the
   body scrolls them together, like JetBrains' merged-result layout. */
function renderMergeEditor() {
  const body = $("merge-body");
  body.replaceChildren();
  let oursNo = 1;
  let theirsNo = 1;
  let resultNo = 1;
  for (const segment of state.mergeSegments) {
    const row = el("div", "merge-row");
    const left = el("div", "merge-cell");
    const right = el("div", "merge-cell");
    const result = el("div", "merge-cell merge-result");
    if (segment.kind === "common") {
      for (const text of segment.lines) {
        left.append(mergeLine(oursNo++, text, "merge-ctx"));
        right.append(mergeLine(theirsNo++, text, "merge-ctx"));
        result.append(mergeLine(resultNo++, text, ""));
      }
    } else {
      const resolved = mergeSegmentLines(segment);
      for (const text of segment.ours) left.append(mergeLine(oursNo++, text, "merge-ours"));
      for (const text of segment.theirs) right.append(mergeLine(theirsNo++, text, "merge-theirs"));
      if (resolved === null) {
        const bar = el("div", "merge-resolve");
        bar.append(
          el("span", "merge-resolve-hint", "unresolved"),
          mergeButton("◀ Accept ours", () => chooseSegment(segment, "ours")),
          mergeButton("Accept theirs ▶", () => chooseSegment(segment, "theirs"))
        );
        result.append(bar);
        result.append(el("div", "merge-unresolved", "Choose a version to place it here."));
      } else {
        for (const text of resolved) {
          result.append(mergeLine(resultNo++, text, `merge-chosen ${segment.choice}`));
        }
        const undo = mergeButton("⟲", () => chooseSegment(segment, null));
        undo.classList.add("merge-undo");
        undo.title = `Reset choice (currently ${segment.choice})`;
        result.append(undo);
      }
    }
    row.append(left, right, result);
    body.append(row);
  }
  const unresolved = mergeUnresolved();
  const pill = $("merge-count");
  pill.textContent = unresolved === 0 ? "all conflict(s) resolved" : `${unresolved} unresolved`;
  pill.classList.toggle("ok", unresolved === 0);
  $("merge-apply").disabled = unresolved > 0;
}

function chooseSegment(segment, choice) {
  segment.choice = choice;
  renderMergeEditor();
}

function acceptAllMerge(choice) {
  if (!state.mergeSegments) return;
  for (const segment of state.mergeSegments) {
    if (segment.kind === "conflict") segment.choice = choice;
  }
  renderMergeEditor();
}

function closeMergeEditor() {
  if ($("merge-dialog").classList.contains("hidden")) return;
  const unresolved = mergeUnresolved();
  if (unresolved > 0 &&
      !confirm(`Leave the merge editor?\n\n${unresolved} conflict(s) unresolved — nothing has been written or staged yet.`)) {
    return;
  }
  hide($("merge-dialog"));
  state.mergeFile = null;
  state.mergeSegments = null;
}

async function applyMergeResult() {
  const content = mergeResolvedContent();
  if (content === null) {
    $("merge-error").textContent = "Conflicts remain unresolved — accept a version for every block first.";
    return;
  }
  const entry = state.mergeFile;
  try {
    await invoke("write_merge_result", { file: entry.path, content });
    hide($("merge-dialog"));
    state.mergeFile = null;
    state.mergeSegments = null;
    setStatus(`Resolved ${entry.displayPath} — ready to commit.`);
    await Promise.all([refreshStatus(), refreshLog(false), refreshBranches()]);
  } catch (error) {
    $("merge-error").textContent = errorText(error);
    setStatus(`Apply merge failed: ${errorText(error)}`, true);
  }
}

/* ------------------------------------------------------ merge / branch ops */

async function doMergeIntoCurrent(name, noFF) {
  const target = `Merge branch '${name}'`;
  if (!confirm(`Merge ${name} into the current branch?\n\n` +
      "Merge conflicts appear under Local Changes → Conflicts; resolve them, stage and commit.\n" +
      "You can abort the merge while resolving.")) return;
  setStatus("Merging…");
  try {
    const output = await invoke("merge_branch", { branch: name, noFF: Boolean(noFF) });
    setStatus(truncate(String(output || "Merge complete"), 3));
  } catch (error) {
    const message = errorText(error);
    const conflict = message.includes("CONFLICT") || message.includes("conflict");
    if (conflict) {
      $("commit-message").value = target;
      setStatus(`Merging ${name}: conflicts detected — resolve them under Conflicts, stage, then commit.`, true);
    } else {
      setStatus(`Merge failed: ${message}`, true);
    }
  }
  await Promise.all([refreshStatus(), refreshBranches()]);
}

async function deleteBranch(branch, force) {
  if (branch.remote) {
    if (!confirm(`Delete remote branch ${branch.name}?\n\nThis runs git push --delete on the remote.`)) return;
    await runAction(`Delete ${branch.name}`, () =>
      invoke("delete_remote_branch", { name: branch.name }));
    await refreshBranches();
    return;
  }
  if (branch.current) {
    setStatus("Cannot delete the current branch — switch first.", true);
    return;
  }
  const question = `Delete branch ${branch.name}?`;
  if (!confirm(question)) return;
  try {
    const output = await invoke("delete_branch", { name: branch.name, force: Boolean(force) });
    setStatus(truncate(String(output || "Deleted"), 3));
  } catch (error) {
    const message = errorText(error);
    if (!force && message.includes("not fully merged")) {
      if (confirm(`${branch.name} is not fully merged.\nDelete it anyway (force)?`)) {
        try {
          const output = await invoke("delete_branch", { name: branch.name, force: true });
          setStatus(truncate(String(output || "Deleted"), 3));
        } catch (forcedError) {
          setStatus(`Delete failed: ${errorText(forcedError)}`, true);
        }
      }
    } else {
      setStatus(`Delete failed: ${message}`, true);
    }
  }
  await refreshBranches();
}

async function renameBranchFlow(branch) {
  promptDialog(`Rename ${branch.name}`, "New branch name", branch.name, "", async (name) => {
    if (name === branch.name) return;
    await runAction(`Rename ${branch.name}`, () =>
      invoke("rename_branch", { oldName: branch.name, newName: name }));
    await refreshBranches();
  });
}

async function newBranchFrom(branch) {
  const suggested = branch.remote ? branch.name.replace(/^[^/]+\//, "") : branch.name + "-from";
  promptDialog(`New branch from ${branch.name}`, "Branch name", suggested, "", async (name) => {
    await runAction(`New branch ${name}`, () =>
      invoke("checkout_new_branch", { name, fromRef: branch.name }));
    await refreshBranches();
  });
}

async function setUpstreamFlow(branch) {
  promptDialog(
    `Set upstream for ${branch.name}`,
    "Upstream, e.g. origin/" + branch.name,
    branch.upstream || "origin/" + branch.name,
    "",
    async (upstream) => {
      await runAction(`Set upstream ${upstream}`, () =>
        invoke("set_upstream", { branch: branch.remote ? null : branch.name, upstream }));
      await refreshBranches();
    }
  );
}

async function unsetUpstreamFlow(branch) {
  await runAction(`Unset upstream of ${branch.name}`, () =>
    invoke("unset_upstream", { branch: branch.remote ? null : branch.name }));
  await refreshBranches();
}

async function compareBranchFlow(branch) {
  try {
    setStatus("Comparing…");
    const cmp = await invoke("compare_branch", { branch: branch.name });
    $("compare-name").textContent = branch.name;
    $("compare-name-2").textContent = branch.name;
    const ours = $("compare-ours");
    const theirs = $("compare-theirs");
    ours.replaceChildren();
    theirs.replaceChildren();
    const fill = (node, commits, emptyText) => {
      if (!commits || commits.length === 0) {
        node.append(el("div", "empty-hint", emptyText));
        return;
      }
      for (const commit of commits) {
        const row = el("div", "file-row");
        row.append(
          el("span", "commit-sha", commit.shortHash),
          el("span", "file-path", commit.subject),
          el("span", "commit-meta", commit.date)
        );
        row.title = commit.hash;
        node.append(row);
      }
    };
    fill(ours, cmp.ours, "No differences");
    fill(theirs, cmp.theirs, "No differences");
    show($("compare-dialog"));
    setStatus("Ready");
  } catch (error) {
    setStatus(`Compare failed: ${errorText(error)}`, true);
  }
}

/* ---------------------------------------------------------------- push / pull dialogs */

async function openPushDialog() {
  const dialog = $("push-dialog");
  $("push-remote").value = "origin";
  $("push-branch").value = "";
  $("push-force").checked = false;
  $("push-upstream").checked = false;
  $("push-error").textContent = "";
  const list = $("push-commits");
  list.replaceChildren(el("div", "empty-hint", "Loading commits…"));
  show(dialog);
  try {
    const { commits, hasUpstream } = await invoke("outgoing");
    list.replaceChildren();
    if (!hasUpstream) {
      list.append(el("div", "empty-hint", "No upstream — pushing with -u creates remote tracking."));
    }
    if (commits.length === 0) {
      list.append(el("div", "empty-hint", "Everything already pushed ✔"));
    } else {
      for (const commit of commits) {
        const row = el("div", "file-row");
        row.append(
          el("span", "commit-sha", commit.shortHash),
          el("span", "file-path", commit.subject),
          el("span", "commit-meta", commit.date)
        );
        list.append(row);
      }
    }
  } catch (error) {
    $("push-error").textContent = errorText(error);
  }
}

async function submitPushDialog() {
  const remote = $("push-remote").value.trim();
  const branch = $("push-branch").value.trim();
  $("push-error").textContent = "";
  try {
    const output = await invoke("push_options", {
      remote: remote || null,
      branch: branch || null,
      force: $("push-force").checked,
      setUpstream: $("push-upstream").checked,
    });
    hide($("push-dialog"));
    setStatus(truncate(String(output || "Push complete"), 3));
    await Promise.all([refreshStatus(), refreshBranches()]);
  } catch (error) {
    $("push-error").textContent = errorText(error);
  }
}

function openPullDialog() {
  $("pull-remote").value = "";
  $("pull-branch").value = "";
  $("pull-error").textContent = "";
  show($("pull-dialog"));
}

/* --------------------------------------------------------- incoming dialog */

/* Commits the upstream has that the current branch does not: exactly what a
   pull would bring in (HEAD..@{upstream}). */
async function openIncomingDialog() {
  const dialog = $("incoming-dialog");
  const list = $("incoming-commits");
  $("incoming-error").textContent = "";
  $("incoming-count").textContent = "";
  list.replaceChildren(el("div", "empty-hint", "Loading commits…"));
  show(dialog);
  try {
    const { commits, hasUpstream } = await invoke("incoming");
    list.replaceChildren();
    if (!hasUpstream) {
      list.append(el("div", "empty-hint", "The current branch has no upstream to fetch from."));
      $("incoming-pull").disabled = true;
      return;
    }
    $("incoming-pull").disabled = false;
    if (commits.length === 0) {
      list.append(el("div", "empty-hint", "Everything is up to date ✔"));
      return;
    }
    $("incoming-count").textContent = `(${commits.length})`;
    for (const commit of commits) {
      const row = el("div", "file-row");
      row.append(
        el("span", "commit-sha", commit.shortHash),
        el("span", "file-path", commit.subject),
        el("span", "commit-meta", `${commit.author} · ${commit.date}`)
      );
      row.title = commit.hash;
      list.append(row);
    }
  } catch (error) {
    list.replaceChildren();
    $("incoming-error").textContent = errorText(error);
  }
}

/* ---------------------------------------------------------- remotes dialog */

async function openRemotesDialog() {
  $("remotes-error").textContent = "";
  show($("remotes-dialog"));
  await refreshRemotes();
}

async function refreshRemotes() {
  if (!state.root) return;
  const list = $("remotes-list");
  list.replaceChildren();
  let remotes = [];
  try {
    remotes = await invoke("list_remotes");
  } catch (error) {
    $("remotes-error").textContent = errorText(error);
    return;
  }
  if (remotes.length === 0) {
    list.append(el("div", "empty-hint", "No remotes configured"));
    return;
  }
  for (const remote of remotes) {
    const row = el("div", "file-row");
    row.append(
      el("span", "file-kind k-remote", "⇅"),
      el("span", "file-path", remote.name),
      el("span", "commit-meta remote-url", remote.fetchUrl)
    );
    row.title = `${remote.name} → fetch: ${remote.fetchUrl} · push: ${remote.pushUrl}`;

    const edit = el("span", "file-action", "edit");
    edit.title = "Change the remote URL";
    edit.addEventListener("click", (event) => {
      event.stopPropagation();
      promptDialog(`Edit remote ${remote.name}`, "Remote URL", remote.fetchUrl, "", async (url) => {
        await invoke("set_remote_url", { name: remote.name, url });
        setStatus(`Remote ${remote.name} now points to ${url}`);
        await refreshRemotes();
      });
    });

    const fetchOne = el("span", "file-action", "fetch");
    fetchOne.title = `Fetch ${remote.name}`;
    fetchOne.addEventListener("click", (event) => {
      event.stopPropagation();
      void runAction(`Fetch ${remote.name}`, () => invoke("fetch_remote", { name: remote.name }), true, fetchOne);
    });

    const remove = el("span", "file-action", "delete");
    remove.title = "Remove this remote";
    remove.addEventListener("click", async (event) => {
      event.stopPropagation();
      if (!confirm(`Remove remote ${remote.name}?\n\nIts remote-tracking branches (origin/…) are removed too. The remote itself keeps its data.`)) return;
      await runAction(`Remove remote ${remote.name}`, () => invoke("remove_remote", { name: remote.name }), true, remove);
      await refreshRemotes();
    });

    row.append(edit, fetchOne, remove);
    list.append(row);
  }
}

async function submitAddRemote() {
  const name = $("remote-name").value.trim();
  const url = $("remote-url").value.trim();
  $("remotes-error").textContent = "";
  if (!name || !url) {
    $("remotes-error").textContent = "Both a name and a URL are required.";
    return;
  }
  try {
    await invoke("add_remote", { name, url });
    $("remote-name").value = "";
    $("remote-url").value = "";
    setStatus(`Remote ${name} added`);
    await refreshRemotes();
  } catch (error) {
    $("remotes-error").textContent = errorText(error);
  }
}

async function submitPullDialog() {
  const strategy = document.querySelector('input[name="pull-strategy"]:checked');
  const ffOnly = strategy ? strategy.value === "ff" : false;
  $("pull-error").textContent = "";
  try {
    const output = await invoke("pull_options", {
      remote: $("pull-remote").value.trim() || null,
      branch: $("pull-branch").value.trim() || null,
      ffOnly,
    });
    hide($("pull-dialog"));
    setStatus(truncate(String(output || "Pull complete"), 3));
    await Promise.all([refreshStatus(), refreshLog(false), refreshBranches()]);
  } catch (error) {
    $("pull-error").textContent = errorText(error);
  }
}

/* --------------------------------------------------------------- tags menu */

async function refreshTags() {
  if (!state.root) return;
  const tags = await invoke("get_tags");
  const list = $("tags-list");
  list.replaceChildren();
  if (tags.length === 0) {
    list.append(el("div", "empty-hint", "No tags"));
    return;
  }
  for (const tag of tags) {
    const row = el("div", "file-row");
    row.append(
      el("span", "file-kind k-changed", "◆"),
      el("span", "file-path", tag.name),
      el("span", "commit-meta", `${tag.hash} ${tag.date}`.trim())
    );
    row.title = `${tag.name} → ${tag.hash}`;
    const deleteBtn = el("span", "file-action", "delete");
    deleteBtn.title = "Delete tag";
    deleteBtn.addEventListener("click", async (event) => {
      event.stopPropagation();
      if (!confirm(`Delete tag ${tag.name}?`)) return;
      await runAction(`Delete tag ${tag.name}`, () => invoke("delete_tag", { name: tag.name }), true, deleteBtn);
      await refreshTags();
      await refreshLog(false);
    });
    row.append(deleteBtn);
    list.append(row);
  }
}

/* --------------------------------------------------------------- branches */

async function refreshBranches() {
  if (!state.root) return;
  const branches = await invoke("get_branches");
  renderBranchList($("local-branches"), branches.local);
  renderBranchList($("remote-branches"), branches.remote);
}

function renderBranchList(container, branches) {
  container.replaceChildren();
  if (branches.length === 0) {
    container.append(el("div", "empty-hint", "none"));
    return;
  }
  for (const branch of branches) {
    const row = el("div", "file-row");
    const marker = el("span", `file-kind ${branch.current ? "k-added" : ""}`, branch.current ? "●" : "○");
    const label = el("span", "file-path", branch.name);
    row.append(marker, label);
    const info = [];
    if (branch.upstream) info.push(branch.upstream);
    if (branch.ahead) info.push(`↑${branch.ahead}`);
    if (branch.behind) info.push(`↓${branch.behind}`);
    if (branch.gone) info.push("gone");
    if (info.length > 0) {
      row.append(el("span", "commit-meta", info.join(" ")));
    }
    if (!branch.current && !branch.remote) {
      const action = el("span", "file-action", "switch");
      action.addEventListener("click", (event) => {
        event.stopPropagation();
        runAction(`Switch to ${branch.name}`, () => invoke("switch_branch", { branch: branch.name }), true, action)
          .then(() => hide($('branch-menu')));
      });
      row.append(action);
    } else if (branch.remote) {
      action2(row, branch);
    }
    row.addEventListener("click", () => {
      if (!branch.current && !branch.remote) {
        void guardButton(row, () =>
          runAction(`Switch to ${branch.name}`, () => invoke("switch_branch", { branch: branch.name })))
          .then(() => hide($('branch-menu')));
      }
    });
    row.addEventListener("contextmenu", (event) => {
      const items = [{ head: branch.name }];
      if (!branch.current && !branch.remote) {
        items.push({ label: `Switch to ${branch.name}`, hint: branch.hash, run: () =>
          runAction(`Switch to ${branch.name}`, () => invoke("switch_branch", { branch: branch.name }))
            .then(() => hide($("branch-menu"))) });
        items.push({ label: "Merge into Current", run: () => {
          hide($("branch-menu"));
          void doMergeIntoCurrent(branch.name, false);
        } });
        items.push({ label: "Checkout as New Branch from here…", run: () => {
          hide($("branch-menu"));
          void newBranchFrom(branch);
        } });
      }
      if (branch.remote) {
        items.push({ label: "Checkout as New Local Branch…", run: () => {
          hide($("branch-menu"));
          void newBranchFrom(branch);
        } });
        items.push({ label: "Merge into Current", run: () => {
          hide($("branch-menu"));
          void doMergeIntoCurrent(branch.name, false);
        } });
      }
      items.push({ label: "Compare with Current", run: () => {
        hide($("branch-menu"));
        void compareBranchFlow(branch);
      } });
      if (!branch.remote && branch.upstream) {
        items.push({ sep: true });
        items.push({ label: "Set upstream…", hint: branch.upstream, run: () => {
          hide($("branch-menu"));
          void setUpstreamFlow(branch);
        } });
        items.push({ label: "Stop tracking upstream", run: () => {
          hide($("branch-menu"));
          void unsetUpstreamFlow(branch);
        } });
      } else if (!branch.remote && !branch.current) {
        items.push({ sep: true });
        items.push({ label: "Set upstream…", run: () => {
          hide($("branch-menu"));
          void setUpstreamFlow(branch);
        } });
      }
      items.push({ sep: true });
      if (!branch.remote && !branch.current) {
        items.push({ label: "Rename…", run: () => {
          hide($("branch-menu"));
          void renameBranchFlow(branch);
        } });
      }
      if (!(branch.current && !branch.remote) && !(branch.remote && branch.name.endsWith("/HEAD"))) {
        items.push({
          label: branch.remote ? "Delete remote branch…" : "Delete…",
          danger: true,
          run: () => {
            hide($("branch-menu"));
            void deleteBranch(branch, false);
          },
        });
      }
      if (branch.upstream) items.push({ label: "Copy upstream", run: () => copyText(branch.upstream) });
      items.push({ label: "Copy branch name", run: () => copyText(branch.name) });
      openContextMenu(event, items);
    });
    container.append(row);
  }
}

function action2(row, branch) {
  const action = el("span", "file-action", "…");
  action.title = "Copy branch name";
  action.addEventListener("click", (event) => {
    event.stopPropagation();
    setStatus(branch.name);
  });
  row.append(action);
}

/* ------------------------------------------------------------- op log */

/* Viewer for the last 1000 git commands Git Jet ran (this session only). */
async function openOpLog() {
  try {
    renderOpLog(await invoke("git_op_log"));
    show($("oplog-dialog"));
  } catch (error) {
    setStatus(`Op log failed: ${errorText(error)}`, true);
  }
}

function renderOpLog(lines) {
  $("oplog-count").textContent = lines.length ? `${lines.length} line(s)` : "";
  const list = $("oplog-list");
  list.replaceChildren();
  if (lines.length === 0) {
    list.append(el("div", "empty-hint", "No git operations yet — everything Git Jet runs shows up here."));
    return;
  }
  const fragment = document.createDocumentFragment();
  for (let index = lines.length - 1; index >= 0; index--) {
    fragment.append(el("div", "oplog-line", lines[index]));
  }
  list.append(fragment);
}

async function clearOpLog() {
  try {
    await invoke("clear_git_op_log");
    renderOpLog([]);
    setStatus("Operation log cleared");
  } catch (error) {
    setStatus(`Clear failed: ${errorText(error)}`, true);
  }
}

/* ----------------------------------------------------------------- stashes */
async function refreshStashes() {
  if (!state.root) return;
  const stashes = await invoke("get_stashes");
  const list = $("stash-list");
  list.replaceChildren();
  if (stashes.length === 0) {
    list.append(el("div", "empty-hint", "No stashes"));
    return;
  }
  for (const stash of stashes) {
    const row = el("div", "file-row");
    const label = el("span", "file-path");
    label.append(
      el("span", "muted", stash.selector + " "),
      stash.message.length > 40 ? stash.message.slice(0, 40) + "…" : stash.message
    );
    row.append(label, el("span", "commit-meta", stash.date || ""));
    const wrap = el("span");
    for (const [name, action] of [["pop", "pop"], ["apply", "apply"], ["drop", "drop"]]) {
      const btn = el("span", "file-action", name);
      btn.addEventListener("click", (event) => {
        event.stopPropagation();
        runAction(`Stash ${name}`, () =>
          invoke("stash_apply", { action: name, selector: stash.selector }), true, btn
        );
      });
      wrap.append(btn);
    }
    row.append(wrap);
    row.addEventListener("contextmenu", (event) => {
      const action = (name) => () =>
        runAction(`Stash ${name}`, () =>
          invoke("stash_apply", { action: name, selector: stash.selector }));
      openContextMenu(event, [
        { head: stash.message || `${stash.selector} stash` },
        { label: "Pop stash", run: action("pop") },
        { label: "Apply stash", run: action("apply") },
        { sep: true },
        { label: "Drop stash", danger: true, run: action("drop") },
        { sep: true },
        { label: "Copy message", run: () => copyText(stash.message) },
      ]);
    });
    list.append(row);
  }
}

/* ------------------------------------------------------------------- tabs */

function switchTab(name) {
  document.querySelectorAll(".tabs .tab").forEach((node) => {
    node.classList.toggle("active", node.dataset.tab === name);
  });
  $("tab-log").classList.toggle("hidden", name !== "log");
  $("tab-diff").classList.toggle("hidden", name !== "diff");
  if (name === "diff" && !state.selectedCommit && !state.selectedFile) loadDiff();
}

/* -------------------------------------------------------------- lifecycle */

async function refreshAll() {
  if (!state.root) return;
  setStatus("Refreshing…");
  try {
    await Promise.all([refreshStatus(), refreshLog(true), refreshBranches(), refreshStashes()]);
    await refreshTags().catch(() => {});
    await loadDiff().catch(() => {});
    setStatus("Ready");
  } catch (error) {
    const text = errorText(error);
    if (text.includes("No repository is open") || text.includes("Not a Git repository")) {
      leaveApp("The repository is no longer available: " + text);
    } else {
      setStatus(text, true);
    }
  }
}

function boot() {
  $("app").classList.add("hidden");
  if (!tauri()) {
    leaveApp("The Tauri API is not available — run Git Jet through its desktop app, not a browser.");
    return;
  }

  $("open-folder-btn").addEventListener("click", browseForRepository);
  $("clone-open-btn").addEventListener("click", openCloneDialog);
  $("open-path-btn").addEventListener("click", () => {
    const value = $("repo-path").value.trim();
    if (value) openRepository(expandHome(value));
  });
  $("repo-path").addEventListener("keydown", (event) => {
    if (event.key === "Enter") $("open-path-btn").click();
  });

  $("refresh-btn").addEventListener("click", () => void guardButton($("refresh-btn"), refreshAll));
  $("switch-repo-btn").addEventListener("click", () =>
    runAction("Close", () => invoke("close_repository"), false, $("switch-repo-btn")).then(leaveApp.bind(null, ""))
  );

  $("fetch-btn").addEventListener("click", () =>
    runAction("Fetch", () => invoke("fetch", { all: true }), true, $("fetch-btn"))
  );
  $("pull-btn").addEventListener("click", openPullDialog);
  $("push-btn").addEventListener("click", openPushDialog);

  $("pull-cancel").addEventListener("click", () => hide($("pull-dialog")));
  $("pull-ok").addEventListener("click", () => void guardButton($("pull-ok"), submitPullDialog));
  $("push-cancel").addEventListener("click", () => hide($("push-dialog")));
  $("push-ok").addEventListener("click", () => void guardButton($("push-ok"), submitPushDialog));
  $("compare-close").addEventListener("click", () => hide($("compare-dialog")));
  $("prompt-cancel").addEventListener("click", () => hide($("prompt-dialog")));

  /* incoming commits */
  $("incoming-close").addEventListener("click", () => hide($("incoming-dialog")));
  $("incoming-pull").addEventListener("click", () => {
    hide($("incoming-dialog"));
    openPullDialog();
  });

  /* remotes */
  $("remotes-btn").addEventListener("click", () => void openRemotesDialog());
  $("remotes-close").addEventListener("click", () => hide($("remotes-dialog")));
  $("oplog-btn").addEventListener("click", () => void openOpLog());
  $("oplog-close").addEventListener("click", () => hide($("oplog-dialog")));
  $("oplog-clear").addEventListener("click", () => void clearOpLog());
  $("remote-add").addEventListener("click", () => void guardButton($("remote-add"), submitAddRemote));
  for (const id of ["remote-name", "remote-url"]) {
    $(id).addEventListener("keydown", (event) => {
      if (event.key === "Enter") $("remote-add").click();
    });
  }

  /* commit message editor (squash / reword) */
  $("message-cancel").addEventListener("click", () => hide($("message-dialog")));
  $("message-ok").addEventListener("click", () => {
    const handler = messageHandler;
    if (handler) void guardButton($("message-ok"), handler);
  });
  $("message-input").addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      const handler = messageHandler;
      if (handler) void guardButton($("message-ok"), handler);
    }
  });

  /* clone */
  $("clone-cancel").addEventListener("click", () => hide($("clone-dialog")));
  $("clone-browse").addEventListener("click", () => void browseCloneDestination());
  $("clone-ok").addEventListener("click", () => void submitCloneDialog());
  $("clone-url").addEventListener("input", updateCloneDest);
  $("clone-name").addEventListener("input", updateCloneDest);
  $("clone-url").addEventListener("keydown", (event) => {
    if (event.key === "Enter") $("clone-name").focus();
  });

  /* merge editor */
  $("merge-cancel").addEventListener("click", closeMergeEditor);
  $("merge-apply").addEventListener("click", () => void guardButton($("merge-apply"), applyMergeResult));
  $("merge-all-ours").addEventListener("click", () => void guardButton($("merge-all-ours"), () => acceptAllMerge("ours")));
  $("merge-all-theirs").addEventListener("click", () => void guardButton($("merge-all-theirs"), () => acceptAllMerge("theirs")));
  $("prompt-ok").addEventListener("click", () => {
    const handler = promptHandler;
    if (handler) void guardButton($("prompt-ok"), handler);
  });
  $("prompt-input").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      const handler = promptHandler;
      if (handler) void guardButton($("prompt-ok"), handler);
    }
  });

  /* Tags dropdown */
  $("tags-button").addEventListener("click", async (event) => {
    const menu = $("tags-menu");
    if (menu.classList.contains("hidden")) {
      const button = event.currentTarget;
      await refreshTags().catch((e) => setStatus(`Tags failed: ${errorText(e)}`, true));
      const rect = button.getBoundingClientRect();
      menu.style.left = rect.left + "px";
      menu.style.top = rect.bottom + 4 + "px";
      menu.style.position = "fixed";
      show(menu);
    } else {
      hide(menu);
    }
  });
  $("tag-create-btn").addEventListener("click", () => {
    const name = $("tag-name").value.trim();
    if (!name) {
      setStatus("Type a tag name first.", true);
      return;
    }
    const rev = $("tag-rev").value.trim() || null;
    runAction(`Tag ${name}`, () => invoke("create_tag", { name, rev }), true, $("tag-create-btn")).then(() => {
      $("tag-name").value = "";
      $("tag-rev").value = "";
      refreshTags();
      refreshLog(false);
    });
  });
  $("tag-name").addEventListener("keydown", (event) => {
    if (event.key === "Enter") $("tag-create-btn").click();
  });
  $("tag-rev").addEventListener("keydown", (event) => {
    if (event.key === "Enter") $("tag-create-btn").click();
  });
  $("tag-push-btn").addEventListener("click", () =>
    runAction("Push tags", () => invoke("push_tags"), true, $("tag-push-btn"))
  );

  /* Log filter inputs */
  const applyLogFilters = () => refreshLog(true);
  for (const [inputId, key] of [["log-text", "text"], ["log-author", "author"], ["log-path", "path"]]) {
    $(inputId).addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        state.logFilters[key] = $(inputId).value.trim();
        applyLogFilters();
      } else if (event.key === "Escape") {
        state.logFilters[key] = "";
        $(inputId).value = "";
        applyLogFilters();
      }
    });
  }
  $("log-clear-btn").addEventListener("click", () => {
    for (const id of ["log-text", "log-author", "log-path"]) $(id).value = "";
    state.logFilters = { text: "", author: "", path: "" };
    applyLogFilters();
  });


  $("branch-button").addEventListener("click", (event) => {
    const button = event.currentTarget;
    const menu = $("branch-menu");
    if (!menu.classList.contains("hidden")) {
      hide(menu);
      return;
    }
    void refreshBranches().then(() => {
      const rect = button.getBoundingClientRect();
      menu.style.left = rect.left + "px";
      menu.style.top = rect.bottom + 4 + "px";
      show(menu);
    });
  });
  document.addEventListener("click", (event) => {
    const menu = $("context-menu");
    if (!menu.classList.contains("hidden") && !menu.contains(event.target)) closeContextMenu();
    const branch = $("branch-menu");
    if (!branch.classList.contains("hidden") && !branch.contains(event.target) &&
        event.target.closest("#branch-button") === null) hide(branch);
    const stash = $("stash-menu");
    if (!stash.classList.contains("hidden") && !stash.parentElement.contains(event.target)) hide(stash);
    const tags = $("tags-menu");
    if (!tags.classList.contains("hidden") && !tags.contains(event.target) &&
        event.target.closest("#tags-button") === null) hide(tags);
  });
  document.addEventListener("contextmenu", (event) => {
    // Keep the native menu where copying/pasting text matters.
    if (event.target.closest("input, textarea, [contenteditable], .diff-view")) return;
    event.preventDefault();
    hide($("branch-menu"));
    hide($("stash-menu"));
    if (event.target.closest("#changes-list")) showChangesContextMenu(event);
    else showGeneralContextMenu(event);
  });
  document.addEventListener("scroll", () => closeContextMenu(), true);
  window.addEventListener("resize", closeContextMenu);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeContextMenu();
      hide($("branch-menu"));
      hide($("stash-menu"));
      hide($('tags-menu'));
    }
  });

  $("create-branch-btn").addEventListener("click", async () => {
    const name = $("new-branch-name").value.trim();
    if (!name) return;
    await runAction(`New branch ${name}`, () => invoke("create_branch", { name }), true, $("create-branch-btn"));
    $("new-branch-name").value = "";
    hide($("branch-menu"));
  });
  $("new-branch-name").addEventListener("keydown", (event) => {
    if (event.key === "Enter") $("create-branch-btn").click();
  });

  $("stash-button").addEventListener("click", async () => {
    const menu = $("stash-menu");
    if (menu.classList.contains("hidden")) {
      await refreshStashes();
      show(menu);
    } else {
      hide(menu);
    }
  });
  $("stash-push-btn").addEventListener("click", () => {
    const message = $("stash-message").value.trim();
    runAction("Stash push", () => invoke("stash_push", { message }), true, $("stash-push-btn")).then(() => {
      $("stash-message").value = "";
    });
  });

  $("commit-btn").addEventListener("click", () => {
    const message = $("commit-message").value.trim();
    if (!message) {
      setStatus("Type a commit message first.", true);
      return;
    }
    const amend = $("amend-check").checked;
    runAction("Commit", () => invoke("commit", { message, amend }), true, $("commit-btn")).then(() => {
      if (!$("status-text").parentElement.classList.contains("error")) {
        $("commit-message").value = "";
        hide($("stash-menu"));
      }
    });
  });
  $("commit-message").addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) $("commit-btn").click();
  });

  document.querySelectorAll(".tabs .tab").forEach((node) =>
    node.addEventListener("click", () => switchTab(node.dataset.tab))
  );
  document.querySelectorAll(".diff-mode .tab").forEach((node) =>
    node.addEventListener("click", () => setDiffMode(node.dataset.mode))
  );
  document.querySelectorAll(".diff-layout .tab").forEach((node) =>
    node.addEventListener("click", () => {
      state.diffLayout = node.dataset.layout;
      localStorage.setItem("gitjet.diffLayout", state.diffLayout);
      document.querySelectorAll(".diff-layout .tab").forEach((n) =>
        n.classList.toggle("active", n.dataset.layout === state.diffLayout)
      );
      loadDiff();
    })
  );
  $("log-all").addEventListener("change", (event) => {
    state.logAll = event.target.checked;
    refreshLog(true);
  });
  $("load-more-btn").addEventListener("click", () => void guardButton($("load-more-btn"), loadMore));

  // Apply the persisted diff layout before the first render.
  {
    state.diffLayout = localStorage.getItem("gitjet.diffLayout") || "unified";
    document.querySelectorAll(".diff-layout .tab").forEach((n) =>
      n.classList.toggle("active", n.dataset.layout === state.diffLayout)
    );
  }

  document.addEventListener("keydown", (event) => {
    if (event.key === "F5") void guardButton($("refresh-btn"), refreshAll);
    if (event.key === "Escape") closeDialogs();
  });

  /* Auto refresh: while the window is focused, poll a cheap filesystem
   * signature of the repository (a few stat calls, no git process) and
   * only run the full status re-read when something actually changed —
   * external edits (another tool, a terminal, a fetch) show up without F5. */
  setInterval(async () => {
    if (!state.root || !document.hasFocus()) return;
    if (document.querySelector(".dialog:not(.hidden)")) return;
    try {
      const stamp = await invoke("repo_stamp").catch(() => null);
      if (stamp && state.lastStamp && stamp === state.lastStamp) return;
      await refreshStatus();
    } catch { /* transient errors stay silent */ }
  }, 15000);
  window.addEventListener("focus", () => {
    if (state.root) refreshStatus().catch(() => {});
  });

  /* Resizable left panel: drag the divider, remember the width. */
  const resizer = $("panel-resizer");
  const changesPanel = document.querySelector(".changes-panel");
  const PANEL_MIN = 260;
  const panelMax = () => Math.max(PANEL_MIN, window.innerWidth - 420);
  const savedWidth = Number(localStorage.getItem("gitjet.changesWidth"));
  if (savedWidth >= PANEL_MIN && savedWidth <= panelMax()) {
    changesPanel.style.width = savedWidth + "px";
  }
  let panelDrag = null;
  resizer.addEventListener("pointerdown", (event) => {
    panelDrag = { startX: event.clientX, startWidth: changesPanel.getBoundingClientRect().width };
    resizer.setPointerCapture(event.pointerId);
    resizer.classList.add("dragging");
    document.body.classList.add("resizing");
  });
  resizer.addEventListener("pointermove", (event) => {
    if (!panelDrag) return;
    const width = Math.min(panelMax(), Math.max(PANEL_MIN, panelDrag.startWidth + event.clientX - panelDrag.startX));
    changesPanel.style.width = width + "px";
  });
  const endPanelDrag = (event) => {
    if (!panelDrag) return;
    panelDrag = null;
    resizer.releasePointerCapture(event.pointerId);
    resizer.classList.remove("dragging");
    document.body.classList.remove("resizing");
    localStorage.setItem("gitjet.changesWidth", String(changesPanel.getBoundingClientRect().width));
  };
  resizer.addEventListener("pointerup", endPanelDrag);
  resizer.addEventListener("pointercancel", endPanelDrag);


  renderRecent();

  const last = localStorage.getItem("gitjet.lastRoot");
  if (last) {
    try {
      const { root } = JSON.parse(last);
      if (root) openRepository(root);
    } catch { /* ignore corrupt value */ }
  }
}

function expandHome(path) {
  return path.replace(/^~(?=\/|\\|$)/, "");
}

document.addEventListener("DOMContentLoaded", boot);
