//! Private account/profile setup for an operator's suspended native-session takeover.
//!
//! Call this after adding the provider's documented resume arguments and before spawning the
//! terminal. It deliberately does not add Lazoo's controller MCP bridge or permission policy.

use std::ffi::OsString;

use crate::{
    CommandSpec, ProviderError, ProviderKind,
    codex::account_provider_overrides,
    opencode::{LAZOO_PROVIDER_CONFIG_ENV, private_provider_config},
};
use lazoo_runtime::{LaunchEnvironment, TerminalCommand};

const OPENCODE_CONFIG_NAMES: [&str; 3] = [
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_CONFIG_DIR",
];

/// Applies the selected private account/profile environment to a native resume command.
///
/// `command` must already contain the provider's documented resume arguments. For Codex this
/// adds documented process-local `-c` provider overrides when a selected API key exists. For
/// `OpenCode` converts Lazoo's private provider-only marker into an owned inline configuration,
/// and removes inherited configuration carriers so personal config cannot replace it.
pub fn prepare_native_resume_command(
    provider: ProviderKind,
    mut command: CommandSpec,
    environment: &LaunchEnvironment,
) -> Result<CommandSpec, ProviderError> {
    command.apply_environment(environment);
    match provider {
        ProviderKind::Codex => Ok(account_provider_overrides(command, environment)),
        ProviderKind::OpenCode => {
            apply_opencode_environment(&mut command, environment)?;
            Ok(command)
        }
        _ => Ok(command),
    }
}

/// Terminal-command counterpart of [`prepare_native_resume_command`].
///
/// The returned command has the selected profile and credentials. Codex's `-c` provider
/// overrides are appended using the same documented values as the command-spec path.
pub fn prepare_native_resume_terminal(
    provider: ProviderKind,
    command: TerminalCommand,
    environment: &LaunchEnvironment,
) -> Result<TerminalCommand, ProviderError> {
    let mut command = match provider {
        ProviderKind::OpenCode => {
            apply_terminal_environment_without_private_provider_marker(command, environment)
        }
        _ => environment.apply_to_terminal(command),
    };
    match provider {
        ProviderKind::Codex => {
            let spec = account_provider_overrides(CommandSpec::new("ignored", "."), environment);
            command = command.args(spec.args);
            Ok(command)
        }
        ProviderKind::OpenCode => {
            let config = opencode_config(environment)?;
            for name in OPENCODE_CONFIG_NAMES {
                command = command.env_remove(name);
            }
            command = command.env_remove(LAZOO_PROVIDER_CONFIG_ENV);
            if let Some(config) = config {
                command = command.env("OPENCODE_CONFIG_CONTENT", config);
            }
            Ok(command)
        }
        _ => Ok(command),
    }
}

fn apply_opencode_environment(
    command: &mut CommandSpec,
    environment: &LaunchEnvironment,
) -> Result<(), ProviderError> {
    let config = opencode_config(environment)?;
    for name in OPENCODE_CONFIG_NAMES {
        let name = OsString::from(name);
        command.env.remove(&name);
        command.env_remove.push(name);
    }
    let marker = OsString::from(LAZOO_PROVIDER_CONFIG_ENV);
    command.env.remove(&marker);
    command.env_remove.push(marker);
    if let Some(config) = config {
        command.env.insert(
            OsString::from("OPENCODE_CONFIG_CONTENT"),
            OsString::from(config),
        );
    }
    Ok(())
}

fn opencode_config(environment: &LaunchEnvironment) -> Result<Option<String>, ProviderError> {
    private_provider_config(environment).map(|config| config.map(|config| config.to_string()))
}

fn apply_terminal_environment_without_private_provider_marker(
    mut command: TerminalCommand,
    environment: &LaunchEnvironment,
) -> TerminalCommand {
    for name in environment.removals() {
        command = command.env_remove(name);
    }
    for (name, value) in environment.values() {
        if name.as_os_str() != std::ffi::OsStr::new(LAZOO_PROVIDER_CONFIG_ENV)
            && !OPENCODE_CONFIG_NAMES
                .iter()
                .any(|config| name.as_os_str() == std::ffi::OsStr::new(config))
        {
            command = command.env(name, value);
        }
    }
    command.env_remove(LAZOO_PROVIDER_CONFIG_ENV)
}
