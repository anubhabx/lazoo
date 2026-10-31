//! Private per-child credential environment. Never serialized or shown in diagnostics.
use std::{collections::BTreeMap, ffi::OsString, fmt};

use crate::{JobCommand, TerminalCommand};

#[derive(Clone, Default, Eq, PartialEq)]
pub struct LaunchEnvironment {
    values: BTreeMap<OsString, OsString>,
    removals: Vec<OsString>,
}

impl fmt::Debug for LaunchEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LaunchEnvironment")
            .field("keys", &self.values.keys().collect::<Vec<_>>())
            .field("removals", &self.removals)
            .finish()
    }
}

impl LaunchEnvironment {
    pub fn set(&mut self, name: impl Into<OsString>, value: impl Into<OsString>) {
        self.values.insert(name.into(), value.into());
    }

    pub fn remove(&mut self, name: impl Into<OsString>) {
        self.removals.push(name.into());
    }

    #[must_use]
    pub fn values(&self) -> &BTreeMap<OsString, OsString> {
        &self.values
    }

    #[must_use]
    pub fn removals(&self) -> &[OsString] {
        &self.removals
    }

    #[must_use]
    pub fn apply_to_job(&self, mut command: JobCommand) -> JobCommand {
        for name in &self.removals {
            command = command.env_remove(name);
        }
        command.envs(self.values.clone())
    }

    #[must_use]
    pub fn apply_to_terminal(&self, mut command: TerminalCommand) -> TerminalCommand {
        for name in &self.removals {
            command = command.env_remove(name);
        }
        for (name, value) in &self.values {
            command = command.env(name, value);
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_environment_redacts_values_for_both_spawn_paths() {
        let mut environment = LaunchEnvironment::default();
        environment.set("API_KEY", "secret-fixture-value");
        environment.remove("OTHER_KEY");
        assert!(!format!("{environment:?}").contains("secret-fixture-value"));
        let command = environment.apply_to_job(JobCommand::new("provider", "."));
        assert!(!format!("{command:?}").contains("secret-fixture-value"));
        let terminal = environment.apply_to_terminal(TerminalCommand::new("provider", ".", 80, 24));
        assert!(!format!("{terminal:?}").contains("secret-fixture-value"));
    }
}
