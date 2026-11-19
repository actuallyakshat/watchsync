import { filmstripSide, gridFit, pickStage, tileKey, type TileRef } from "./layout";

const cam = (id: string, self = false): TileRef => ({ key: tileKey(id, "cam"), kind: "cam", self });
const scr = (id: string, self = false): TileRef => ({ key: tileKey(id, "screen"), kind: "screen", self });

test("the big spot: your pin, else a friend's newest screen, else who's talking", () => {
  const tiles = [scr("b"), scr("c"), cam("b"), cam("c"), scr("self", true), cam("self", true)];
  expect(pickStage(tiles, null, "b")).toBe("c:screen");
  expect(pickStage(tiles, "b", "c")).toBe("b");
  expect(pickStage(tiles, "self", null)).toBe("self");
  // Your own screen never takes it by itself; a pin that's gone is ignored.
  expect(pickStage([cam("b"), cam("c"), scr("self", true), cam("self", true)], "x:screen", "c")).toBe("c");
  expect(pickStage([cam("b"), cam("c")], null, null)).toBe("b");
  expect(pickStage([cam("self", true)], null, null)).toBe(null);
});

test("a filmstrip only where there's room, beside a wide view and under a tall one", () => {
  expect(filmstripSide(null)).toBe(null);
  expect(filmstripSide({ w: 320, h: 200 })).toBe(null);
  expect(filmstripSide({ w: 800, h: 450 })).toBe("right");
  expect(filmstripSide({ w: 500, h: 600 })).toBe("bottom");
});

test("grid tiles stay 16:9 and as big as they fit", () => {
  expect(gridFit(1, { w: 1600, h: 900 })).toEqual({ cols: 1, w: 1600, h: 900 });
  // Three in a wide window: one row is too thin, two rows fit better.
  const three = gridFit(3, { w: 1600, h: 900 })!;
  expect(three.cols).toBe(2);
  expect(three.w / three.h).toBeCloseTo(16 / 9, 1);
  // Three in a very wide one: a single row.
  expect(gridFit(3, { w: 3000, h: 500 })!.cols).toBe(3);
  expect(gridFit(0, { w: 100, h: 100 })).toBe(null);
  expect(gridFit(2, null)).toBe(null);
});
