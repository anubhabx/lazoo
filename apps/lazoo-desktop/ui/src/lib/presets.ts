// Default agents that ship with the app. Each role lists the harnesses that
// suit it best, in order; the first one detected on this machine wins.
//
// The picks follow what developers reported in October 2026 comparisons of
// Claude Code and Codex (see docs/superpowers/specs/2026-11-01-desktop-ui-refresh-design.md,
// section 5): Claude Opus for planning, architecture and long agentic runs;
// Sonnet for everyday and UI work; GPT-6.1 Sol in Codex for everyday features
// and mechanical multi-file refactors; GPT-6 Astra for the hardest reviews,
// with a different vendor reviewing than writing; Gemini (Antigravity) for
// fast, large-context reading and docs; OpenCode for small quick edits on
// whatever model the user already configured there.

import type {
  ActivationPolicy,
  AuthorityMode,
  EmployeeCheckoutPolicy,
  ProviderListEntry,
} from "./protocol";

export type PresetRole =
  "lead" | "builder" | "frontend" | "reviewer" | "researcher" | "quick";

export interface HarnessChoice {
  provider: string;
  model: string | null;
  reasoningEffort: string | null;
}

export interface Preset {
  role: PresetRole;
  name: string;
  /** Free-text role stored on the employee. */
  roleLabel: string;
  title: string;
  summary: string;
  instructions: string;
  checkoutPolicy: EmployeeCheckoutPolicy;
  activationPolicy: ActivationPolicy;
  authorityMode: AuthorityMode;
  /** Shown only when the first choice's harness is detected. */
  optional: boolean;
  choices: HarnessChoice[];
}

/** One editable row in the setup dialog. */
export interface AgentDraft {
  key: PresetRole;
  include: boolean;
  name: string;
  roleLabel: string;
  title: string;
  summary: string;
  why: string;
  provider: string;
  model: string;
  reasoningEffort: string;
  authorityMode: AuthorityMode;
  checkoutPolicy: EmployeeCheckoutPolicy;
  activationPolicy: ActivationPolicy;
  instructions: string;
  /** An enabled employee already uses this name; the row starts unticked. */
  exists: boolean;
}

const claude = (model: string, effort: string | null): HarnessChoice => ({
  provider: "claude-code",
  model,
  reasoningEffort: effort,
});
const codex = (model: string, effort: string | null): HarnessChoice => ({
  provider: "codex",
  model,
  reasoningEffort: effort,
});
const antigravity: HarnessChoice = {
  provider: "antigravity",
  model: null,
  reasoningEffort: null,
};
const opencode: HarnessChoice = {
  provider: "opencode",
  model: null,
  reasoningEffort: null,
};

export const PRESETS: Preset[] = [
  {
    role: "lead",
    name: "atlas",
    roleLabel: "lead",
    title: "Lead",
    summary: "Plans the work, delegates, and reports back",
    instructions:
      "You lead a small team of coding agents. Turn the request into a short plan, delegate well-scoped tasks to the teammate best suited for each, ask the reviewer to check risky changes, and finish with a concise summary of what changed, how it was verified, and what is left.",
    checkoutPolicy: "isolated",
    activationPolicy: "notice-turns",
    authorityMode: "self-decision",
    optional: false,
    choices: [
      claude("opus", "high"),
      codex("gpt-6-astra", "high"),
      antigravity,
      opencode,
    ],
  },
  {
    role: "builder",
    name: "forge",
    roleLabel: "implementer",
    title: "Builder",
    summary: "Implements features and multi-file refactors",
    instructions:
      "You implement well-scoped tasks end to end. Keep the diff small and focused, follow the conventions already in the code, run the relevant tests, and report what you changed and how you verified it.",
    checkoutPolicy: "isolated",
    activationPolicy: "manual",
    authorityMode: "self-decision",
    optional: false,
    choices: [
      codex("gpt-6.1-sol", "medium"),
      claude("sonnet", "medium"),
      opencode,
      antigravity,
    ],
  },
  {
    role: "frontend",
    name: "pixel",
    roleLabel: "frontend",
    title: "Frontend",
    summary: "Components, styling, accessibility, polish",
    instructions:
      "You own user-facing work: components, styling, interaction states, and accessibility. Match the existing design system, keep markup semantic, and check the result in the running app when you can.",
    checkoutPolicy: "isolated",
    activationPolicy: "manual",
    authorityMode: "self-decision",
    optional: false,
    choices: [
      claude("sonnet", "medium"),
      codex("gpt-6.1-sol", "medium"),
      antigravity,
      opencode,
    ],
  },
  {
    role: "reviewer",
    name: "sentinel",
    roleLabel: "reviewer",
    title: "Reviewer",
    summary: "Reviews diffs for bugs, regressions, and tests",
    instructions:
      "You review changes; you do not edit files. Look for correctness bugs, regressions, security problems, and missing tests. Report findings ranked by severity, each with the file, the line, and a concrete failure scenario.",
    checkoutPolicy: "read-only",
    activationPolicy: "manual",
    authorityMode: "ask",
    optional: false,
    choices: [codex("gpt-6-astra", "high"), claude("opus", "high"), opencode],
  },
  {
    role: "researcher",
    name: "scout",
    roleLabel: "research",
    title: "Researcher",
    summary: "Reads large codebases, logs, and docs fast",
    instructions:
      "You read quickly and widely: map unfamiliar code, trace how a feature works, summarise long logs, and draft documentation. Cite the files you relied on. Do not change code.",
    checkoutPolicy: "read-only",
    activationPolicy: "manual",
    authorityMode: "ask",
    optional: true,
    choices: [antigravity, claude("sonnet", "low")],
  },
  {
    role: "quick",
    name: "dash",
    roleLabel: "quick fixes",
    title: "Quick fixes",
    summary: "Small edits: renames, configs, one-file fixes",
    instructions:
      "You handle small, fast edits: renames, configuration tweaks, one-file fixes, and scripts. Make the change, check it, and stop.",
    checkoutPolicy: "isolated",
    activationPolicy: "manual",
    authorityMode: "self-decision",
    optional: true,
    choices: [opencode, claude("haiku", "low")],
  },
];

