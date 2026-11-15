// The window may only call plugin commands its capability file allows; a
// missing one fails silently at runtime (that's how Leave room broke). Check
// that every plugin API the UI uses is granted.
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

const capability = (name: string) => JSON.parse(readFileSync(join(here, `../src-tauri/capabilities/${name}.json`), "utf8"));

function sources(dir: string, skip: string[] = []): string {
  return readdirSync(dir, { withFileTypes: true })
    .map((e) => {
      const p = join(dir, e.name);
      if (e.isDirectory()) return e.name === "test" || skip.includes(e.name) ? "" : sources(p);
      return /\.tsx?$/.test(e.name) && !/\.test\./.test(e.name) ? readFileSync(p, "utf8") : "";
    })
    .join("\n");
}

// The main window runs everything but src/call; the call window runs
// src/call (and the shared modules it imports, which only use core APIs).
const windows: { label: string; code: string; granted: string[] }[] = [
  { label: "main", code: sources(here, ["call"]), granted: capability("default").permissions },
  { label: "call", code: sources(join(here, "call")), granted: capability("call").permissions },
];

// API used → permissions that would allow it.
const needs: [RegExp, string[]][] = [
  [/\bopen\(\{/, ["dialog:default", "dialog:allow-open"]],
  [/\bask\(/, ["dialog:default", "dialog:allow-message"]],
  [/\bwriteText\(/, ["clipboard-manager:allow-write-text"]],
  [/\bcheck\(\)/, ["updater:default", "updater:allow-check"]],
  [/\bdownloadAndInstall\(/, ["updater:default", "updater:allow-download-and-install"]],
  [/\brelaunch\(\)/, ["process:default", "process:allow-restart"]],
  [/data-tauri-drag-region/, ["core:window:allow-start-dragging"]],
  [/\bgetCurrent\(\)/, ["deep-link:default", "deep-link:allow-get-current"]],
  [/\bonOpenUrl\(/, ["core:default", "core:event:default"]],
  [/\bstartResizeDragging\(/, ["core:window:allow-start-resize-dragging"]],
  [/\.setSize\(/, ["core:window:allow-set-size"]],
  [/\.setPosition\(/, ["core:window:allow-set-position"]],
  [/\bonResized\(/, ["core:default", "core:window:default"]],
  [/\blisten\(/, ["core:default", "core:event:default"]],
];

for (const w of windows) {
  test.each(needs)(`${w.label} window uses %s, so it is allowed to`, (pattern, perms) => {
    if (!pattern.test(w.code)) return;
    expect(perms.some((p) => w.granted.includes(p))).toBe(true);
  });
}

test("each capability file is for the window it's named after", () => {
  expect(capability("default").windows).toEqual(["main"]);
  expect(capability("call").windows).toEqual(["call"]);
});
