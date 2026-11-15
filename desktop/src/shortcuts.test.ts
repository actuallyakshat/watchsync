import { fromEvent, label } from "./shortcuts";

const key = (code: string, mods: Partial<Record<"ctrlKey" | "altKey" | "shiftKey" | "metaKey", boolean>> = {}) => ({
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  ...mods,
});

test("a key press becomes a shortcut only with a modifier and a real key", () => {
  expect(fromEvent(key("KeyM", { altKey: true, metaKey: true }))).toBe("Alt+Command+KeyM");
  expect(fromEvent(key("KeyM"))).toBeNull();
  expect(fromEvent(key("MetaLeft", { metaKey: true }))).toBeNull();
  expect(fromEvent(key("Digit1", { ctrlKey: true, shiftKey: true }))).toBe("Control+Shift+Digit1");
});

test("shortcuts read the way macOS menus show them", () => {
  expect(label("Alt+Command+KeyM")).toBe("⌥⌘M");
  expect(label("Control+Shift+Digit1")).toBe("⌃⇧1");
  expect(label("Command+F5")).toBe("⌘F5");
});