/** Why a harness and model were picked, in one line, for the setup dialog. */
export function whyChoice(role: PresetRole, choice: HarnessChoice): string {
  const key = `${role}:${choice.provider}`;
  switch (key) {
    case "lead:claude-code":
      return "Opus is the community pick for planning and long agentic runs.";
    case "lead:codex":
      return "GPT-6 Astra handles ambiguous, multi-hour work in Codex.";
    case "builder:codex":
      return "GPT-6.1 Sol is Codex's everyday model; strong on mechanical refactors.";
    case "builder:claude-code":
      return "Sonnet is fast and dependable for everyday feature work.";
    case "frontend:claude-code":
      return "Claude is most often preferred for UI and styling work.";
    case "frontend:codex":
      return "GPT-6.1 Sol produces clean component code at speed.";
    case "reviewer:codex":
      return "A different vendor reviewing than writing catches more; Astra is thorough.";
    case "reviewer:claude-code":
      return "Opus is careful with cross-module reasoning and regressions.";
    case "researcher:antigravity":
      return "Gemini reads very large contexts quickly; good for docs.";
    case "researcher:claude-code":
      return "Sonnet at low effort reads and summarises cheaply.";
    case "quick:opencode":
      return "OpenCode runs small edits on the model you already set up there.";
    case "quick:claude-code":
      return "Haiku is the fastest Claude for small, well-defined edits.";
    default:
      return "Uses the harness's own default model.";
  }
}

/**
 * The team to offer on this machine: one draft per role whose harness is
 * detected. Names already taken by an enabled employee start unticked.
 */
export function recommendTeam(
  providers: ProviderListEntry[],
  takenNames: string[] = [],
): AgentDraft[] {
  const detected = new Set(
    providers.filter((entry) => entry.detected).map((entry) => entry.provider),
  );
  const taken = new Set(takenNames.map((name) => name.trim().toLowerCase()));
  const drafts: AgentDraft[] = [];
  for (const preset of PRESETS) {
    if (preset.optional && !detected.has(preset.choices[0]!.provider)) continue;
    const choice = preset.choices.find((entry) => detected.has(entry.provider));
    if (!choice) continue;
    const exists = taken.has(preset.name);
    drafts.push({
      key: preset.role,
      include: !exists,
      name: preset.name,
      roleLabel: preset.roleLabel,
      title: preset.title,
      summary: preset.summary,
      why: whyChoice(preset.role, choice),
      provider: choice.provider,
      model: choice.model ?? "",
      reasoningEffort: choice.reasoningEffort ?? "",
      authorityMode: preset.authorityMode,
      checkoutPolicy: preset.checkoutPolicy,
      activationPolicy: preset.activationPolicy,
      instructions: preset.instructions,
      exists,
    });
  }
  return drafts;
}

/** Model suggestions per harness for the editable model field. */
export const MODEL_SUGGESTIONS: Record<string, string[]> = {
  "claude-code": ["opus", "sonnet", "haiku", "fable"],
  codex: ["gpt-6-astra", "gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna"],
  antigravity: ["gemini-3.5-flash", "gemini-3.1-pro"],
  opencode: [],
  "grok-build": [],
};

/** Reasoning effort values each harness accepts; empty means not settable. */
export const EFFORTS: Record<string, string[]> = {
  "claude-code": ["low", "medium", "high", "xhigh", "max"],
  codex: ["low", "medium", "high", "xhigh"],
  antigravity: [],
  opencode: [],
  "grok-build": [],
};

/** The `employee.create` parameters for a draft. */
export function employeeParams(draft: AgentDraft) {
  const efforts = EFFORTS[draft.provider] ?? [];
  return {
    name: draft.name.trim(),
    provider: draft.provider,
    role: draft.roleLabel,
    instructions: draft.instructions,
    model: draft.model.trim() || null,
    reasoningEffort: efforts.includes(draft.reasoningEffort)
      ? draft.reasoningEffort
      : null,
    nativeSettings: {},
    authorityMode: draft.authorityMode,
    checkoutPolicy: draft.checkoutPolicy,
    activationPolicy: draft.activationPolicy,
    idleTimeoutSeconds: 0,
    enabled: true,
  };
}
