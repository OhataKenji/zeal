//! The bazel command line Zelith assembles for one invocation: which commands
//! the composer may send, which target strings are safe to pass verbatim, and
//! the observation flags every invocation carries.
//!
//! The request rules are deliberately tiny and pure so they can mirror the
//! UI's `targetError` exactly — the two validations must stay in lockstep.

/// Commands the composer offers. Arguments cannot change the command.
const COMMANDS: [&str; 3] = ["build", "test", "run"];

/// Parse shell-style words, without executing expansions or operators.
/// Reject malformed input before starting Bazel.
pub fn parse_arguments(input: &str) -> Result<Vec<String>, String> {
    if input.contains('\0') {
        return Err("Arguments must not contain NUL characters".to_string());
    }
    shlex::split(input).ok_or_else(|| "Unclosed quote or trailing escape in arguments".to_string())
}

/// Ordinary Bazel defaults, not an invocation policy: caller arguments
/// follow and may override them.
pub fn argv(cmd: &str, arguments: &[String], endpoint: &str, id: &str) -> Vec<String> {
    let mut args = vec![
        "--noblock_for_lock".into(),
        cmd.into(),
        format!("--bes_backend={endpoint}"),
        "--bes_proxy=".into(),
        "--bes_upload_mode=wait_for_upload_complete".into(),
        format!("--invocation_id={id}"),
        "--tool_tag=zelith".into(),
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

/// The target pattern is passed to bazel as one `Command::arg`: nothing is
/// split, quoted or escaped, so whitespace, control characters and a leading
/// `-` are rejected instead of interpreted. Patterns such as `//...` or `:all`
/// are bazel's business. The message names both rules, without echoing the
/// input, which may hold control characters; it must stay in step with the
/// UI's `targetError`. That rule — Unicode White_Space plus Cc — is what
/// `char::is_whitespace` and `char::is_control` test here, and the TS regexp
/// `/[\p{White_Space}\p{Cc}]/u` is its exact counterpart.
pub fn validate_pattern(pattern: &str) -> Result<(), String> {
    let ok = !pattern.is_empty()
        && !pattern.starts_with('-')
        && !pattern.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(())
    } else {
        Err(
            "pattern must be one label or pattern, without whitespace or control \
             characters, and must not start with '-'"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::argv;
    use super::parse_arguments;

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
    fn invocation_defaults_precede_verbatim_arguments() {
        let arguments =
            parse_arguments("--invocation_id=app //:app --config dev -- 'two words'").unwrap();
        let args = argv("run", &arguments, "grpc://127.0.0.1:12345", "id");
        let first_argument = args.len() - arguments.len();
        assert_eq!(
            args[..first_argument],
            [
                "--noblock_for_lock",
                "run",
                "--bes_backend=grpc://127.0.0.1:12345",
                "--bes_proxy=",
                "--bes_upload_mode=wait_for_upload_complete",
                "--invocation_id=id",
                "--tool_tag=zelith",
            ]
        );
        assert_eq!(args[first_argument..], arguments);
    }

    #[test]
    fn profiling_follows_bazel_configuration() {
        let args = argv("build", &[], "grpc://127.0.0.1:12345", "id");
        assert!(!args.iter().any(|arg| arg.contains("profile")));
    }
}
