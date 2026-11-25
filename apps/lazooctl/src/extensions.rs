//! Skill/plugin CLI arguments contain only public identities, never credentials or native receipts.
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    List,
    #[command(subcommand)]
    Source(SkillSourceCommand),
    UpdatePreview {
        name: String,
    },
    Install(SkillInstallArgs),
    Update(SkillInstallArgs),
    Expose {
        name: String,
        #[arg(long, value_delimiter = ',', required = true)]
        providers: Vec<String>,
        #[arg(long,value_parser=["link","copy"],default_value="link")]
        mode: String,
        #[arg(long)]
        expected_revision: u64,
    },
    Unexpose {
        name: String,
        #[arg(long, value_delimiter = ',', required = true)]
        providers: Vec<String>,
        #[arg(long)]
        expected_revision: u64,
    },
    Remove {
        name: String,
        #[arg(long)]
        expected_revision: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum SkillSourceCommand {
    Preview {
        source: String,
        #[arg(long,default_value="auto",value_parser=["auto","local","github","skills-sh","git"])]
        source_kind: String,
        #[arg(long)]
        git_ref: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct SkillInstallArgs {
    pub preview_id: Uuid,
    #[arg(long, value_delimiter = ',', required = true)]
    pub names: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub providers: Vec<String>,
    #[arg(long,value_parser=["link","copy"],default_value="link")]
    pub mode: String,
    #[arg(long)]
    pub replace: bool,
    #[arg(long)]
    pub expected_revision: u64,
}

#[derive(Debug, Args)]
pub struct PluginApplyArgs {
    pub preview_id: Uuid,
    #[arg(long)]
    pub expected_revision: u64,
}

#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    List {
        #[arg(long)]
        target: Option<String>,
    },
    Preview {
        #[arg(long,value_parser=["install","uninstall","enable","disable","marketplace-add","marketplace-remove"])]
        action: String,
        /// provider or provider:account-uuid; repeat to install across harnesses.
        #[arg(long = "target", required = true)]
        targets: Vec<String>,
        #[arg(long)]
        name: String,
        #[arg(long, requires = "marketplace_source")]
        marketplace_name: Option<String>,
        #[arg(long, requires = "marketplace_name")]
        marketplace_source: Option<String>,
    },
    Install(PluginApplyArgs),
    Uninstall(PluginApplyArgs),
    Enable(PluginApplyArgs),
    Disable(PluginApplyArgs),
    #[command(subcommand)]
    Marketplace(PluginMarketplaceCommand),
}

#[derive(Debug, Subcommand)]
pub enum PluginMarketplaceCommand {
    List {
        #[arg(long)]
        target: Option<String>,
    },
    Add(PluginApplyArgs),
    Remove(PluginApplyArgs),
}

pub(super) fn skill_request(command: &SkillCommand) -> (&'static str, Value, bool) {
    let (method, params) = match command {
        SkillCommand::List => ("skill.list", json!({})),
        SkillCommand::Source(SkillSourceCommand::Preview {
            source,
            source_kind,
            git_ref,
        }) => (
            "skill.source.preview",
            json!({"source":source,"sourceKind":source_kind,"gitRef":git_ref}),
        ),
        SkillCommand::UpdatePreview { name } => ("skill.update.preview", json!({"name":name})),
        SkillCommand::Install(args) | SkillCommand::Update(args) => (
            if matches!(command, SkillCommand::Install(_)) {
                "skill.install"
            } else {
                "skill.update"
            },
            json!({"previewId":args.preview_id,"names":args.names,"providers":args.providers,"mode":args.mode,"replace":args.replace,"expectedRevision":args.expected_revision}),
        ),
        SkillCommand::Expose {
            name,
            providers,
            mode,
            expected_revision,
        } => (
            "skill.expose",
            json!({"name":name,"providers":providers,"mode":mode,"expectedRevision":expected_revision}),
        ),
        SkillCommand::Unexpose {
            name,
            providers,
            expected_revision,
        } => (
            "skill.unexpose",
            json!({"name":name,"providers":providers,"expectedRevision":expected_revision}),
        ),
        SkillCommand::Remove {
            name,
            expected_revision,
        } => (
            "skill.remove",
            json!({"name":name,"expectedRevision":expected_revision}),
        ),
    };
    (method, params, false)
}

fn target(text: &str) -> Value {
    let (provider, account) = text
        .split_once(':')
        .map_or((text, None), |(p, a)| (p, Some(a)));
    json!({"provider":provider,"accountId":account})
}
fn apply(method: &'static str, args: &PluginApplyArgs) -> (&'static str, Value, bool) {
    (
        method,
        json!({"previewId":args.preview_id,"expectedRevision":args.expected_revision}),
        false,
    )
}

pub(super) fn plugin_request(command: &PluginCommand) -> (&'static str, Value, bool) {
    match command {
        PluginCommand::List { target: t } => (
            "plugin.list",
            json!({"target":t.as_deref().map(target)}),
            false,
        ),
        PluginCommand::Preview {
            action,
            targets,
            name,
            marketplace_name,
            marketplace_source,
        } => (
            "plugin.preview",
            json!({"action":action,"targets":targets.iter().map(|t|target(t)).collect::<Vec<_>>(),"name":name,"marketplace":marketplace_name.as_ref().zip(marketplace_source.as_ref()).map(|(n,s)|json!({"name":n,"source":s}))}),
            false,
        ),
        PluginCommand::Install(a) => apply("plugin.install", a),
        PluginCommand::Uninstall(a) => apply("plugin.uninstall", a),
        PluginCommand::Enable(a) => apply("plugin.enable", a),
        PluginCommand::Disable(a) => apply("plugin.disable", a),
        PluginCommand::Marketplace(PluginMarketplaceCommand::List { target: t }) => (
            "plugin.marketplace.list",
            json!({"target":t.as_deref().map(target)}),
            false,
        ),
        PluginCommand::Marketplace(PluginMarketplaceCommand::Add(a)) => {
            apply("plugin.marketplace.add", a)
        }
        PluginCommand::Marketplace(PluginMarketplaceCommand::Remove(a)) => {
            apply("plugin.marketplace.remove", a)
        }
    }
}
