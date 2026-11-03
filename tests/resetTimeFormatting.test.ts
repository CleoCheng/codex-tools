import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { formatResetShort } from "../src/components/classic/accountTokenStats.ts";

test("next-day resets within 24 hours keep their date and clock time", (t) => {
  t.mock.method(Date, "now", () => new Date(2026, 10, 3, 20, 20).getTime());
  for (const [hour, minute] of [[17, 51], [15, 4], [17, 14], [14, 38], [16, 2], [1, 20]]) {
    const resetAt = new Date(2026, 10, 4, hour, minute).getTime() / 1000;
    assert.equal(
      formatResetShort(resetAt, "zh-CN"),
      `11/04 ${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`,
    );
  }
});

test("same-day, distant and past resets retain an unambiguous date and time", (t) => {
  t.mock.method(Date, "now", () => new Date(2026, 10, 3, 20, 20).getTime());
  for (const [day, hour, minute, expected] of [
    [3, 21, 55, "11/03 21:55"],
    [9, 10, 53, "11/09 10:53"],
    [2, 17, 51, "11/02 17:51"],
  ] as const) {
    assert.equal(
      formatResetShort(new Date(2026, 10, day, hour, minute).getTime() / 1000, "zh-CN"),
      expected,
    );
  }
});

test("reset labels use the system time zone once, including across midnight", () => {
  const moduleUrl = new URL("../src/components/classic/accountTokenStats.ts", import.meta.url).href;
  const script = `
    import { formatResetShort } from ${JSON.stringify(moduleUrl)};
    process.stdout.write(JSON.stringify([
      formatResetShort(1793785912, "zh-CN"),
      formatResetShort(Date.parse("2026-11-03T18:20:00Z") / 1000, "zh-CN"),
    ]));
  `;
  for (const [timeZone, expected] of [
    ["Asia/Shanghai", ["11/04 17:51", "11/04 02:20"]],
    ["UTC", ["11/04 09:51", "11/03 18:20"]],
  ] as const) {
    const result = spawnSync(process.execPath, ["--input-type=module", "--eval", script], {
      env: { ...process.env, TZ: timeZone },
      encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), expected);
  }
});

test("missing and malformed reset timestamps remain empty", () => {
  for (const resetAt of [undefined, null, 0, NaN, Infinity, -Infinity, 1e20]) {
    assert.equal(formatResetShort(resetAt, "zh-CN"), null);
  }
});
