use std::io::Read;

use clap::Subcommand;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{CliError, RpcError};

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    List,
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
        #[arg(value_parser = ["inherited", "balanced", "pinned", "api-pool"])]
        mode: String,
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
            account_id,
            expected_revision,
        } => (
            "provider.accounts.policy",
            json!({"provider":provider,"mode":mode,"accountId":account_id,"revision":expected_revision}),
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
