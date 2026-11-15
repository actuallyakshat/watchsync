import { HANGOVER_MS, TAKE_OVER_MS, pickSpeaker, track, type Talk } from "./speaker";

/** Feed levels every 100 ms for `ms`, starting at `t`. */
function run(talk: Map<string, Talk>, levels: Record<string, number>, t: number, ms: number) {
  for (let now = t; now <= t + ms; now += 100) talk = track(talk, new Map(Object.entries(levels)), now);
  return talk;
}

test("the view stays on someone until another person talks for a moment", () => {
  let talk = new Map<string, Talk>();
  const people = ["riya", "aman"];
  expect(pickSpeaker(null, people, talk, 0)).toBe("riya"); // nobody talking: first person

  talk = run(talk, { riya: 0, aman: 0.2 }, 0, 300);
  expect(pickSpeaker("riya", people, talk, 300)).toBe("riya"); // a cough doesn't switch

  talk = run(talk, { riya: 0, aman: 0.2 }, 400, TAKE_OVER_MS);
  expect(pickSpeaker("riya", people, talk, 400 + TAKE_OVER_MS)).toBe("aman");
});

test("the current speaker keeps the view while still talking", () => {
  let talk = run(new Map(), { riya: 0.3, aman: 0.3 }, 0, 2000);
  expect(pickSpeaker("riya", ["riya", "aman"], talk, 2000)).toBe("riya");
  // A short pause still counts as talking.
  talk = run(talk, { riya: 0, aman: 0.3 }, 2100, HANGOVER_MS - 200);
  expect(pickSpeaker("riya", ["riya", "aman"], talk, 2100 + HANGOVER_MS - 200)).toBe("riya");
});

test("someone who left is replaced", () => {
  expect(pickSpeaker("gone", ["riya"], new Map(), 0)).toBe("riya");
  expect(pickSpeaker("gone", [], new Map(), 0)).toBeNull();
});
