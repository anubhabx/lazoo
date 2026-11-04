import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { beforeEach, describe, expect, test } from "vitest";

import { BridgeProvider, StoreProvider } from "../lib/context";
import type {
  PluginMutationResult,
  PluginPreview,
  PluginTargetOutcome,
  SkillMutationResult,
} from "../lib/protocol";
import { Store, type SettingsSection } from "../lib/store";
import { createFakeBridge, type FakeBridge } from "../test/fakeBridge";
import {
  PREVIEW_ID,
  PROFILE_ID,
  librarySkill,
  personal,
  pluginList,
  pluginPreview,
  skillExposure,
  skillInfo,
  skillList,
  skillPreview,
} from "../test/extensionFixtures";
import { event, status } from "../test/fixtures";
import { SettingsPage } from "./Settings";

function mount(bridge: FakeBridge, section: SettingsSection, init = false) {
  const store = new Store(bridge, { statusPollMs: 0, debounceMs: 10 });
  store.navigate({ kind: "settings", section });
  const view = render(
    <StoreProvider value={store}>
      <BridgeProvider value={bridge}>
        <SettingsPage />
      </BridgeProvider>
    </StoreProvider>,
  );
  if (init) void store.init();
  return { store, ...view };
}

function skillOutcome(
  method: string,
  status: "applied" | "rolled-back" | "recovery-needed" = "applied",
): SkillMutationResult {
  return {
    revision: 8,
    outcome: {
      operationId: "11111111-0000-4000-8000-000000000000",
      kind: "skill",
      method,
      status,
      error:
        status === "applied"
          ? null
          : {
              code: "LZ-EXTENSION-IO",
              message: "extension filesystem operation failed",
            },
      backupDirectory: "C:\\Lazoo\\extensions\\operations\\abc",
      requiresProviderRestart: true,
    },
    library: [],
    exposures: [],
  };
}

function pluginOutcome(
  method: string,
  targets: PluginTargetOutcome[],
): PluginMutationResult {
  return {
    revision: 8,
    outcome: {
      operationId: "22222222-0000-4000-8000-000000000000",
      kind: "plugin",
      method,
      targets,
      sessionEffect: "Requires provider restart",
    },
  };
}

function skillBridge() {
  const bridge = createFakeBridge();
  bridge.on("system.status", status());
  bridge.on("skill.list", () => skillList());
  return bridge;
}

function pluginBridge() {
  const bridge = createFakeBridge();
  bridge.on("system.status", status());
  bridge.on("plugin.list", () => pluginList());
  bridge.on("provider.accounts.list", {
    version: 1,
    accounts: [
      {
        spec: {
          id: PROFILE_ID,
          provider: "codex",
          label: "Work",
          credentialKind: "native-profile",
        },
      },
    ],
    policies: [],
  });
  return bridge;
}

async function openAddDialog(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: "Add skills" }));
  return screen.findByRole("dialog", { name: "Add skills" });
}

beforeEach(() => localStorage.clear());

