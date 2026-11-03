// Skills and plugins fixtures for the tests and the `?mock=1` development
// bridge. Shapes follow docs/UI-BACKEND-HANDOFF.md, "Skills and plugins".

import type {
  ExtensionTarget,
  LibrarySkill,
  PluginCapability,
  PluginListResult,
  PluginObservation,
  PluginPreview,
  SkillExposure,
  SkillInfo,
  SkillInventoryEntry,
  SkillListResult,
  SkillPreview,
  SkillTargetInfo,
} from "../lib/protocol";

export const EXT_HOME = "C:\\Users\\dev";
export const EXT_LIBRARY = `${EXT_HOME}\\AppData\\Local\\Lazoo\\extensions\\library`;
export const PROFILE_ID = "acc00000-0000-4000-8000-000000000001";
export const PREVIEW_ID = "99999999-9999-4999-8999-999999999999";

export const personal = (provider: string): ExtensionTarget => ({
  provider,
  accountId: null,
});

export function skillInfo(
  name: string,
  overrides: Partial<SkillInfo> = {},
): SkillInfo {
  return {
    name,
    description: `Helps with ${name.replace(/-/g, " ")}.`,
    relativePath: `skills/${name}`,
    fileCount: 3,
    bytes: 4096,
    checksum: `sha256:${name.padEnd(12, "0")}`,
    problems: [],
    ...overrides,
  };
}

export function librarySkill(
  name: string,
  overrides: Partial<LibrarySkill> = {},
): LibrarySkill {
  return {
    ...skillInfo(name),
    source: {
      source: "https://github.com/acme/skills",
      sourceKind: "github",
      gitRef: null,
    },
    resolvedRevision: "0a1b2c3d4e5f60718293a4b5c6d7e8f901234567",
    revision: 1,
    ...overrides,
  };
}

export function skillExposure(
  name: string,
  provider: string,
  overrides: Partial<SkillExposure> = {},
): SkillExposure {
  return {
    name,
    target: personal(provider),
    path: `${EXT_HOME}\\.${provider}\\skills\\${name}`,
    mode: "link",
    checksum: skillInfo(name).checksum,
    libraryPath: `${EXT_LIBRARY}\\${name}`,
    ...overrides,
  };
}

export function inventoryEntry(
  name: string,
  provider: string,
  overrides: Partial<SkillInventoryEntry> = {},
): SkillInventoryEntry {
  const path = `${EXT_HOME}\\.${provider}\\skills\\${name}`;
  return {
    provider,
    target: personal(provider),
    name,
    description: skillInfo(name).description,
    path,
    canonicalPath: path,
    kind: "real-dir",
    managed: false,
    checksum: skillInfo(name).checksum,
    fileCount: 3,
    bytes: 4096,
    problems: [],
    enabled: null,
    duplicate: false,
    alsoVisibleTo: [],
    ...overrides,
  };
}

export const SKILL_PROVIDERS = [
  "claude-code",
  "codex",
  "github-copilot",
  "opencode",
  "antigravity",
];

export function skillTargets(): SkillTargetInfo[] {
  return SKILL_PROVIDERS.map((provider) => ({
    target: personal(provider),
    skillRoots: [`${EXT_HOME}\\.${provider}\\skills`],
    exposureDirectory: `${EXT_HOME}\\.${provider}\\skills`,
    compatibility: "Native" as const,
    limitations:
      provider === "antigravity"
        ? [
            "global root inferred from documented config location; installed CLI has no read-only skill listing",
          ]
        : [],
  }));
}

export function skillList(
  overrides: Partial<SkillListResult> = {},
): SkillListResult {
  return {
    revision: 7,
    library: [
      librarySkill("code-review"),
      librarySkill("write-tests", {
        description: "Writes focused tests before the change.",
        source: {
          source: "C:\\skills\\write-tests",
          sourceKind: "local",
          gitRef: null,
        },
        resolvedRevision: null,
      }),
    ],
    exposures: [
      skillExposure("code-review", "claude-code", {
        path: `${EXT_HOME}\\.claude\\skills\\code-review`,
      }),
      skillExposure("code-review", "codex"),
    ],
    // Claude Code's own folder is also read by OpenCode, so both report the
    // same managed path and the same library bytes.
    inventory: [
      inventoryEntry("code-review", "claude-code", {
        path: `${EXT_HOME}\\.claude\\skills\\code-review`,
        canonicalPath: `${EXT_LIBRARY}\\code-review`,
        kind: "link",
        managed: true,
        alsoVisibleTo: [personal("opencode")],
      }),
      inventoryEntry("code-review", "opencode", {
        path: `${EXT_HOME}\\.claude\\skills\\code-review`,
        canonicalPath: `${EXT_LIBRARY}\\code-review`,
        kind: "link",
        managed: true,
        alsoVisibleTo: [personal("claude-code")],
      }),
      inventoryEntry("pdf-forms", "claude-code"),
      inventoryEntry("pdf-forms", "codex", { duplicate: true }),
    ],
    problems: [],
    pending: null,
    targets: skillTargets(),
    ...overrides,
  };
}

