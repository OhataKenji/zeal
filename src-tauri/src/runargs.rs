//! The bazel command line Zeal assembles for one run: which commands the
//! composer may send, which target strings are safe to pass verbatim, and
//! the observation flags every run carries.
//!
//! The request rules are deliberately tiny and pure so they can mirror the
//! UI's `targetError` exactly — the two validations must stay in lockstep.

use std::path::Path;

/// Commands the composer offers. Zeal assembles every other argument, so a
/// free-form command line never reaches bazel.
const COMMANDS: [&str; 3] = ["build", "test", "run"];

/// Arguments after the bazel executable. Bazel writes its artifacts into the
/// run dir.
pub fn argv(cmd: &str, target: &str, dir: &Path, endpoint: &str) -> Vec<String> {
    vec![
        cmd.to_string(),
        target.to_string(),
        format!("--bes_backend={endpoint}"),
        "--bes_proxy=".to_string(),
        "--bes_upload_mode=wait_for_upload_complete".to_string(),
        format!("--profile={}", dir.join("profile.gz").display()),
        "--experimental_profile_include_primary_output".to_string(),
        "--noslim_profile".to_string(),
        "--tool_tag=zeal".to_string(),
        "--color=yes".to_string(),
        "--curses=no".to_string(),
    ]
}

/// Only the composer's own commands: the observation flags belong to Zeal, so
/// anything else would be a command line this core does not model.
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
    use super::*;
    use serde::Deserialize;

    /// The single-target rule is shared with the UI's `targetError`; both
    /// sides judge the same cases from `tests/target-labels.json`, so a
    /// change on either side is a change to one file.
    #[derive(Deserialize)]
    struct Vectors {
        accepted: Vec<Case>,
        rejected: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        target: String,
    }

    const VECTORS: &str = include_str!("../../tests/target-labels.json");

    #[test]
    fn target_validation_matches_the_shared_ui_vectors() {
        let vectors: Vectors = serde_json::from_str(VECTORS).unwrap();
        for case in &vectors.accepted {
            assert!(
                validate_target(&case.target).is_ok(),
                "accepted: {:?}",
                case.target
            );
            // The target reaches bazel verbatim: argv never splits or edits it.
            assert_eq!(
                argv(
                    "build",
                    &case.target,
                    Path::new("/data/runs/abc"),
                    "grpc://127.0.0.1:12345"
                )[1],
                case.target
            );
        }
        for case in &vectors.rejected {
            assert!(
                validate_target(&case.target).is_err(),
                "rejected: {:?}",
                case.target
            );
        }
    }

    #[test]
    fn the_error_names_both_rules_without_echoing_the_input() {
        let msg = validate_target("a b").unwrap_err();
        assert!(msg.contains("whitespace"), "{msg}");
        assert!(msg.contains("'-'"), "{msg}");
    }

    #[test]
    fn command_validation_accepts_build_test_and_run() {
        for cmd in ["build", "test", "run"] {
            assert!(validate_cmd(cmd).is_ok(), "{cmd:?}");
        }
        for cmd in ["clean", "query", ""] {
            assert!(validate_cmd(cmd).is_err(), "{cmd:?}");
        }
    }

    #[test]
    fn argv_has_the_command_one_target_and_the_observation_flags() {
        let dir = Path::new("/data/runs/abc");
        for (cmd, target) in [("build", "//pkg:b"), ("test", "//pkg:t")] {
            let args = argv(cmd, target, dir, "grpc://127.0.0.1:12345");
            assert_eq!(args[0], cmd);
            assert_eq!(args[1], target);
            assert_eq!(
                &args[2..],
                [
                    "--bes_backend=grpc://127.0.0.1:12345",
                    "--bes_proxy=",
                    "--bes_upload_mode=wait_for_upload_complete",
                    "--profile=/data/runs/abc/profile.gz",
                    "--experimental_profile_include_primary_output",
                    "--noslim_profile",
                    "--tool_tag=zeal",
                    "--color=yes",
                    "--curses=no",
                ]
            );
        }
    }
}
