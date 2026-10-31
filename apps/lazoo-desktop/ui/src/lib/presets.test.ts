import { describe, expect, test } from "vitest";

import type { ProviderListEntry } from "./protocol";
import { employeeParams, recommendTeam } from "./presets";

function harnesses(...detected: string[]): ProviderListEntry[] {
  return ["codex", "claude-code", "opencode", "antigravity", "grok-build"].map(
    (provider) => ({
      provider,
      detected: detected.includes(provider),
      executable: null,
      discoverySource: null,
    }),
  );
}

describe("recommended team", () => {
  test("nothing detected offers nothing", () => {
    expect(recommendTeam(harnesses())).toEqual([]);
  });

  test("Claude Code and Codex split the roles by what each is best at", () => {
    const team = recommendTeam(harnesses("claude-code", "codex"));
    const byRole = Object.fromEntries(
      team.map((draft) => [draft.key, `${draft.provider}/${draft.model}`]),
    );
    expect(byRole).toEqual({
      lead: "claude-code/opus",
      builder: "codex/gpt-6.1-sol",
      frontend: "claude-code/sonnet",
      reviewer: "codex/gpt-6-astra",
    });
    expect(team.every((draft) => draft.include)).toBe(true);
  });

  test("a single harness fills every core role, optional roles need their own harness", () => {
    const team = recommendTeam(harnesses("codex"));
    expect(team.map((draft) => draft.key)).toEqual([
      "lead",
      "builder",
      "frontend",
      "reviewer",
    ]);
    expect(new Set(team.map((draft) => draft.provider))).toEqual(
      new Set(["codex"]),
    );
  });

  test("optional roles appear when their harness is detected", () => {
    const team = recommendTeam(
      harnesses("claude-code", "codex", "antigravity", "opencode"),
    );
    expect(team.map((draft) => draft.key)).toContain("researcher");
    expect(team.map((draft) => draft.key)).toContain("quick");
    expect(team.find((draft) => draft.key === "researcher")?.provider).toBe(
      "antigravity",
    );
  });

  test("names already taken start unticked", () => {
    const team = recommendTeam(harnesses("claude-code"), ["Atlas"]);
    const lead = team.find((draft) => draft.key === "lead")!;
    expect(lead.exists).toBe(true);
    expect(lead.include).toBe(false);
  });

  test("create params drop an effort the harness cannot take and blank models", () => {
    const [lead] = recommendTeam(harnesses("antigravity"));
    const params = employeeParams({ ...lead!, reasoningEffort: "high" });
    expect(params).toMatchObject({
      name: "atlas",
      provider: "antigravity",
      model: null,
      reasoningEffort: null,
      enabled: true,
    });
  });
});
