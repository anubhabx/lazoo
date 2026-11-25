import { describe, expect, it } from "vitest";

import { describeSchedule, nextFire } from "./schedule";
import type { Schedule } from "./types";

describe("nextFire", () => {
  it("adds the interval after the given time", () => {
    const next = nextFire(
      { mode: "interval", everyMinutes: 30 },
      new Date(2026, 10, 2, 10, 0, 0),
    );
    expect(next).toEqual(new Date(2026, 10, 2, 10, 30, 0));
  });

  it("is strictly after the given time", () => {
    const schedule = {
      mode: "daily",
      time: "09:00",
      days: [0, 1, 2, 3, 4, 5, 6],
    } as Schedule;
    expect(nextFire(schedule, new Date(2026, 10, 2, 9, 0, 0))).toEqual(
      new Date(2026, 10, 3, 9, 0, 0),
    );
    expect(nextFire(schedule, new Date(2026, 10, 2, 8, 59, 59))).toEqual(
      new Date(2026, 10, 2, 9, 0, 0),
    );
  });

  it("rolls over midnight and month ends", () => {
    const schedule = {
      mode: "daily",
      time: "00:30",
      days: [0, 1, 2, 3, 4, 5, 6],
    } as Schedule;
    expect(nextFire(schedule, new Date(2026, 10, 30, 23, 50, 0))).toEqual(
      new Date(2026, 11, 1, 0, 30, 0),
    );
  });

  it("only fires on the chosen days", () => {
    // 2026-11-06 is a Friday; Monday is day 1.
    const schedule = { mode: "daily", time: "07:00", days: [1, 3] } as Schedule;
    const next = nextFire(schedule, new Date(2026, 10, 6, 12, 0, 0));
    expect(next).toEqual(new Date(2026, 10, 9, 7, 0, 0));
    expect(next.getDay()).toBe(1);
    expect(nextFire(schedule, next)).toEqual(new Date(2026, 10, 11, 7, 0, 0));
  });

  it("keeps the wall-clock time across clock changes", () => {
    const schedule = {
      mode: "daily",
      time: "18:30",
      days: [0, 1, 2, 3, 4, 5, 6],
    } as Schedule;
    for (const after of [
      new Date(2026, 2, 28, 19, 0, 0),
      new Date(2026, 9, 24, 19, 0, 0),
    ]) {
      const next = nextFire(schedule, after);
      expect([next.getHours(), next.getMinutes()]).toEqual([18, 30]);
    }
  });
});

describe("describeSchedule", () => {
  it("describes intervals", () => {
    expect(describeSchedule({ mode: "interval", everyMinutes: 30 })).toBe(
      "Every 30 minutes",
    );
    expect(describeSchedule({ mode: "interval", everyMinutes: 1 })).toBe(
      "Every minute",
    );
  });

  it("describes daily schedules", () => {
    expect(
      describeSchedule({ mode: "daily", time: "09:00", days: [1, 2, 3, 4, 5] }),
    ).toBe("Weekdays at 09:00");
    expect(
      describeSchedule({
        mode: "daily",
        time: "18:30",
        days: [0, 1, 2, 3, 4, 5, 6],
      }),
    ).toBe("Every day at 18:30");
    expect(
      describeSchedule({ mode: "daily", time: "07:00", days: [3, 1] }),
    ).toBe("Mon, Wed at 07:00");
    expect(
      describeSchedule({ mode: "daily", time: "10:00", days: [0, 6] }),
    ).toBe("Weekends at 10:00");
  });
});
