# Reclaude

A small Windows desktop app that renames Claude Code project folders without losing your chat history.

> Unofficial tool, not affiliated with Anthropic.

![Preview of a rename: history stats, safety warnings, and a diff of everything that will change](docs/preview.png)

## The Problem

Here's the annoying part. Claude Code doesn't keep a project's chat history in the
project folder. It keeps it in `%USERPROFILE%\.claude\projects\<ENCODED>`, where
`<ENCODED>` is built from the project's absolute path (every non-alphanumeric character
becomes a dash, and the drive letter gets lowercased). So the moment you rename the folder
in Explorer, Claude opens to an empty chat. The history isn't gone, it's just orphaned,
pointing at a path that no longer exists.

Reclaude does the rename properly and keeps four things in sync:

1. The real project folder on disk.
2. The encoded history folder under `%USERPROFILE%\.claude\projects\` (this is also where per-project memory lives).
3. Path references inside `%USERPROFILE%\.claude.json` (literal text replacement, all four path spellings, and it won't clobber siblings).
4. Optionally (on by default), the old path baked into every session `.jsonl` file, so resumed sessions point at the new path too.

Everything gets backed up first (it keeps the last 5 backup sets under
`%LOCALAPPDATA%\Reclaude\backups`), it rolls back automatically if any step fails, and
there's an **Undo last rename** button for when you change your mind.

Scope: native Windows Claude Code only, not WSL.

## Why Tauri

I went with Tauri v2 over a C# WinForms fallback for a few reasons. The web UI makes the
Claude-style theme (dark/light, serif headings, diff-styled preview) easy to build, the exe
stays tiny (~6 MB), and WebView2 already ships with Windows 11 and basically every updated
Windows 10 machine. That last part is what matters most: the built exe just runs on a
double-click, no runtime to install.

## Prerequisites (Build Only)

- **Rust toolchain** (stable, MSVC target), install via [rustup](https://rustup.rs)
- **Visual Studio Build Tools** with the *Desktop development with C++* workload
- **Node.js**, only used to run the Tauri CLI and generate the icon

End users don't need any of this. They just need the exe (WebView2 is already on Win 11 and updated Win 10).

## Build

```powershell
npm install                          # installs @tauri-apps/cli
node scripts/gen-icon.mjs            # (re)generate the icon PNG, only needed once
npx tauri icon scripts/icon-1024.png # (re)generate .ico + pngs, only needed once
npx tauri build                      # release build
```

The final exe lands at:

```text
src-tauri\target\release\Reclaude.exe
```

It's fully standalone, so copy it anywhere and double-click.

There's also an "installed" copy at `%LOCALAPPDATA%\Programs\Reclaude\Reclaude.exe`, which
is what the Start Menu shortcut (and therefore Windows Search) points to. After a rebuild,
refresh it with `npm run install-app` (build + copy) or `node scripts/install.mjs` (copy only).

For development with hot reload of the frontend, run `npx tauri dev`.

One gotcha: `Cargo.lock` pins the transitive `time` crate to 0.3.47, because `time` 0.3.48
currently fails to compile against `cookie` 0.18 (E0119). If you regenerate the lockfile and
hit that error, run `cargo update time --precise 0.3.47` inside `src-tauri`.

## Tests

Unit tests cover the core logic (path encoding, sibling-safe replacement, name validation),
and an integration test drives the whole pipeline (rename, rollback on forced failure, undo,
case-only rename) against a sandboxed fake `%USERPROFILE%`:

```powershell
cd src-tauri
cargo test
```

After building, `node scripts/smoke.mjs` launches the real exe and exercises the UI and IPC
end to end via WebView2 remote debugging, and `node scripts/screenshot.mjs` captures
screenshots of the running app.

## How the Rename Works

The execution order is deliberate. The step most likely to fail (a locked folder) happens
first, and every step after it can be rolled back:

1. Back up `.claude.json` and zip the affected session files.
2. Rename the real folder (case-only renames go through a temp name in two steps).
3. Rename the encoded history folder(s), including verified nested projects when you rename a parent folder.
4. Literal, sibling-safe text replacement in `.claude.json` (never re-serialized, and UTF-8 without BOM is preserved).
5. Deep-fix the `.jsonl` session files.

A `rename-manifest.json` under `%LOCALAPPDATA%\Reclaude` records all of it for Undo.

Then there's the sibling-name trap, which is the whole reason this is fiddly: renaming
`...\Final Year Project` must not touch `...\Final Year Project 2`. So replacements only
fire when the match is followed by `"`, `/`, or `\`, and any ambiguous encoded folders are
verified against the `cwd` recorded in their newest session file. Anything that can't be
verified is listed and left alone.

## License

[MIT](LICENSE)
