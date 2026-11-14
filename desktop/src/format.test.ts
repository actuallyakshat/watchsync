import { formatDrift, formatTime, frameSize, movieTitle } from "./format";

test("cleans up release file names", () => {
  expect(movieTitle("The Odyssey 2026 1080p BluRay HEVC x265 5.1 BONE.mkv")).toEqual({
    title: "The Odyssey",
    year: "2026",
  });
  expect(movieTitle("Primetime (2026) (1080p AMZN WEB-DL x265 10bit EAC3 Atmos 5.1 Silence).mkv")).toEqual({
    title: "Primetime",
    year: "2026",
  });
  expect(movieTitle("2001.A.Space.Odyssey.1968.2160p.UHD.BluRay.mkv")).toEqual({
    title: "2001 A Space Odyssey",
    year: "1968",
  });
  expect(movieTitle("home_video.mp4")).toEqual({ title: "home video", year: null });
  expect(movieTitle("Dune Part Two.mkv")).toEqual({ title: "Dune Part Two", year: null });
});

test("formats times and drift", () => {
  expect(formatTime(65.9)).toBe("1:05");
  expect(formatTime(10355.69)).toBe("2:52:35");
  expect(formatTime(-3)).toBe("0:00");
  expect(formatDrift(null)).toBe("");
  expect(formatDrift(12)).toBe("in sync");
  expect(formatDrift(-180)).toBe("−180 ms");
  expect(formatDrift(2300)).toBe("+2.3 s");
});

test("crop previews are distinct shapes", () => {
  const heights = [16 / 9, 1.85, 1.9, 2, 2.2, 2.39, 2.76].map((a) => frameSize(a).h);
  expect(new Set(heights).size).toBeGreaterThanOrEqual(6);
  expect(heights).toEqual([...heights].sort((a, b) => b - a));
  expect(frameSize(1.43)).toEqual({ w: 23, h: 16 });
  expect(frameSize(null)).toEqual({ w: 28, h: 16 });
});
