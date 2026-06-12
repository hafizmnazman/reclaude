// Copies the built exe to %LOCALAPPDATA%\Programs\Reclaude, which is where
// the Start Menu shortcut (and Windows Search) points. Run after a rebuild,
// or use `npm run install-app` to build + install in one step.
import { copyFileSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, "..", "src-tauri", "target", "release", "Reclaude.exe");
const destDir = join(process.env.LOCALAPPDATA, "Programs", "Reclaude");
mkdirSync(destDir, { recursive: true });
copyFileSync(src, join(destDir, "Reclaude.exe"));
console.log("installed to", join(destDir, "Reclaude.exe"));
