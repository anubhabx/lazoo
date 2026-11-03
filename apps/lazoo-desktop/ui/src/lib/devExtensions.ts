// Development only: a believable skills and plugins daemon for `?mock=1`.
// It keeps its own small ledger so install, expose, update, remove and every
// plugin action visibly change what the pages show.

import {
  PREVIEW_ID,
  inventoryEntry,
  librarySkill,
  personal,
  pluginCapabilities,
  pluginList,
  pluginObservation,
  skillExposure,
  skillInfo,
  skillList,
  skillPreview,
} from "../test/extensionFixtures";
import type {
  ExposureMode,
  ExtensionTarget,
  LibrarySkill,
  PluginAction,
  PluginMarketplaceRecord,
  PluginPreview,
  PluginRecord,
  PluginTargetState,
  SkillExposure,
  SkillPreview,
} from "./protocol";

type Handler = (params: Record<string, unknown>) => unknown;

const key = (target: ExtensionTarget) =>
  `${target.provider}:${target.accountId ?? "personal"}`;

function fail(code: string, message: string): never {
  throw { code, message };
}

export function createExtensionHandlers(
  emit: (eventType: string) => void,
): Record<string, Handler> {
  let revision = 7;
  let library: LibrarySkill[] = skillList().library;
  let exposures: SkillExposure[] = skillList().exposures;
  const skillPreviews = new Map<string, SkillPreview>();
  const pluginPreviews = new Map<string, PluginPreview>();
  const plugins = new Map<
    string,
    { plugins: PluginRecord[]; marketplaces: PluginMarketplaceRecord[] }
  >(
    pluginList().targets.map((entry) => [
      key(entry.target),
      {
        plugins: [...(entry.observed?.plugins ?? [])],
        marketplaces: [...(entry.observed?.marketplaces ?? [])],
      },
    ]),
  );
  let previewCount = 0;

  /** The ledger's event sequence: started, finished, then the changed event. */
  function finish(family: "skill" | "plugin") {
    emit("extension.operation.started");
    revision += 1;
    emit("extension.operation.finished");
    emit(`${family}.changed`);
  }

  function checkRevision(expected: unknown) {
    if (expected !== revision)
      fail(
        "LZ-EXTENSION-STALE",
        "extension revision changed; refresh and preview again",
      );
  }

  const outcome = (method: string, status = "applied") => ({
    operationId: `00000000-0000-4000-8000-${String(revision).padStart(12, "0")}`,
    kind: "skill",
    method,
    status,
    error: null,
    backupDirectory:
      "C:\\Users\\dev\\AppData\\Local\\Lazoo\\extensions\\backup",
    requiresProviderRestart: true,
  });

  function listResult() {
    const base = skillList();
    return {
      ...base,
      revision,
      library,
      exposures,
      inventory: [
        ...exposures.map((entry) =>
          inventoryEntry(entry.name, entry.target.provider, {
            path: entry.path,
            canonicalPath: entry.libraryPath,
            kind: entry.mode === "link" ? ("link" as const) : ("copy" as const),
            managed: true,
          }),
        ),
        ...base.inventory.filter(
          (entry) => !entry.managed && entry.name !== "code-review",
        ),
      ],
    };
  }

  function sourcePreview(params: Record<string, unknown>): SkillPreview {
    const source = String(params.source ?? "");
    if (!source.trim())
      fail("LZ-INVALID-PARAMS", "invalid extension parameters");
    const kind = String(params.sourceKind ?? "auto");
    const names = ["pdf-forms", "api-design", "code-review"];
    const found = skillPreview(names, {
      revision,
      source: {
        source,
        sourceKind: kind as SkillPreview["source"]["sourceKind"],
        gitRef: (params.gitRef as string | null | undefined) ?? null,
      },
      resolvedRevision:
        kind === "local" ? null : "5d3c9a1b7e2f4c80a6b19d0e3f7a5c2b8d4e6f10",
    });
    found.skills = [
      ...found.skills.slice(0, 2),
      skillInfo("code-review", {
        checksum: "sha256:code-review-v2",
        fileCount: 4,
      }),
      skillInfo("Bad Skill", {
        problems: ["name must be lowercase letters, digits and hyphens"],
      }),
    ];
    found.conflicts = [
      {
        name: "code-review",
        scope: "library",
        kind: "conflict",
        replaceAllowed: true,
      },
      {
        name: "pdf-forms",
        scope: "harness",
        kind: "duplicate",
        replaceAllowed: false,
        target: personal("claude-code"),
        path: "C:\\Users\\dev\\.claude-code\\skills\\pdf-forms",
        managed: false,
      },
    ];
    skillPreviews.set(found.previewId, found);
    return found;
  }

  function install(params: Record<string, unknown>, method: string) {
    checkRevision(params.expectedRevision);
    const preview = skillPreviews.get(String(params.previewId));
    if (!preview)
      fail(
        "LZ-EXTENSION-PREVIEW-EXPIRED",
        "preview is missing, consumed or expired; preview again",
      );
    const mode = (params.mode as ExposureMode) ?? "link";
    for (const name of params.names as string[]) {
      const info = preview.skills.find((skill) => skill.name === name);
      if (!info) continue;
      const old = library.find((skill) => skill.name === name);
      if (old && old.checksum !== info.checksum && !params.replace)
        fail(
          "LZ-SKILL-CONFLICT",
          "same-name library content differs; explicit replace is required",
        );
      library = [
        ...library.filter((skill) => skill.name !== name),
        {
          ...librarySkill(name),
          ...info,
          source: preview.source,
          resolvedRevision: preview.resolvedRevision,
          revision: (old?.revision ?? 0) + 1,
        },
      ].sort((a, b) => a.name.localeCompare(b.name));
      exposures = exposures.map((entry) =>
        entry.name === name ? { ...entry, checksum: info.checksum } : entry,
      );
      for (const provider of (params.providers as string[]) ?? []) {
        exposures = [
          ...exposures.filter(
            (entry) =>
              !(entry.name === name && entry.target.provider === provider),
          ),
          skillExposure(name, provider, { mode, checksum: info.checksum }),
        ];
      }
    }
    skillPreviews.delete(preview.previewId);
    finish("skill");
    return { revision, outcome: outcome(method), library, exposures };
  }

  function planCommands(
    provider: string,
    action: PluginAction,
    name: string,
    marketplace?: { name: string; source: string },
  ): string[][] {
    const plan: string[][] = [];
    if (action === "install" && marketplace)
      plan.push(["plugin", "marketplace", "add", marketplace.source]);
    if (action === "marketplace-add")
      return [["plugin", "marketplace", "add", marketplace?.source ?? name]];
    if (action === "marketplace-remove")
      return [["plugin", "marketplace", "remove", name]];
    const verb =
      action === "install"
        ? provider === "codex"
          ? "add"
          : "install"
        : action === "uninstall"
          ? provider === "codex"
            ? "remove"
            : "uninstall"
          : action;
    plan.push(
      provider === "claude-code"
        ? ["plugin", verb, name, "--scope", "user"]
        : ["plugin", verb, name],
    );
    return plan;
  }

  function pluginPreviewResult(params: Record<string, unknown>): PluginPreview {
    const targets = params.targets as ExtensionTarget[];
    const action = params.action as PluginAction;
    const name = String(params.name);
    const marketplace = params.marketplace as
      { name: string; source: string } | undefined;
    previewCount += 1;
    const caps = pluginCapabilities();
    const result: PluginPreview = {
      previewId:
        previewCount === 1
          ? PREVIEW_ID
          : `99999999-9999-4999-8999-${String(previewCount).padStart(12, "0")}`,
      revision,
      expiresAt: new Date(Date.now() + 10 * 60_000).toISOString(),
      request: {
        action,
        targets,
        name,
        ...(marketplace ? { marketplace } : {}),
      },
      targets: targets.map((target) => {
        const cap = caps.find((entry) => entry.provider === target.provider);
        const state = plugins.get(key(target));
        const verb = action.replace("marketplace-", "marketplace.");
        if (!cap?.actions.includes(verb))
          return {
            target,
            before: null,
            commands: [],
            error: {
              code: "LZ-EXTENSION-UNSUPPORTED",
              message:
                "unsupported: native plugin action is unavailable; inspect capabilities",
            },
          };
        return {
          target,
          before: pluginObservation({
            plugins: state?.plugins ?? [],
            marketplaces: state?.marketplaces ?? [],
          }),
          commands: planCommands(target.provider, action, name, marketplace),
          error: null,
        };
      }),
    };
    pluginPreviews.set(result.previewId, result);
    return result;
  }

  function applyPlugin(params: Record<string, unknown>, method: string) {
    checkRevision(params.expectedRevision);
    const preview = pluginPreviews.get(String(params.previewId));
    if (!preview)
      fail(
        "LZ-EXTENSION-PREVIEW-EXPIRED",
        "plugin preview is missing, consumed or expired",
      );
    const { action, name, marketplace } = preview.request;
    const results = preview.targets.map((planned) => {
      if (planned.error)
        return {
          target: planned.target,
          status: "unsupported-or-unavailable",
          error: planned.error,
          observed: null,
          rollbackStatus: "not-needed",
        };
      const state = plugins.get(key(planned.target)) ?? {
        plugins: [],
        marketplaces: [],
      };
      const [base] = name.split("@");
      if (action === "install") {
        if (
          marketplace &&
          !state.marketplaces.some((m) => m.name === marketplace.name)
        )
          state.marketplaces.push({
            name: marketplace.name,
            source: marketplace.source,
          });
        state.plugins.push({
          name: base!,
          marketplace: marketplace?.name ?? name.split("@")[1] ?? null,
          version: "1.0.0",
          enabled: true,
        });
      } else if (action === "uninstall") {
        state.plugins = state.plugins.filter((plugin) => plugin.name !== name);
      } else if (action === "enable" || action === "disable") {
        state.plugins = state.plugins.map((plugin) =>
          plugin.name === name
            ? { ...plugin, enabled: action === "enable" }
            : plugin,
        );
      } else if (action === "marketplace-add" && marketplace) {
        state.marketplaces.push({
          name: marketplace.name,
          source: marketplace.source,
        });
      } else if (action === "marketplace-remove") {
        state.marketplaces = state.marketplaces.filter((m) => m.name !== name);
      }
      plugins.set(key(planned.target), state);
      return {
        target: planned.target,
        status: "applied",
        error: null,
        observed: null,
        rollbackStatus: "not-needed",
      };
    });
    pluginPreviews.delete(preview.previewId);
    finish("plugin");
    return {
      revision,
      outcome: {
        operationId: "00000000-0000-4000-8000-0000000000aa",
        kind: "plugin",
        method,
        targets: results,
        sessionEffect: "Requires provider restart",
      },
    };
  }

  return {
    "skill.list": () => listResult(),
    "skill.source.preview": (params) => sourcePreview(params),
    "skill.update.preview": (params) => {
      const name = String(params.name);
      const old = library.find((skill) => skill.name === name);
      if (!old) fail("LZ-INVALID-PARAMS", "skill is not in the Lazoo library");
      const next = skillInfo(name, {
        checksum: `sha256:${name}-newer`,
        fileCount: old.fileCount + 1,
        bytes: old.bytes + 512,
      });
      const preview = skillPreview([name], {
        revision,
        source: old.source,
        skills: [next],
        updateName: name,
        resolvedRevision: "c0ffee1234567890c0ffee1234567890c0ffee12",
      });
      skillPreviews.set(preview.previewId, preview);
      return preview;
    },
    "skill.install": (params) => install(params, "skill.install"),
    "skill.update": (params) => install(params, "skill.update"),
    "skill.expose": (params) => {
      checkRevision(params.expectedRevision);
      const name = String(params.name);
      for (const provider of params.providers as string[])
        exposures = [
          ...exposures.filter(
            (entry) =>
              !(entry.name === name && entry.target.provider === provider),
          ),
          skillExposure(name, provider, { mode: params.mode as ExposureMode }),
        ];
      finish("skill");
      return { revision, outcome: outcome("skill.expose"), library, exposures };
    },
    "skill.unexpose": (params) => {
      checkRevision(params.expectedRevision);
      const providers = params.providers as string[];
      exposures = exposures.filter(
        (entry) =>
          entry.name !== params.name ||
          !providers.includes(entry.target.provider),
      );
      finish("skill");
      return {
        revision,
        outcome: outcome("skill.unexpose"),
        library,
        exposures,
      };
    },
    "skill.remove": (params) => {
      checkRevision(params.expectedRevision);
      library = library.filter((skill) => skill.name !== params.name);
      exposures = exposures.filter((entry) => entry.name !== params.name);
      finish("skill");
      return { revision, outcome: outcome("skill.remove"), library, exposures };
    },
    "plugin.list": () => {
      const base = pluginList();
      const targets: PluginTargetState[] = base.targets.map((entry) => {
        const state = plugins.get(key(entry.target));
        if (!entry.observed || !state) return entry;
        return {
          ...entry,
          observed: {
            ...entry.observed,
            plugins: state.plugins,
            marketplaces: state.marketplaces,
          },
        };
      });
      return { ...base, revision, targets };
    },
    "plugin.preview": (params) => pluginPreviewResult(params),
    "plugin.install": (params) => applyPlugin(params, "plugin.install"),
    "plugin.uninstall": (params) => applyPlugin(params, "plugin.uninstall"),
    "plugin.enable": (params) => applyPlugin(params, "plugin.enable"),
    "plugin.disable": (params) => applyPlugin(params, "plugin.disable"),
    "plugin.marketplace.add": (params) =>
      applyPlugin(params, "plugin.marketplace.add"),
    "plugin.marketplace.remove": (params) =>
      applyPlugin(params, "plugin.marketplace.remove"),
    "plugin.marketplace.list": () => ({
      ...pluginList(),
      revision,
    }),
  };
}
