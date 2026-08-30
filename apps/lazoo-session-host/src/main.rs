use std::{ffi::OsString, process::Stdio};

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, String> {
    let (gate, executable, arguments) = parse_arguments(std::env::args_os())?;
    lazoo_runtime::wait_for_launch_gate(&gate)
        .map_err(|error| format!("session launch gate failed: {error}"))?;
    let status = std::process::Command::new(&executable)
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .and_then(|mut child| child.wait())
        .map_err(|error| format!("provider process failed: {error}"))?;
    Ok(status.code().unwrap_or(1))
}

fn parse_arguments(
    values: impl IntoIterator<Item = OsString>,
) -> Result<(String, OsString, Vec<OsString>), String> {
    let mut values = values.into_iter();
    let _program = values.next();
    if values.next().as_deref() != Some(std::ffi::OsStr::new("--gate")) {
        return Err("usage: lazoo-session-host --gate <name> -- <executable> [args...]".to_owned());
    }
    let gate = values
        .next()
        .ok_or_else(|| "session launch gate name is missing".to_owned())?
        .into_string()
        .map_err(|_| "session launch gate name is not Unicode".to_owned())?;
    if values.next().as_deref() != Some(std::ffi::OsStr::new("--")) {
        return Err("session host executable separator is missing".to_owned());
    }
    let executable = values
        .next()
        .ok_or_else(|| "provider executable is missing".to_owned())?;
    Ok((gate, executable, values.collect()))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::parse_arguments;

    #[test]
    fn argument_parser_preserves_provider_arguments() {
        let (gate, executable, arguments) = parse_arguments([
            OsString::from("host"),
            OsString::from("--gate"),
            OsString::from("Local\\gate"),
            OsString::from("--"),
            OsString::from("provider.exe"),
            OsString::from("--model"),
            OsString::from("value with spaces"),
        ])
        .unwrap();
        assert_eq!(gate, "Local\\gate");
        assert_eq!(executable, "provider.exe");
        assert_eq!(arguments, ["--model", "value with spaces"]);
    }
}
