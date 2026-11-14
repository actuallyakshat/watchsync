// The window may only call plugin commands its capability file allows; a
// missing one fails silently at runtime (that's how Leave room broke). Check
// that every plugin API the UI uses is granted.
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

const cap = JSON.parse(readFileSync(join(here, "../src-tauri/capabilities/default.json"), "utf8"));
const granted: string[] = cap.permissions;

function sources(dir: string): string {
  return readdirSync(dir, { withFileTypes: true })
    .map((e) => {
      const p = join(dir, e.name);
      if (e.isDirectory()) return e.name === "test" ? "" : sources(p);
      return /\.tsx?$/.test(e.name) && !/\.test\./.test(e.name) ? readFileSync(p, "utf8") : "";
    })
    .join("\n");
}
const code = sources(here);

// API used → permissions that would allow it.
const needs: [RegExp, string[]][] = [
  [/\bopen\(\{/, ["dialog:default", "dialog:allow-open"]],
  [/\bask\(/, ["dialog:default", "dialog:allow-message"]],
  [/\bwriteText\(/, ["clipboard-manager:allow-write-text"]],
  [/\bcheck\(\)/, ["updater:default", "updater:allow-check"]],
  [/\bdownloadAndInstall\(/, ["updater:default", "updater:allow-download-and-install"]],
  [/\brelaunch\(\)/, ["process:default", "process:allow-restart"]],
  [/data-tauri-drag-region/, ["core:window:allow-start-dragging"]],
];

test.each(needs)("UI uses %s, so the window is allowed to", (pattern, perms) => {
  if (!pattern.test(code)) return;
  expect(perms.some((p) => granted.includes(p))).toBe(true);
});
