// Schedules in local time. Dates are built from local parts, so a DST change never
// skips or doubles a wall-clock time. Missed occurrences are never replayed: the
// scheduler only asks for the next one after "now or the last fire".

import type { Schedule } from "./types";

const DAY_NAMES = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function parseTime(time: string): [number, number] {
  const match = /^(\d{1,2}):(\d{2})$/.exec(time);
  const hours = match ? Number(match[1]) : 9;
  const minutes = match ? Number(match[2]) : 0;
  return [Math.min(23, hours), Math.min(59, minutes)];
}

/** The first occurrence strictly after `after`. */
export function nextFire(schedule: Schedule, after: Date): Date {
  if (schedule.mode === "interval") {
    const minutes = Math.max(1, Math.round(schedule.everyMinutes));
    return new Date(after.getTime() + minutes * 60_000);
  }
  const [hours, minutes] = parseTime(schedule.time);
  const days = schedule.days.length ? schedule.days : [0, 1, 2, 3, 4, 5, 6];
  for (let offset = 0; offset <= 8; offset += 1) {
    const candidate = new Date(
      after.getFullYear(),
      after.getMonth(),
      after.getDate() + offset,
      hours,
      minutes,
      0,
      0,
    );
    if (
      candidate.getTime() > after.getTime() &&
      days.includes(candidate.getDay())
    ) {
      return candidate;
    }
  }
  return new Date(after.getTime() + 24 * 3_600_000);
}

export function describeSchedule(schedule: Schedule): string {
  if (schedule.mode === "interval") {
    return schedule.everyMinutes === 1
      ? "Every minute"
      : `Every ${schedule.everyMinutes} minutes`;
  }
  const days = [...new Set(schedule.days)].sort((a, b) => a - b);
  const key = days.join(",");
  let when: string;
  if (days.length === 0 || days.length === 7) when = "Every day";
  else if (key === "1,2,3,4,5") when = "Weekdays";
  else if (key === "0,6") when = "Weekends";
  else {
    // Monday first reads naturally: "Mon, Wed" rather than "Sun, Mon".
    when = [...days]
      .sort((a, b) => ((a + 6) % 7) - ((b + 6) % 7))
      .map((day) => DAY_NAMES[day])
      .join(", ");
  }
  return `${when} at ${schedule.time}`;
}