describe("Skills page", () => {
  test("lists the library with its harness toggles and what else sees it", async () => {
    const bridge = skillBridge();
    const { container } = mount(bridge, "skills");
    const library = await screen.findByRole("list", { name: "Library skills" });
    const rows = within(library).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("code-review");
    expect(rows[0]).toHaveTextContent("Helps with code review.");
    expect(rows[0]).toHaveTextContent("acme/skills");
    expect(rows[0]).toHaveTextContent("0a1b2c3");
    expect(rows[1]).toHaveTextContent("write-tests");

    const pressed = (name: string) =>
      within(rows[0]!)
        .getByRole("button", { name })
        .getAttribute("aria-pressed");
    expect(pressed("Claude Code for code-review")).toBe("true");
    expect(pressed("Codex for code-review")).toBe("true");
    expect(pressed("OpenCode for code-review")).toBe("false");
    expect(rows[0]).toHaveTextContent(
      "Also visible to OpenCode through ~/.claude/skills",
    );

    // The unmanaged group lists skills Lazoo does not own, once per name.
    const unmanaged = screen.getByRole("list", { name: "Unmanaged skills" });
    const group = within(unmanaged)
      .getAllByRole("listitem")
      .filter((item) => item.tagName === "DIV");
    expect(group).toHaveLength(1);
    expect(group[0]).toHaveTextContent("pdf-forms");
    expect(group[0]).toHaveTextContent("Claude Code");
    expect(group[0]).toHaveTextContent("Codex");
    expect(group[0]).toHaveTextContent("real-dir");
    expect(group[0]).toHaveTextContent("duplicate");

    const results = await axe.run(container, {
      runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] },
    });
    expect(
      results.violations.filter(
        (v) => v.impact === "critical" || v.impact === "serious",
      ),
    ).toEqual([]);
  });

  test("a harness toggle exposes or unexposes with the revision and the library's mode", async () => {
    const bridge = skillBridge();
    bridge.on("skill.expose", () => skillOutcome("skill.expose"));
    bridge.on("skill.unexpose", () => skillOutcome("skill.unexpose"));
    const user = userEvent.setup();
    mount(bridge, "skills");
    await user.click(
      await screen.findByRole("button", { name: "OpenCode for code-review" }),
    );
    await waitFor(() =>
      expect(bridge.callsTo("skill.expose")[0]).toEqual({
        name: "code-review",
        providers: ["opencode"],
        mode: "link",
        expectedRevision: 7,
      }),
    );
    expect(
      await screen.findByText(
        /Running sessions pick this up after they restart/,
      ),
    ).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "Codex for code-review" }),
    );
    await waitFor(() =>
      expect(bridge.callsTo("skill.unexpose")[0]).toEqual({
        name: "code-review",
        providers: ["codex"],
        expectedRevision: 7,
      }),
    );
  });

  test("an exposure keeps its copy mode when another harness is added", async () => {
    const bridge = skillBridge();
    bridge.on("skill.list", () =>
      skillList({
        exposures: [skillExposure("code-review", "codex", { mode: "copy" })],
      }),
    );
    bridge.on("skill.expose", () => skillOutcome("skill.expose"));
    const user = userEvent.setup();
    mount(bridge, "skills");
    await user.click(
      await screen.findByRole("button", {
        name: "Claude Code for code-review",
      }),
    );
    await waitFor(() =>
      expect(bridge.callsTo("skill.expose")[0]).toMatchObject({ mode: "copy" }),
    );
  });

  test("a stale revision reloads the list and asks to try again", async () => {
    const bridge = skillBridge();
    bridge.fail("skill.expose", {
      code: "LZ-EXTENSION-STALE",
      message: "extension revision changed; refresh and preview again",
    });
    const user = userEvent.setup();
    mount(bridge, "skills");
    await user.click(
      await screen.findByRole("button", { name: "OpenCode for code-review" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "LZ-EXTENSION-STALE",
    );
    expect(screen.getByRole("alert")).toHaveTextContent("try once more");
    await waitFor(() => expect(bridge.callsTo("skill.list")).toHaveLength(2));
  });

  test("an interrupted journal pauses changes and names the paths", async () => {
    const bridge = skillBridge();
    bridge.on("skill.list", () =>
      skillList({
        pending: {
          operationId: "x",
          kind: "skill",
          journal: "C:\\Lazoo\\extensions\\operations\\abc\\journal.json",
        },
      }),
    );
    mount(bridge, "skills");
    const note = await screen.findByText("Changes are paused.");
    expect(note.closest(".ext-note")).toHaveTextContent(
      "C:\\Lazoo\\extensions\\operations\\abc\\journal.json",
    );
    expect(
      screen.getByRole("button", { name: "OpenCode for code-review" }),
    ).toBeDisabled();
  });

  test("a skill change event re-reads the library", async () => {
    const bridge = skillBridge();
    mount(bridge, "skills", true);
    await screen.findByRole("list", { name: "Library skills" });
    await waitFor(() => expect(bridge.subscribed.length).toBe(1));
    expect(bridge.callsTo("skill.list")).toHaveLength(1);
    bridge.emit(event(500, "extension.operation.started"));
    expect(
      await screen.findByText(/A change is in progress/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "OpenCode for code-review" }),
    ).toBeDisabled();
    bridge.emit(event(501, "extension.operation.finished"));
    bridge.emit(event(502, "skill.changed"));
    await waitFor(() => expect(bridge.callsTo("skill.list")).toHaveLength(2));
    await waitFor(() =>
      expect(screen.queryByText(/A change is in progress/)).toBeNull(),
    );
  });

  test("Remove names what it deletes and asks before it calls", async () => {
    const bridge = skillBridge();
    bridge.on("skill.remove", () => skillOutcome("skill.remove"));
    const user = userEvent.setup();
    mount(bridge, "skills");
    await user.click(
      await screen.findByRole("button", { name: "More for code-review" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Remove code-review",
    });
    expect(dialog).toHaveTextContent("Lazoo's library copy");
    expect(dialog).toHaveTextContent("~\\.claude\\skills\\code-review");
    expect(dialog).toHaveTextContent("The link in Codex");
    expect(bridge.callsTo("skill.remove")).toHaveLength(0);
    await user.click(
      within(dialog).getByRole("button", { name: "Remove skill" }),
    );
    await waitFor(() =>
      expect(bridge.callsTo("skill.remove")[0]).toEqual({
        name: "code-review",
        expectedRevision: 7,
      }),
    );
  });

  test("Update previews old against new, then replaces", async () => {
    const bridge = skillBridge();
    bridge.on("skill.update.preview", () =>
      skillPreview(["code-review"], {
        updateName: "code-review",
        skills: [
          skillInfo("code-review", {
            checksum: "sha256:newer-checksum",
            fileCount: 5,
          }),
        ],
        resolvedRevision: "abcdef0123456789abcdef0123456789abcdef01",
      }),
    );
    bridge.on("skill.update", () => skillOutcome("skill.update"));
    const user = userEvent.setup();
    mount(bridge, "skills");
    await user.click(
      await screen.findByRole("button", { name: "More for code-review" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Update" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Update code-review",
    });
    expect(bridge.callsTo("skill.update.preview")[0]).toEqual({
      name: "code-review",
    });
    await within(dialog).findByText("abcdef0");
    expect(dialog).toHaveTextContent("0a1b2c3");
    expect(dialog).toHaveTextContent("newer-checks");
    await user.click(within(dialog).getByRole("button", { name: "Update" }));
    await waitFor(() =>
      expect(bridge.callsTo("skill.update")[0]).toEqual({
        previewId: PREVIEW_ID,
        names: ["code-review"],
        providers: [],
        mode: "link",
        replace: true,
        expectedRevision: 7,
      }),
    );
  });

  test("adding skills previews the source, then installs exactly what was chosen", async () => {
    const bridge = skillBridge();
    bridge.on("skill.source.preview", () =>
      skillPreview(["pdf-forms", "api-design"], {
        skills: [
          skillInfo("pdf-forms"),
          skillInfo("api-design"),
          skillInfo("Bad Skill", {
            problems: ["name must be lowercase"],
          }),
        ],
      }),
    );
    bridge.on("skill.install", () => skillOutcome("skill.install"));
    const user = userEvent.setup();
    mount(bridge, "skills");
    const dialog = await openAddDialog(user);
    await user.type(
      within(dialog).getByLabelText("Source"),
      "acme/more-skills",
    );
    await user.type(
      within(dialog).getByLabelText("Branch, tag or commit"),
      "v2",
    );
    await user.click(
      within(dialog).getByRole("button", { name: "Find skills" }),
    );
    await within(dialog).findByText("Skills found");
    expect(bridge.callsTo("skill.source.preview")[0]).toEqual({
      source: "acme/more-skills",
      sourceKind: "github",
      gitRef: "v2",
    });
    expect(dialog).toHaveTextContent("acme/more-skills");
    expect(dialog).toHaveTextContent("fedcba9");
    // A skill with problems is unchecked, disabled and explains why.
    expect(
      within(dialog).getByRole("checkbox", { name: /Bad Skill/ }),
    ).toBeDisabled();
    expect(dialog).toHaveTextContent("name must be lowercase");
    // Antigravity's unverified note shows once it is selected.
    expect(dialog).not.toHaveTextContent("Unverified");
    await user.click(
      within(dialog).getByRole("checkbox", { name: /Antigravity/ }),
    );
    expect(dialog).toHaveTextContent("Unverified");
    await user.click(within(dialog).getByRole("checkbox", { name: /Codex/ }));
    // Several installable skills: none is chosen for the user.
    expect(
      within(dialog).getByRole("checkbox", { name: /api-design/ }),
    ).not.toBeChecked();
    await user.click(
      within(dialog).getByRole("checkbox", { name: /pdf-forms/ }),
    );
    await user.click(within(dialog).getByRole("button", { name: "Copy" }));
    await user.click(
      within(dialog).getByRole("button", { name: "Install 1 skill" }),
    );
    await waitFor(() =>
      expect(bridge.callsTo("skill.install")[0]).toEqual({
        previewId: PREVIEW_ID,
        names: ["pdf-forms"],
        providers: ["codex", "antigravity"],
        mode: "copy",
        replace: false,
        expectedRevision: 7,
      }),
    );
    expect(await within(dialog).findByText("Installed 1 skill.")).toBeTruthy();
    expect(dialog).toHaveTextContent(
      "Running sessions pick this up after they restart.",
    );
  });

  test("a differing library skill installs only with Replace ticked", async () => {
    const bridge = skillBridge();
    bridge.on("skill.source.preview", () =>
      skillPreview(["code-review", "write-tests"], {
        skills: [
          skillInfo("code-review", { checksum: "sha256:other" }),
          skillInfo("write-tests", {
            checksum: librarySkill("write-tests").checksum,
          }),
        ],
        conflicts: [
          {
            name: "code-review",
            scope: "library",
            kind: "conflict",
            replaceAllowed: true,
          },
          {
            name: "write-tests",
            scope: "library",
            kind: "duplicate",
            replaceAllowed: true,
          },
          {
            name: "code-review",
            scope: "harness",
            kind: "conflict",
            replaceAllowed: false,
            target: personal("opencode"),
            path: "C:\\Users\\dev\\.claude\\skills\\code-review",
            managed: false,
          },
        ],
      }),
    );
    bridge.on("skill.install", () => skillOutcome("skill.install"));
    const user = userEvent.setup();
    mount(bridge, "skills");
    const dialog = await openAddDialog(user);
    await user.type(within(dialog).getByLabelText("Source"), "acme/skills");
    await user.click(
      within(dialog).getByRole("button", { name: "Find skills" }),
    );
    await within(dialog).findByText("Skills found");
    // The identical skill is skipped and cannot be chosen.
    expect(
      within(dialog).getByRole("checkbox", { name: /write-tests/ }),
    ).toBeDisabled();
    expect(dialog).toHaveTextContent("Already in your library and identical");
    const install = () =>
      within(dialog).getByRole("button", { name: "Install 1 skill" });
    await user.click(
      within(dialog).getByRole("checkbox", { name: /code-review/ }),
    );
    expect(install()).toBeDisabled();
    await user.click(
      within(dialog).getByRole("checkbox", { name: /^Replace/ }),
    );
    expect(install()).toBeEnabled();
    // An unmanaged copy a selected harness already sees blocks the install.
    await user.click(
      within(dialog).getByRole("checkbox", { name: /OpenCode/ }),
    );
    expect(install()).toBeDisabled();
    expect(dialog).toHaveTextContent("It blocks the install");
    await user.click(
      within(dialog).getByRole("checkbox", { name: /OpenCode/ }),
    );
    await user.click(install());
    await waitFor(() =>
      expect(bridge.callsTo("skill.install")[0]).toMatchObject({
        names: ["code-review"],
        providers: [],
        replace: true,
      }),
    );
  });

  test("a stale install returns to the source step with the source kept", async () => {
    const bridge = skillBridge();
    bridge.on("skill.source.preview", () => skillPreview(["pdf-forms"]));
    bridge.fail("skill.install", {
      code: "LZ-EXTENSION-DRIFT",
      message: "preview was created at a different revision",
    });
    const user = userEvent.setup();
    mount(bridge, "skills");
    const dialog = await openAddDialog(user);
    await user.type(within(dialog).getByLabelText("Source"), "acme/skills");
    await user.click(
      within(dialog).getByRole("button", { name: "Find skills" }),
    );
    await within(dialog).findByText("Skills found");
    await user.click(
      within(dialog).getByRole("button", { name: "Install 1 skill" }),
    );
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "LZ-EXTENSION-DRIFT",
    );
    expect(within(dialog).getByLabelText("Source")).toHaveValue("acme/skills");
    await waitFor(() => expect(bridge.callsTo("skill.list")).toHaveLength(2));
  });

  test("an expired preview returns to the source step", async () => {
    const bridge = skillBridge();
    bridge.on("skill.source.preview", () => skillPreview(["pdf-forms"]));
    bridge.fail("skill.install", {
      code: "LZ-EXTENSION-PREVIEW-EXPIRED",
      message: "preview is missing, consumed or expired; preview again",
    });
    const user = userEvent.setup();
    mount(bridge, "skills");
    const dialog = await openAddDialog(user);
    await user.type(within(dialog).getByLabelText("Source"), "acme/skills");
    await user.click(
      within(dialog).getByRole("button", { name: "Find skills" }),
    );
    await within(dialog).findByText("Skills found");
    await user.click(
      within(dialog).getByRole("button", { name: "Install 1 skill" }),
    );
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "The preview expired",
    );
    expect(within(dialog).getByLabelText("Source")).toHaveValue("acme/skills");
  });

  test("a rolled-back install is not shown as success", async () => {
    const bridge = skillBridge();
    bridge.on("skill.source.preview", () => skillPreview(["pdf-forms"]));
    bridge.on("skill.install", () =>
      skillOutcome("skill.install", "rolled-back"),
    );
    const user = userEvent.setup();
    mount(bridge, "skills");
    const dialog = await openAddDialog(user);
    await user.type(within(dialog).getByLabelText("Source"), "acme/skills");
    await user.click(
      within(dialog).getByRole("button", { name: "Find skills" }),
    );
    await within(dialog).findByText("Skills found");
    await user.click(
      within(dialog).getByRole("button", { name: "Install 1 skill" }),
    );
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "Rolled back. Nothing was changed.",
    );
    expect(dialog).not.toHaveTextContent("Installed 1 skill");
  });

  test("Add to library prefills the directory as a local source", async () => {
    const bridge = skillBridge();
    bridge.on("skill.list", () =>
      skillList({
        inventory: [...skillList().inventory].map((entry) =>
          entry.name === "pdf-forms" && entry.provider === "claude-code"
            ? { ...entry, canonicalPath: "\\\\?\\C:\\skills\\pdf-forms" }
            : entry,
        ),
      }),
    );
    const user = userEvent.setup();
    mount(bridge, "skills");
    await user.click(
      await screen.findByRole("button", { name: "Add pdf-forms to library" }),
    );
    const dialog = await screen.findByRole("dialog", { name: "Add skills" });
    expect(within(dialog).getByLabelText("Source")).toHaveValue(
      "C:\\skills\\pdf-forms",
    );
    expect(
      within(dialog).getByRole("button", { name: "Folder" }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  test("the command palette's Add skills opens the dialog", async () => {
    const bridge = skillBridge();
    const store = new Store(bridge, { statusPollMs: 0, debounceMs: 10 });
    store.openExtensionDialog("add-skills");
    render(
      <StoreProvider value={store}>
        <BridgeProvider value={bridge}>
          <SettingsPage />
        </BridgeProvider>
      </StoreProvider>,
    );
    expect(
      await screen.findByRole("dialog", { name: "Add skills" }),
    ).toBeInTheDocument();
    expect(store.getSnapshot().extensionIntent).toBeNull();
  });
});

describe("Plugins page", () => {
  test("shows each harness's capability, and a Switch only where it can toggle", async () => {
    const bridge = pluginBridge();
    const user = userEvent.setup();
    const { container } = mount(bridge, "plugins");
    const claude = await screen.findByRole("list", {
      name: "Installed plugins",
    });
    expect(screen.getByRole("tab", { name: /Claude Code/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    const capability = screen.getByLabelText("Capabilities");
    expect(capability).toHaveTextContent("Native");
    expect(capability).toHaveTextContent("turn on and off");
    expect(capability).toHaveTextContent("user scope only");
    expect(capability).toHaveTextContent(
      "New plugins load when a session restarts.",
    );
    expect(
      within(claude).getByRole("switch", { name: "Disable superpowers" }),
    ).toBeInTheDocument();
    expect(within(claude).getByText("@acme-market")).toBeInTheDocument();
    expect(within(claude).getByText("1.4.0")).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: /Codex/ }));
    const codex = await screen.findByRole("list", {
      name: "Installed plugins",
    });
    expect(screen.queryByRole("switch")).toBeNull();
    const state = within(codex).getByText("Enabled");
    expect(state).toHaveAttribute(
      "title",
      expect.stringContaining("enable/disable unsupported"),
    );
    expect(screen.getByLabelText("Capabilities")).toHaveTextContent(
      "enable/disable unsupported",
    );

    await user.click(screen.getByRole("tab", { name: /OpenCode/ }));
    expect(screen.getByLabelText("Capabilities")).toHaveTextContent(
      "Translated with documented limitations",
    );
    expect(screen.getByLabelText("Capabilities")).toHaveTextContent(
      "npm modules only",
    );
    // No marketplace management for OpenCode.
    expect(screen.queryByRole("region", { name: "Marketplaces" })).toBeNull();

    await user.click(screen.getByRole("tab", { name: /Antigravity/ }));
    expect(
      await screen.findByText("Antigravity was not detected on this computer."),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "LZ-EXTENSION-NOT-DETECTED",
    );

    const results = await axe.run(container, {
      runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] },
    });
    expect(
      results.violations.filter(
        (v) => v.impact === "critical" || v.impact === "serious",
      ),
    ).toEqual([]);
  });

  test("a harness with profiles offers Personal and each profile", async () => {
    const bridge = pluginBridge();
    const user = userEvent.setup();
    mount(bridge, "plugins");
    await user.click(await screen.findByRole("tab", { name: /Codex/ }));
    const select = await screen.findByLabelText("Profile");
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["Personal", "Work"]);
    await user.selectOptions(select, "Work");
    expect(
      await screen.findByText("No plugins installed in Codex."),
    ).toBeInTheDocument();
  });

  test("turning a plugin off previews, applies in one step, and reports in a toast", async () => {
    const bridge = pluginBridge();
    bridge.on("plugin.preview", (params: unknown) =>
      pluginPreview(params as PluginPreview["request"]),
    );
    bridge.on("plugin.disable", () =>
      pluginOutcome("plugin.disable", [
        {
          target: personal("claude-code"),
          status: "applied",
          error: null,
          observed: null,
          rollbackStatus: "not-needed",
        },
      ]),
    );
    const user = userEvent.setup();
    const { store } = mount(bridge, "plugins");
    await user.click(
      await screen.findByRole("switch", { name: "Disable superpowers" }),
    );
    await waitFor(() =>
      expect(bridge.callsTo("plugin.disable")[0]).toEqual({
        previewId: PREVIEW_ID,
        expectedRevision: 7,
      }),
    );
    expect(bridge.callsTo("plugin.preview")[0]).toEqual({
      action: "disable",
      targets: [{ provider: "claude-code" }],
      name: "superpowers",
    });
    await waitFor(() =>
      expect(store.getSnapshot().toasts[0]).toMatchObject({
        tone: "info",
        title: "Disabled superpowers",
      }),
    );
  });

  test("a refused toggle is reported as an error, not success", async () => {
    const bridge = pluginBridge();
    bridge.on("plugin.preview", (params: unknown) =>
      pluginPreview(params as PluginPreview["request"]),
    );
    bridge.on("plugin.disable", () =>
      pluginOutcome("plugin.disable", [
        {
          target: personal("claude-code"),
          status: "refused",
          error: {
            code: "LZ-EXTENSION-DRIFT",
            message: "native plugin state/version changed after preview",
          },
          observed: null,
          rollbackStatus: "not-needed",
        },
      ]),
    );
    const user = userEvent.setup();
    const { store } = mount(bridge, "plugins");
    await user.click(
      await screen.findByRole("switch", { name: "Disable superpowers" }),
    );
    await waitFor(() =>
      expect(store.getSnapshot().toasts[0]).toMatchObject({
        tone: "error",
        title: "superpowers was not disabled",
        detail: "native plugin state/version changed after preview",
      }),
    );
  });

  test("uninstall shows the preview's exact commands before anything is applied", async () => {
    const bridge = pluginBridge();
    bridge.on("plugin.preview", (params: unknown) => {
      const request = params as PluginPreview["request"];
      return pluginPreview(request, {
        targets: [
          {
            target: request.targets[0]!,
            before: null,
            commands: [
              ["plugin", "uninstall", "superpowers", "--scope", "user"],
            ],
            error: null,
          },
        ],
      });
    });
    bridge.on("plugin.uninstall", () =>
      pluginOutcome("plugin.uninstall", [
        {
          target: personal("claude-code"),
          status: "applied",
          error: null,
          observed: null,
          rollbackStatus: "not-needed",
        },
      ]),
    );
    const user = userEvent.setup();
    mount(bridge, "plugins");
    await user.click(
      await screen.findByRole("button", { name: "More for superpowers" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Uninstall" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Uninstall superpowers",
    });
    const commands = await within(dialog).findByLabelText(
      "Commands for Claude Code · Personal",
    );
    expect(commands).toHaveTextContent(
      "plugin uninstall superpowers --scope user",
    );
    expect(bridge.callsTo("plugin.preview")[0]).toEqual({
      action: "uninstall",
      targets: [{ provider: "claude-code" }],
      name: "superpowers",
    });
    expect(bridge.callsTo("plugin.uninstall")).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Uninstall" }));
    await waitFor(() =>
      expect(bridge.callsTo("plugin.uninstall")[0]).toEqual({
        previewId: PREVIEW_ID,
        expectedRevision: 7,
      }),
    );
    expect(await within(dialog).findByText(/Applied on 1 target/)).toBeTruthy();
  });

  test("a target error in the preview keeps the destructive button off", async () => {
    const bridge = pluginBridge();
    bridge.on("plugin.preview", (params: unknown) =>
      pluginPreview(params as PluginPreview["request"], {
        targets: [
          {
            target: personal("claude-code"),
            before: null,
            commands: [],
            error: {
              code: "LZ-EXTENSION-NATIVE-FAILED",
              message: "native command failed (output withheld)",
            },
          },
        ],
      }),
    );
    const user = userEvent.setup();
    mount(bridge, "plugins");
    await user.click(
      await screen.findByRole("button", { name: "More for superpowers" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Uninstall" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Uninstall superpowers",
    });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "LZ-EXTENSION-NATIVE-FAILED",
    );
    expect(
      within(dialog).getByRole("button", { name: "Uninstall" }),
    ).toBeDisabled();
  });

  test("a marketplace is removed through its own preview, and added from the inline form", async () => {
    const bridge = pluginBridge();
    bridge.on("plugin.preview", (params: unknown) =>
      pluginPreview(params as PluginPreview["request"]),
    );
    const user = userEvent.setup();
    mount(bridge, "plugins");
    const markets = await screen.findByRole("list", { name: "Marketplaces" });
    expect(markets).toHaveTextContent("acme-market");
    expect(markets).toHaveTextContent("acme/market");
    await user.click(
      within(markets).getByRole("button", { name: "More for acme-market" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Remove" }));
    const removal = await screen.findByRole("dialog", {
      name: "Remove marketplace acme-market",
    });
    await within(removal).findByLabelText(
      "Commands for Claude Code · Personal",
    );
    expect(bridge.callsTo("plugin.preview")[0]).toEqual({
      action: "marketplace-remove",
      targets: [{ provider: "claude-code" }],
      name: "acme-market",
    });
    await user.click(within(removal).getByRole("button", { name: "Cancel" }));

    await user.type(screen.getByLabelText("Marketplace name"), "tools");
    await user.type(screen.getByLabelText("Marketplace source"), "acme/tools");
    await user.click(screen.getByRole("button", { name: "Add marketplace" }));
    await screen.findByRole("dialog", { name: "Add marketplace tools" });
    await waitFor(() =>
      expect(bridge.callsTo("plugin.preview")[1]).toEqual({
        action: "marketplace-add",
        targets: [{ provider: "claude-code" }],
        name: "tools",
        marketplace: { name: "tools", source: "acme/tools" },
      }),
    );
  });

  test("the install dialog plans per target, shows errors, and reports every outcome", async () => {
    const bridge = pluginBridge();
    bridge.on("plugin.preview", (params: unknown) => {
      const request = params as PluginPreview["request"];
      return pluginPreview(request, {
        targets: [
          {
            target: request.targets[0]!,
            before: null,
            commands: [
              ["plugin", "marketplace", "add", "acme/market"],
              ["plugin", "install", "notes@acme-market", "--scope", "user"],
            ],
            error: null,
          },
          {
            target: request.targets[1]!,
            before: null,
            commands: [],
            error: {
              code: "LZ-EXTENSION-UNSUPPORTED",
              message: "unsupported: native plugin action is unavailable",
            },
          },
        ],
      });
    });
    bridge.on("plugin.install", () =>
      pluginOutcome("plugin.install", [
        {
          target: personal("claude-code"),
          status: "applied",
          error: null,
          observed: null,
          rollbackStatus: "not-needed",
        },
        {
          target: personal("github-copilot"),
          status: "unsupported-or-unavailable",
          error: {
            code: "LZ-EXTENSION-UNSUPPORTED",
            message: "unsupported: native plugin action is unavailable",
          },
          observed: null,
          rollbackStatus: "not-needed",
          limitations: ["native package downloads may leave cache files"],
        },
      ]),
    );
    const user = userEvent.setup();
    mount(bridge, "plugins");
    await user.click(
      await screen.findByRole("button", { name: "Install plugin" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Install plugin",
    });
    // Claude Code is chosen for the open tab; OpenCode waits for an npm name.
    expect(
      within(dialog).getByRole("checkbox", { name: /Claude Code · Personal/ }),
    ).toBeChecked();
    expect(
      within(dialog).queryByRole("checkbox", { name: /OpenCode/ }),
    ).toBeNull();
    await user.type(
      within(dialog).getByLabelText("Plugin"),
      "notes@acme-market",
    );
    expect(
      within(dialog).queryByRole("checkbox", { name: /OpenCode/ }),
    ).toBeNull();
    await user.type(
      within(dialog).getByLabelText("Marketplace source"),
      "acme/market",
    );
    await user.click(
      within(dialog).getByRole("checkbox", {
        name: /GitHub Copilot · Personal/,
      }),
    );
    await user.click(within(dialog).getByRole("button", { name: "Preview" }));
    await within(dialog).findByLabelText("Planned changes");
    expect(bridge.callsTo("plugin.preview")[0]).toEqual({
      action: "install",
      targets: [{ provider: "claude-code" }, { provider: "github-copilot" }],
      name: "notes@acme-market",
      marketplace: { name: "acme-market", source: "acme/market" },
    });
    expect(
      within(dialog).getByLabelText("Commands for Claude Code · Personal"),
    ).toHaveTextContent(
      "plugin marketplace add acme/market plugin install notes@acme-market --scope user",
    );
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "LZ-EXTENSION-UNSUPPORTED",
    );
    expect(bridge.callsTo("plugin.install")).toHaveLength(0);
    await user.click(
      within(dialog).getByRole("button", { name: "Install on 1 target" }),
    );
    await waitFor(() =>
      expect(bridge.callsTo("plugin.install")[0]).toEqual({
        previewId: PREVIEW_ID,
        expectedRevision: 7,
      }),
    );
    const results = await within(dialog).findByLabelText("Results");
    expect(
      within(dialog).getByText(/Not every target succeeded: 1 of 2 applied\./),
    ).toBeTruthy();
    expect(results).toHaveTextContent("applied");
    expect(results).toHaveTextContent("unsupported or unavailable");
    expect(results).toHaveTextContent("LZ-EXTENSION-UNSUPPORTED");
    expect(results).toHaveTextContent("may leave cache files");
  });

  test("OpenCode is offered once the name looks like an npm module", async () => {
    const bridge = pluginBridge();
    const user = userEvent.setup();
    mount(bridge, "plugins");
    await user.click(
      await screen.findByRole("button", { name: "Install plugin" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Install plugin",
    });
    await user.type(within(dialog).getByLabelText("Plugin"), "oh-my-opencode");
    expect(
      within(dialog).getByRole("checkbox", { name: /OpenCode · Personal/ }),
    ).toBeInTheDocument();
  });

  test("a marketplace source needs a plugin@marketplace name", async () => {
    const bridge = pluginBridge();
    const user = userEvent.setup();
    mount(bridge, "plugins");
    await user.click(
      await screen.findByRole("button", { name: "Install plugin" }),
    );
    const dialog = await screen.findByRole("dialog", {
      name: "Install plugin",
    });
    await user.type(within(dialog).getByLabelText("Plugin"), "notes");
    await user.type(
      within(dialog).getByLabelText("Marketplace source"),
      "acme/market",
    );
    expect(
      within(dialog).getByRole("button", { name: "Preview" }),
    ).toBeDisabled();
    expect(dialog).toHaveTextContent("Name the plugin as plugin@marketplace");
  });

  test("the plugin list is shown as the harnesses report it, with the pending note", async () => {
    const bridge = pluginBridge();
    bridge.on("plugin.list", () =>
      pluginList({
        pending: {
          kind: "plugin",
          backupDirectory: "C:\\Lazoo\\extensions\\operations\\zzz",
        },
      }),
    );
    mount(bridge, "plugins");
    const note = await screen.findByText("Changes are paused.");
    expect(note.closest(".ext-note")).toHaveTextContent(
      "C:\\Lazoo\\extensions\\operations\\zzz",
    );
    expect(
      screen.getByRole("switch", { name: "Disable superpowers" }),
    ).toBeDisabled();
  });
});
