//! The bazel command line Zeal assembles for one run: which commands the
//! composer may send, which target strings are safe to pass verbatim, and
//! the observation flags every run carries.
//!
//! The request rules are deliberately tiny and pure so they can mirror the
//! UI's `targetError` exactly — the two validations must stay in lockstep.

use std::path::Path;

/// Commands the composer offers. Arguments cannot change the command.
const COMMANDS: [&str; 3] = ["build", "test", "run"];

/// Parse shell-style words, without executing expansions or operators.
/// Reject malformed input before reserving a run or starting Bazel.
pub fn parse_arguments(input: &str) -> Result<Vec<String>, String> {
    if input.contains('\0') {
        return Err("Arguments must not contain NUL characters".to_string());
    }
    shlex::split(input).ok_or_else(|| "Unclosed quote or trailing escape in arguments".to_string())
}

/// Keep app defaults and the selected target before user arguments. A trailing
/// value-taking option cannot consume them, and `run --` reaches the program
/// without any Zeal flags appended to it. User options retain Bazel precedence.
pub fn argv(
    cmd: &str,
    target: &str,
    arguments: &[String],
    dir: &Path,
    endpoint: &str,
    id: &str,
) -> Vec<String> {
    let mut args = vec![
        cmd.to_string(),
        format!("--bes_backend={endpoint}"),
        "--bes_proxy=".to_string(),
        "--bes_upload_mode=wait_for_upload_complete".to_string(),
        format!("--profile={}", dir.join("profile.gz").display()),
        "--experimental_profile_include_primary_output".to_string(),
        "--noslim_profile".to_string(),
        "--tool_tag=zeal".to_string(),
        "--color=yes".to_string(),
        "--curses=no".to_string(),
        format!("--invocation_id={id}"),
        target.to_string(),
    ];
    args.extend_from_slice(arguments);
    args
}

/// Only the composer's own commands are supported.
pub fn validate_cmd(cmd: &str) -> Result<(), String> {
    if COMMANDS.contains(&cmd) {
        Ok(())
    } else {
        Err(format!("unsupported command: {cmd}"))
    }
}

/// The target is passed to bazel as one `Command::arg`: nothing is split,
/// quoted or escaped, so whitespace, control characters and a leading `-` are
/// rejected instead of interpreted. Patterns such as `//...` or `:all` are
/// bazel's business. The message names both rules, without echoing the input,
/// which may hold control characters; it must stay in step with the UI's
/// `targetError`. That rule — Unicode White_Space plus Cc — is what
/// `char::is_whitespace` and `char::is_control` test here, and the TS regexp
/// `/[\p{White_Space}\p{Cc}]/u` is its exact counterpart.
pub fn validate_target(target: &str) -> Result<(), String> {
    let ok = !target.is_empty()
        && !target.starts_with('-')
        && !target.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(())
    } else {
        Err(
            "target must be one label or pattern, without whitespace or control \
             characters, and must not start with '-'"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::argv;
    use super::parse_arguments;
    use std::path::Path;

    #[test]
    fn shell_words_preserve_quotes_empty_values_and_escapes() {
        assert_eq!(
            parse_arguments(
                r#"--config=dev --test_filter='suite one' "two words" '' one\ two "a\"b""#
            )
            .unwrap(),
            [
                "--config=dev",
                "--test_filter=suite one",
                "two words",
                "",
                "one two",
                "a\"b"
            ]
        );
    }

    #[test]
    fn operators_and_expansions_are_literal_words() {
        assert_eq!(
            parse_arguments(r#"-- $HOME $(id) '$(touch marker)' `id` ; | > * '#' # comment"#)
                .unwrap(),
            [
                "--",
                "$HOME",
                "$(id)",
                "$(touch marker)",
                "`id`",
                ";",
                "|",
                ">",
                "*",
                "#"
            ]
        );
    }

    #[test]
    fn empty_arguments_are_allowed() {
        assert!(parse_arguments("").unwrap().is_empty());
        assert!(parse_arguments(" \t\n").unwrap().is_empty());
    }

    #[test]
    fn unfinished_quotes_and_escapes_are_rejected() {
        assert!(parse_arguments("'unfinished").is_err());
        assert!(parse_arguments("\"unfinished").is_err());
        assert!(parse_arguments("trailing\\").is_err());
    }

    #[test]
    fn nul_is_rejected_even_inside_a_comment() {
        assert!(parse_arguments("--name=a\0b").is_err());
        assert!(parse_arguments("# comment\0").is_err());
    }

    #[test]
    fn app_defaults_and_target_precede_user_options_and_program_arguments() {
        let arguments =
            parse_arguments("--config dev -- 'two words' --profile=program-value").unwrap();
        let args = argv(
            "run",
            "//:app",
            &arguments,
            Path::new("/data/run"),
            "grpc://127.0.0.1:12345",
            "id",
        );
        let target = args.iter().position(|arg| arg == "//:app").unwrap();
        assert!(args[..target].contains(&"--invocation_id=id".to_string()));
        assert!(args[..target].contains(&"--profile=/data/run/profile.gz".to_string()));
        assert_eq!(&args[target + 1..], arguments);
    }

    #[test]
    fn trailing_value_taking_option_has_no_app_argument_to_consume() {
        let arguments = parse_arguments("--test_filter").unwrap();
        let args = argv(
            "test",
            "//:test",
            &arguments,
            Path::new("/data/run"),
            "grpc://127.0.0.1:12345",
            "id",
        );
        assert_eq!(&args[args.len() - 2..], ["//:test", "--test_filter"]);
    }
}