export function skillPreview(
  names: string[] = ["pdf-forms"],
  overrides: Partial<SkillPreview> = {},
): SkillPreview {
  return {
    previewId: PREVIEW_ID,
    revision: 7,
    expiresAt: "2026-11-04T10:10:00Z",
    source: {
      source: "https://github.com/acme/more-skills",
      sourceKind: "github",
      gitRef: null,
    },
    skills: names.map((name) => skillInfo(name)),
    resolvedRevision: "fedcba9876543210fedcba9876543210fedcba98",
    conflicts: [],
    updateName: null,
    ...overrides,
  };
}

export function pluginCapabilities(): PluginCapability[] {
  const base = {
    sessionEffect: "Requires provider restart",
    authentication: "Requires user OAuth or native setup",
  };
  return [
    {
      provider: "claude-code",
      compatibility: "Native",
      actions: [
        "list",
        "marketplace.list",
        "marketplace.add",
        "marketplace.remove",
        "install",
        "uninstall",
        "enable",
        "disable",
      ],
      limitations: ["user scope only"],
      ...base,
    },
    {
      provider: "codex",
      compatibility: "Native",
      actions: [
        "list",
        "marketplace.list",
        "marketplace.add",
        "marketplace.remove",
        "install",
        "uninstall",
      ],
      limitations: [
        "enable/disable unsupported: no native CLI command; reviewed TOML writer is MCP-only",
      ],
      ...base,
    },
    {
      provider: "github-copilot",
      compatibility: "Native",
      actions: [
        "list",
        "marketplace.list",
        "marketplace.add",
        "marketplace.remove",
        "install",
        "uninstall",
      ],
      limitations: ["enable/disable unsupported"],
      ...base,
    },
    {
      provider: "antigravity",
      compatibility: "Native",
      actions: ["list", "install", "uninstall", "enable", "disable"],
      limitations: ["marketplace management unsupported"],
      ...base,
    },
    {
      provider: "opencode",
      compatibility: "Translated with documented limitations",
      actions: ["list", "install"],
      limitations: ["npm modules only; removal and toggles unsupported"],
      ...base,
    },
    {
      provider: "grok-build",
      compatibility: "Unsupported",
      actions: [],
      limitations: ["excluded from core v1"],
      ...base,
    },
  ];
}

export function pluginObservation(
  overrides: Partial<PluginObservation> = {},
): PluginObservation {
  return {
    plugins: [],
    marketplaces: [],
    providerVersion: "1.0.0",
    compatibility: "Native",
    limitations: [],
    ...overrides,
  };
}

export function pluginList(
  overrides: Partial<PluginListResult> = {},
): PluginListResult {
  return {
    revision: 7,
    targets: [
      {
        target: personal("claude-code"),
        observed: pluginObservation({
          plugins: [
            {
              name: "superpowers",
              marketplace: "acme-market",
              version: "1.4.0",
              enabled: true,
              path: `${EXT_HOME}\\.claude\\plugins\\superpowers`,
            },
          ],
          marketplaces: [{ name: "acme-market", source: "acme/market" }],
        }),
        error: null,
      },
      {
        target: personal("codex"),
        observed: pluginObservation({
          plugins: [
            {
              name: "notes",
              marketplace: "acme-market",
              version: "0.3.1",
              enabled: true,
            },
          ],
          marketplaces: [{ name: "acme-market", source: "acme/market" }],
        }),
        error: null,
      },
      {
        target: { provider: "codex", accountId: PROFILE_ID },
        observed: pluginObservation(),
        error: null,
      },
      {
        target: personal("github-copilot"),
        observed: pluginObservation(),
        error: null,
      },
      {
        target: personal("opencode"),
        observed: pluginObservation({
          plugins: [{ name: "opencode-helper", enabled: null }],
        }),
        error: null,
      },
      {
        target: personal("antigravity"),
        observed: null,
        error: {
          code: "LZ-EXTENSION-NOT-DETECTED",
          message: "native harness executable was not detected",
        },
      },
    ],
    capabilities: pluginCapabilities(),
    pending: null,
    ...overrides,
  };
}

export function pluginPreview(
  request: PluginPreview["request"],
  overrides: Partial<PluginPreview> = {},
): PluginPreview {
  return {
    previewId: PREVIEW_ID,
    revision: 7,
    expiresAt: "2026-11-04T10:10:00Z",
    request,
    targets: request.targets.map((target) => ({
      target,
      before: pluginObservation(),
      commands: [["plugin", request.action, request.name]],
      error: null,
    })),
    ...overrides,
  };
}
