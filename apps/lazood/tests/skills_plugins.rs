use lazoo_protocol::JsonRpcRequest;
use lazoo_providers::{DiscoveryEnvironment, ProviderKind};
use lazoo_runtime::DataRoot;
use lazoo_store::Store;
use lazood::App;
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

fn fixture_skill(temp: &TempDir, body: &str) -> std::path::PathBuf {
    let source = temp.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        format!("---\nname: test-skill\ndescription: A fixture skill\n---\n{body}\n"),
    )
    .unwrap();
    source
}

fn test_app(temp: &TempDir) -> (App, std::path::PathBuf) {
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let root = DataRoot::from_path(temp.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    (
        App::new_with_provider_environment(
            store,
            root,
            DiscoveryEnvironment::empty().with_config_home(&home),
        )
        .unwrap(),
        home,
    )
}

fn fake_claude(temp: &TempDir) -> std::path::PathBuf {
    let bin = temp.path().join("fake-native");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("fake-plugin.ps1");
    std::fs::write(&script, r#"
param([Parameter(ValueFromRemainingArguments=$true)][string[]]$NativeArgs)
$statePath = Join-Path $env:HOME '.claude/fake-plugin-state.json'
$dir = Split-Path -Parent $statePath
New-Item -ItemType Directory -Force -Path $dir | Out-Null
if (Test-Path $statePath) { $state = Get-Content -Raw $statePath | ConvertFrom-Json } else { $state = [pscustomobject]@{ plugins=@(); marketplaces=@() } }
if ($NativeArgs.Count -eq 1 -and $NativeArgs[0] -eq '--version') { 'fake-claude 1.0'; exit 0 }
if (($NativeArgs -join ' ') -eq 'plugin list --json') { if (@($state.plugins).Count -eq 0) { '[]' } else { ConvertTo-Json -Compress -InputObject @($state.plugins) }; exit 0 }
if (($NativeArgs -join ' ') -eq 'plugin marketplace list --json') { if (@($state.marketplaces).Count -eq 0) { '[]' } else { ConvertTo-Json -Compress -InputObject @($state.marketplaces) }; exit 0 }
if ($NativeArgs[0..2] -join ' ' -eq 'plugin marketplace add') { $state.marketplaces = @($state.marketplaces) + [pscustomobject]@{name='market';source=$NativeArgs[3]}; $state | ConvertTo-Json -Depth 5 | Set-Content $statePath; exit 0 }
$name = $NativeArgs[2]
if ($NativeArgs[0..1] -join ' ' -eq 'plugin install') { if ($name -eq 'ghost' -or $name -eq 'native-fail') { '{"external":true}' | Set-Content (Join-Path $dir 'settings.json'); $state | ConvertTo-Json -Depth 5 | Set-Content $statePath; if ($name -eq 'native-fail') { exit 9 }; exit 0 }; $package = Join-Path $dir ('plugins/' + $name); New-Item -ItemType Directory -Force -Path $package | Out-Null; '{"name":"fixture"}' | Set-Content (Join-Path $package 'plugin.json'); $state.plugins = @($state.plugins | Where-Object { $_.id -ne $name }) + [pscustomobject]@{id=$name;enabled=$true;installPath=$package}; $state | ConvertTo-Json -Depth 5 | Set-Content $statePath; exit 0 }
if ($NativeArgs[0..1] -join ' ' -eq 'plugin uninstall') { $state.plugins = @($state.plugins | Where-Object { $_.id -ne $name }); $state | ConvertTo-Json -Depth 5 | Set-Content $statePath; exit 0 }
if ($NativeArgs[0..1] -join ' ' -eq 'plugin enable' -or $NativeArgs[0..1] -join ' ' -eq 'plugin disable') { foreach ($p in @($state.plugins)) { if ($p.id -eq $name) { $p.enabled = ($NativeArgs[1] -eq 'enable') } }; $state | ConvertTo-Json -Depth 5 | Set-Content $statePath; exit 0 }
exit 1
"#).unwrap();
    let command = bin.join("claude.cmd");
    std::fs::write(&command, "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%~dp0fake-plugin.ps1\" %*\r\n").unwrap();
    command
}

fn plugin_app(temp: &TempDir) -> (App, std::path::PathBuf) {
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let root = DataRoot::from_path(temp.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    let environment = DiscoveryEnvironment::empty()
        .with_config_home(&home)
        .with_known_location(ProviderKind::ClaudeCode, fake_claude(temp));
    (
        App::new_with_provider_environment(store, root, environment).unwrap(),
        home,
    )
}

async fn response(app: &App, method: &str, params: Value) -> lazoo_protocol::JsonRpcResponse {
    app.handle(JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: method.into(),
        method: method.into(),
        params,
    })
    .await
}

async fn rpc(app: &App, method: &str, params: Value) -> Value {
    let response = response(app, method, params).await;
    assert!(response.error.is_none(), "{method}: {:?}", response.error);
    response.result.unwrap()
}

async fn rpc_error(app: &App, method: &str, params: Value, code: &str) {
    let response = response(app, method, params).await;
    assert_eq!(response.error.unwrap().code, code, "{method}");
}

async fn preview(app: &App, source: &std::path::Path) -> Value {
    rpc(
        app,
        "skill.source.preview",
        json!({"source": source, "sourceKind":"local"}),
    )
    .await
}

#[tokio::test]
async fn skills_preview_install_inventory_and_checked_remove() {
    let temp = tempfile::tempdir().unwrap();
    let source = fixture_skill(&temp, "Hello");
    let (app, home) = test_app(&temp);
    let staged = preview(&app, &source).await;
    assert_eq!(staged["skills"][0]["name"], "test-skill");
    assert!(!home.join(".claude/skills/test-skill").exists());
    let installed = rpc(&app, "skill.install", json!({"previewId":staged["previewId"], "names":["test-skill"], "providers":["claude-code"], "mode":"copy", "replace":false, "expectedRevision":1})).await;
    assert_eq!(installed["revision"], 2);
    let list = rpc(&app, "skill.list", json!({})).await;
    assert_eq!(list["library"][0]["name"], "test-skill");
    assert!(home.join(".claude/skills/test-skill/SKILL.md").is_file());
    let removed = rpc(
        &app,
        "skill.remove",
        json!({"name":"test-skill", "expectedRevision":2}),
    )
    .await;
    assert_eq!(removed["revision"], 3);
    assert!(!home.join(".claude/skills/test-skill").exists());
}

#[tokio::test]
async fn skills_refuse_stale_revisions_and_preserve_unmanaged_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let source = fixture_skill(&temp, "managed");
    let (app, home) = test_app(&temp);
    let unmanaged = home.join(".claude/skills/test-skill");
    std::fs::create_dir_all(&unmanaged).unwrap();
    std::fs::write(unmanaged.join("SKILL.md"), "foreign bytes").unwrap();
    let staged = preview(&app, &source).await;
    rpc_error(&app, "skill.install", json!({"previewId":staged["previewId"], "names":["test-skill"], "providers":["claude-code"], "mode":"copy", "replace":false, "expectedRevision":0}), "LZ-EXTENSION-STALE").await;
    rpc_error(&app, "skill.install", json!({"previewId":staged["previewId"], "names":["test-skill"], "providers":["claude-code"], "mode":"copy", "replace":false, "expectedRevision":1}), "LZ-SKILL-UNMANAGED-CONFLICT").await;
    assert_eq!(
        std::fs::read_to_string(unmanaged.join("SKILL.md")).unwrap(),
        "foreign bytes"
    );
    let shared = home.join(".agents/skills/shared-skill");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(
        shared.join("SKILL.md"),
        "---\nname: shared-skill\ndescription: Shared visibility fixture\n---\nfixture\n",
    )
    .unwrap();
    let inventory = rpc(&app, "skill.list", json!({})).await;
    let codex_shared = inventory["inventory"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["provider"] == "codex" && item["name"] == "shared-skill")
        .unwrap();
    assert!(
        codex_shared["alsoVisibleTo"]
            .as_array()
            .unwrap()
            .iter()
            .any(|target| target["provider"] == "opencode")
    );
    let resources = temp.path().join("shared-resources");
    std::fs::create_dir_all(&resources).unwrap();
    lazoo_providers::extensions::create_junction(&shared.join("resources"), &resources)
        .await
        .unwrap();
    let linked_inventory = rpc(&app, "skill.list", json!({})).await;
    let linked_skill = linked_inventory["inventory"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["provider"] == "codex" && item["name"] == "shared-skill")
        .unwrap();
    assert!(linked_skill["checksum"].is_null());
    assert!(!linked_skill["problems"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn skills_detect_copy_drift_and_link_expose_unexpose_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let source = fixture_skill(&temp, "original");
    let (app, home) = test_app(&temp);
    let staged = preview(&app, &source).await;
    rpc(&app, "skill.install", json!({"previewId":staged["previewId"], "names":["test-skill"], "providers":["claude-code"], "mode":"copy", "replace":false, "expectedRevision":1})).await;
    let copied = home.join(".claude/skills/test-skill/SKILL.md");
    std::fs::write(&copied, "changed outside Lazoo").unwrap();
    rpc_error(
        &app,
        "skill.remove",
        json!({"name":"test-skill", "expectedRevision":2}),
        "LZ-EXTENSION-DRIFT",
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(&copied).unwrap(),
        "changed outside Lazoo"
    );

    let linked = tempfile::tempdir().unwrap();
    let linked_source = fixture_skill(&linked, "linked");
    let (linked_app, linked_home) = test_app(&linked);
    let linked_preview = preview(&linked_app, &linked_source).await;
    rpc(&linked_app, "skill.install", json!({"previewId":linked_preview["previewId"], "names":["test-skill"], "providers":["claude-code"], "mode":"link", "replace":false, "expectedRevision":1})).await;
    let exposure = linked_home.join(".claude/skills/test-skill");
    assert!(exposure.join("SKILL.md").is_file());
    rpc(
        &linked_app,
        "skill.unexpose",
        json!({"name":"test-skill", "providers":["claude-code"], "expectedRevision":2}),
    )
    .await;
    assert!(!exposure.exists());
    rpc(&linked_app, "skill.expose", json!({"name":"test-skill", "providers":["claude-code"], "mode":"link", "expectedRevision":3})).await;
    assert!(exposure.join("SKILL.md").is_file());
    rpc(
        &linked_app,
        "skill.remove",
        json!({"name":"test-skill", "expectedRevision":4}),
    )
    .await;
    assert!(!exposure.exists());
}

#[tokio::test]
async fn skills_update_rechecks_staging_and_refuses_retargeted_links() {
    let temp = tempfile::tempdir().unwrap();
    let source = fixture_skill(&temp, "one");
    let (app, home) = test_app(&temp);
    let initial = preview(&app, &source).await;
    rpc(&app, "skill.install", json!({"previewId":initial["previewId"],"names":["test-skill"],"providers":["claude-code"],"mode":"link","replace":false,"expectedRevision":1})).await;
    let exposure = home.join(".claude/skills/test-skill");
    std::fs::remove_dir_all(&exposure).unwrap();
    std::fs::create_dir_all(&exposure).unwrap();
    std::fs::write(exposure.join("SKILL.md"), "foreign retarget").unwrap();
    rpc_error(
        &app,
        "skill.remove",
        json!({"name":"test-skill","expectedRevision":2}),
        "LZ-EXTENSION-DRIFT",
    )
    .await;

    let update_temp = tempfile::tempdir().unwrap();
    let update_source = fixture_skill(&update_temp, "old");
    let (update_app, update_home) = test_app(&update_temp);
    let first = preview(&update_app, &update_source).await;
    rpc(&update_app, "skill.install", json!({"previewId":first["previewId"],"names":["test-skill"],"providers":["claude-code"],"mode":"copy","replace":false,"expectedRevision":1})).await;
    std::fs::write(
        update_source.join("SKILL.md"),
        "---\nname: test-skill\ndescription: A fixture skill\n---\nnew\n",
    )
    .unwrap();
    let update = rpc(
        &update_app,
        "skill.update.preview",
        json!({"name":"test-skill"}),
    )
    .await;
    let staged_file = update_temp
        .path()
        .join("data/extensions/staging")
        .join(update["previewId"].as_str().unwrap())
        .join("skills/0/SKILL.md");
    std::fs::write(&staged_file, "tampered").unwrap();
    rpc_error(&update_app, "skill.update", json!({"previewId":update["previewId"],"names":["test-skill"],"providers":["claude-code"],"mode":"copy","replace":true,"expectedRevision":2}), "LZ-EXTENSION-DRIFT").await;
    let update = rpc(
        &update_app,
        "skill.update.preview",
        json!({"name":"test-skill"}),
    )
    .await;
    rpc(&update_app, "skill.update", json!({"previewId":update["previewId"],"names":["test-skill"],"providers":["claude-code"],"mode":"copy","replace":true,"expectedRevision":2})).await;
    assert!(
        std::fs::read_to_string(update_home.join(".claude/skills/test-skill/SKILL.md"))
            .unwrap()
            .contains("new")
    );
}

#[tokio::test]
async fn plugin_preview_apply_readback_and_partial_outcomes_use_fake_native_cli() {
    let temp = tempfile::tempdir().unwrap();
    let (app, home) = plugin_app(&temp);
    let request = json!({"action":"install","targets":[{"provider":"claude-code","accountId":null},{"provider":"codex","accountId":null}],"name":"fixture@market","marketplace":{"name":"market","source":"https://example.invalid/market.git"}});
    let preview = rpc(&app, "plugin.preview", request).await;
    assert!(
        preview["targets"][0]["error"].is_null(),
        "fake Claude preview failed: {}",
        preview["targets"][0]
    );
    assert_eq!(preview["targets"][0]["commands"][0][0], "plugin");
    let installed = rpc(
        &app,
        "plugin.install",
        json!({"previewId":preview["previewId"],"expectedRevision":1}),
    )
    .await;
    assert_eq!(installed["outcome"]["targets"][0]["status"], "applied");
    assert_eq!(
        installed["outcome"]["targets"][1]["status"],
        "unsupported-or-unavailable"
    );
    let listed = rpc(
        &app,
        "plugin.list",
        json!({"target":{"provider":"claude-code","accountId":null}}),
    )
    .await;
    assert_eq!(
        listed["targets"][0]["observed"]["plugins"][0]["name"],
        "fixture@market"
    );
    let disable = rpc(&app, "plugin.preview", json!({"action":"disable","targets":[{"provider":"claude-code","accountId":null}],"name":"fixture@market","marketplace":null})).await;
    let disabled = rpc(
        &app,
        "plugin.disable",
        json!({"previewId":disable["previewId"],"expectedRevision":2}),
    )
    .await;
    assert_eq!(disabled["outcome"]["targets"][0]["status"], "applied");
    let uninstall = rpc(&app, "plugin.preview", json!({"action":"uninstall","targets":[{"provider":"claude-code","accountId":null}],"name":"fixture@market","marketplace":null})).await;
    let uninstalled = rpc(
        &app,
        "plugin.uninstall",
        json!({"previewId":uninstall["previewId"],"expectedRevision":3}),
    )
    .await;
    assert_eq!(uninstalled["outcome"]["targets"][0]["status"], "applied");
    assert!(home.join(".claude/fake-plugin-state.json").is_file());
}

#[tokio::test]
async fn plugin_readback_failure_preserves_concurrent_native_config_for_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let (app, home) = plugin_app(&temp);
    let config = home.join(".claude/settings.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "{\"before\":true}").unwrap();
    let preview = rpc(&app, "plugin.preview", json!({"action":"install","targets":[{"provider":"claude-code","accountId":null}],"name":"ghost","marketplace":null})).await;
    let result = rpc(
        &app,
        "plugin.install",
        json!({"previewId":preview["previewId"],"expectedRevision":1}),
    )
    .await;
    assert_eq!(result["outcome"]["targets"][0]["status"], "recovery-needed");
    assert!(
        std::fs::read_to_string(config)
            .unwrap()
            .contains("external")
    );
}

#[tokio::test]
async fn failed_native_plugin_mutation_preserves_concurrent_config_and_blocks_followups() {
    let temp = tempfile::tempdir().unwrap();
    let (app, home) = plugin_app(&temp);
    let config = home.join(".claude/settings.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "{\"before\":true}").unwrap();
    let preview = rpc(&app, "plugin.preview", json!({"action":"install","targets":[{"provider":"claude-code","accountId":null}],"name":"native-fail","marketplace":null})).await;
    let result = rpc(
        &app,
        "plugin.install",
        json!({"previewId":preview["previewId"],"expectedRevision":1}),
    )
    .await;
    let target = &result["outcome"]["targets"][0];
    assert_eq!(target["status"], "recovery-needed");
    assert!(target["observed"].is_object());
    assert!(
        std::fs::read_to_string(config)
            .unwrap()
            .contains("external")
    );
    rpc_error(
        &app,
        "plugin.install",
        json!({"previewId":preview["previewId"],"expectedRevision":2}),
        "LZ-EXTENSION-RECOVERY-REQUIRED",
    )
    .await;
}

#[tokio::test]
async fn skills_expand_native_profiles_and_pending_intents_block_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let source = fixture_skill(&temp, "profile fixture");
    let (app, home) = test_app(&temp);
    let profile = temp.path().join("native-profile");
    let account_id = Uuid::new_v4();
    rpc(&app, "provider.accounts.put", json!({"expectedRevision":0,"spec":{"id":account_id,"provider":"codex","label":"Fixture profile","credentialKind":"native-profile","profileDirectory":profile,"credentialEnvironment":null,"baseUrl":null,"apiProtocol":null,"backendProvider":null,"models":[],"enabled":true}})).await;
    let staged = preview(&app, &source).await;
    rpc(&app, "skill.install", json!({"previewId":staged["previewId"], "names":["test-skill"], "providers":["codex"], "mode":"copy", "replace":false, "expectedRevision":1})).await;
    assert!(home.join(".codex/skills/test-skill/SKILL.md").is_file());
    assert!(profile.join("skills/test-skill/SKILL.md").is_file());

    let blocked = tempfile::tempdir().unwrap();
    let root = DataRoot::from_path(blocked.path().join("data"));
    root.ensure().unwrap();
    let store = Store::open_path(root.state_db()).unwrap();
    store
        .begin_extension_operation(1, &json!({"kind":"fixture-interrupted"}))
        .unwrap();
    let blocked_app = App::new_with_provider_environment(
        store,
        root,
        DiscoveryEnvironment::empty().with_config_home(blocked.path().join("home")),
    )
    .unwrap();
    rpc_error(
        &blocked_app,
        "skill.remove",
        json!({"name":"test-skill", "expectedRevision":1}),
        "LZ-EXTENSION-RECOVERY-REQUIRED",
    )
    .await;

    let invalid = tempfile::tempdir().unwrap();
    let (invalid_app, _) = test_app(&invalid);
    rpc_error(
        &invalid_app,
        "skill.source.preview",
        json!({"source":invalid.path().join("missing"),"sourceKind":"local"}),
        "LZ-EXTENSION-IO",
    )
    .await;
    let staging = invalid.path().join("data/extensions/staging");
    assert!(std::fs::read_dir(staging).unwrap().next().is_none());
}
