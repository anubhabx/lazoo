use std::io::Read;

use clap::Subcommand;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{CliError, RpcError};

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    List,
    /// Query locally observed account usage; omitted account filter includes all accounts.
    Usage {
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, default_value = "day", value_parser = ["hour", "day"])]
        bucket: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, conflicts_with = "personal")]
        account_id: Option<Uuid>,
        #[arg(long)]
        personal: bool,
    },
    /// Set a reserve percentage, or clear it by omitting --reserve-percent.
    Preferences {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=90))]
        reserve_percent: Option<u8>,
    },
    /// Add or update account metadata. API credentials are read from stdin, never argv.
    Put {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        label: String,
        #[arg(long)]
        id: Option<Uuid>,
        #[arg(long, default_value_t = 0)]
        expected_revision: u64,
        /// Empty creates an isolated profile below Lazoo's data root.
        #[arg(long, default_value = "")]
        profile_directory: String,
        /// For API keys: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `COPILOT_GITHUB_TOKEN`, etc.
        #[arg(long)]
        credential_environment: Option<String>,
        #[arg(long, requires = "credential_environment")]
        key_stdin: bool,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long, value_parser = ["responses", "anthropic"])]
        api_protocol: Option<String>,
        #[arg(long)]
        backend_provider: Option<String>,
        #[arg(long = "model")]
        models: Vec<String>,
        #[arg(long)]
        disabled: bool,
    },
    Remove {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Routing changes apply to new sessions; existing sessions retain their account.
    Policy {
        provider: String,
        #[arg(value_parser = ["inherited", "balanced", "priority", "pinned", "api-pool"])]
        mode: String,
        #[arg(long, value_delimiter = ',')]
        order: Vec<Uuid>,
        #[arg(long)]
        account_id: Option<Uuid>,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Open the native provider sign-in in an isolated account profile.
    Login {
        id: Uuid,
        #[arg(long)]
        expected_revision: u64,
    },
}

pub(super) fn command_request(command: &AccountCommand) -> (&'static str, Value, bool) {
    match command {
        AccountCommand::List => ("provider.accounts.list", json!({}), false),
        AccountCommand::Usage {
            from,
            to,
            bucket,
            provider,
            account_id,
            personal,
        } => {
            let mut params = json!({"from":from,"to":to,"bucket":bucket,"provider":provider});
            if *personal || account_id.is_some() {
                params["accountId"] = json!(account_id);
            }
            ("provider.accounts.usage", params, false)
        }
        AccountCommand::Preferences {
            id,
            expected_revision,
            reserve_percent,
        } => (
            "provider.accounts.preferences",
            json!({"id":id,"expectedRevision":expected_revision,"reservePercent":reserve_percent}),
            false,
        ),
        AccountCommand::Put {
            provider,
            label,
            id,
            expected_revision,
            profile_directory,
            credential_environment,
            base_url,
            api_protocol,
            backend_provider,
            models,
            disabled,
            ..
        } => (
            "provider.accounts.put",
            json!({
                "expectedRevision":expected_revision,
                "spec":{"id":id.unwrap_or_else(Uuid::new_v4), "provider":provider, "label":label,
                    "credentialKind":if credential_environment.is_some() {"api-key"} else {"native-profile"},
                    "profileDirectory":profile_directory, "credentialEnvironment":credential_environment,
                    "baseUrl":base_url,"apiProtocol":api_protocol,"backendProvider":backend_provider,
                    "models":models,"enabled":!disabled}
            }),
            false,
        ),
        AccountCommand::Remove {
            id,
            expected_revision,
        } => (
            "provider.accounts.remove",
            json!({"id":id,"expectedRevision":expected_revision}),
            false,
        ),
        AccountCommand::Policy {
            provider,
            mode,
            order,
            account_id,
            expected_revision,
        } => (
            "provider.accounts.policy",
            json!({"provider":provider,"mode":mode,"accountId":account_id,"revision":expected_revision,"order":order}),
            false,
        ),
        AccountCommand::Login {
            id,
            expected_revision,
        } => (
            "provider.accounts.login",
            json!({"id":id,"expectedRevision":expected_revision}),
            false,
        ),
    }
}

pub(super) fn supply_credential(
    command: &AccountCommand,
    params: &mut Value,
) -> Result<(), CliError> {
    if matches!(
        command,
        AccountCommand::Put {
            key_stdin: true,
            ..
        }
    ) {
        let mut input = String::new();
        std::io::stdin()
            .take(16_386)
            .read_to_string(&mut input)
            .map_err(|_| credential_error())?;
        let input = input.trim_end_matches(['\r', '\n']);
        if input.is_empty() || input.len() > 16_384 || input.chars().any(char::is_control) {
            return Err(credential_error());
        }
        params["secret"] = Value::String(input.to_owned());
    }
    Ok(())
}

fn credential_error() -> CliError {
    CliError::Rpc(RpcError {
        code: "LZ-ACCOUNT-INPUT".into(),
        message: "credential stdin must contain one nonempty line of at most 16384 bytes".into(),
        data: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Command};
    use clap::Parser;

    #[test]
    fn usage_priority_and_reserve_commands_preserve_wire_filters_and_revisions() {
        let id = Uuid::new_v4().to_string();
        let request = |args: Vec<&str>| {
            let Command::Account(command) = Cli::try_parse_from(args).unwrap().command else {
                panic!("account command expected")
            };
            let (method, params, _) = command_request(&command);
            (method, params)
        };
        let (method, all) = request(vec!["lazooctl", "account", "usage"]);
        assert_eq!(method, "provider.accounts.usage");
        assert_eq!(all["bucket"], "day");
        assert!(all.get("accountId").is_none());
        let (_, personal) = request(vec!["lazooctl", "account", "usage", "--personal"]);
        assert!(personal.get("accountId").unwrap().is_null());
        let (_, named) = request(vec![
            "lazooctl",
            "account",
            "usage",
            "--account-id",
            &id,
            "--bucket",
            "hour",
        ]);
        assert_eq!(named["accountId"], id);
        let (method, policy) = request(vec![
            "lazooctl",
            "account",
            "policy",
            "codex",
            "priority",
            "--order",
            &id,
            "--expected-revision",
            "3",
        ]);
        assert_eq!(method, "provider.accounts.policy");
        assert_eq!(policy["order"], json!([id]));
        assert_eq!(policy["revision"], 3);
        let (method, reserve) = request(vec![
            "lazooctl",
            "account",
            "preferences",
            &id,
            "--expected-revision",
            "4",
            "--reserve-percent",
            "25",
        ]);
        assert_eq!(method, "provider.accounts.preferences");
        assert_eq!(reserve["reservePercent"], 25);
        assert_eq!(reserve["expectedRevision"], 4);
        let (_, cleared) = request(vec![
            "lazooctl",
            "account",
            "preferences",
            &id,
            "--expected-revision",
            "5",
        ]);
        assert!(cleared["reservePercent"].is_null());
        assert!(
            Cli::try_parse_from([
                "lazooctl",
                "account",
                "preferences",
                &id,
                "--expected-revision",
                "5",
                "--reserve-percent",
                "91"
            ])
            .is_err()
        );
    }

    #[test]
    fn account_cli_keeps_keys_out_of_arguments_and_routes_metadata() {
        let cli = Cli::try_parse_from([
            "lazooctl",
            "account",
            "put",
            "--provider",
            "github-copilot",
            "--label",
            "Work",
            "--credential-environment",
            "COPILOT_GITHUB_TOKEN",
            "--key-stdin",
        ])
        .unwrap();
        let Command::Account(command) = cli.command else {
            panic!("wrong command")
        };
        let (method, params, follows) = command_request(&command);
        assert_eq!(method, "provider.accounts.put");
        assert!(params.get("secret").is_none());
        assert_eq!(params["spec"]["credentialKind"], "api-key");
        assert!(!follows);
        assert!(
            Cli::try_parse_from([
                "lazooctl",
                "account",
                "put",
                "--provider",
                "codex",
                "--label",
                "Work",
                "--api-key",
                "secret"
            ])
            .is_err()
        );
    }
}
