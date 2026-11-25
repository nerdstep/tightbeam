# AGENTS.md

Tightbeam is private chat between two people on the same LAN: a Rust backend (Tauri 2, iroh, SQLite) in `src-tauri/` and a React UI in `ui/`. `README.md` explains the protocol, sync and layout; read it before touching either. `ROADMAP.md` has what's next and why. `GLOSSARY.md` holds the domain terms: use them when naming code or writing docs, and add one when you introduce a new concept.

## Done

- A change is done when the checks in `.github/workflows/ci.yml` pass locally: the UI check from the project root, and fmt, clippy and tests in `src-tauri/`.
- A protocol change gets a test in `src-tauri/src/chat/protocol_tests.rs`, which runs two engines on an in-memory network with paused time.
- A change people will notice also gets a line under `## Unreleased` in `CHANGELOG.md`. That section becomes the release text.
- Commit messages and PR titles follow Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:` …). PRs squash-merge, so the title becomes the commit on `main`.

## Compatibility between versions

The two peers routinely run different builds, and both keep their history across upgrades.

- **Wire format** (`Frame` in `src-tauri/src/chat.rs`, and a `msg` frame's body, `Wire`, in `src-tauri/src/entry.rs`): grow it by adding frame types and optional fields. Existing frames keep their meaning. Older peers skip unknown types (`Frame::Unknown`); `Hello` and `PairRequest` stay strict.
- **Message log**: append-only per author. Edits, deletes, reactions and replies are new entries that refer to `(author, seq)`. The one change in place: a delete blanks the text of the message it deletes and of that message's edits.
- **SQLite**: a schema change migrates existing databases in place.

## Gotchas

- Tauri's sync commands run off the tokio runtime. Spawn async work through `Core`'s runtime handle (`self.rt`).
- Windows can't send from a LAN address to itself, so same-machine runs use loopback: `scripts/two-windows.ps1`, or `TIGHTBEAM_DATA_DIR` plus `TIGHTBEAM_LOOPBACK=1`.
- The macOS webview is WebKit (Safari 16.4 and up). CSS newer than that needs a fallback, like `fit()` in `Chat.tsx` and the scrollbar rules in `styles.css`.
- `src-tauri/Info.plist` declares the mDNS service and the Local Network reason. Without them, macOS blocks discovery silently.
- `ui/src/components/ui/` is shadcn's generated code. Local edits there carry a `Tightbeam:` comment; reapply them after `shadcn add` overwrites a file.
- The transcript stays fast because status events (acks, discovery) leave message rows alone: rows are memoized, and components select narrowly from the store.
- The version lives only in `src-tauri/Cargo.toml`. Releases go through `scripts/release.ps1` (see "Releases" in `README.md`).

## Public repo

Everything committed is public. Machine-specific details (hostnames, IPs, account names, SSH setup) go in gitignored `*.local.md` files. Secrets, such as the updater signing key, go in GitHub repository secrets.

## Agent skills

### Issue tracker

Issues live in this repo's GitHub Issues, managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five default triage roles, each label named after its role. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
