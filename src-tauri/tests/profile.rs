//! Golden tests for the profile parser, on the fixtures from
//! `tests/fixtures/run/gen.sh`.

use flate2::Compression;
use flate2::write::GzEncoder;
use std::fs::File;
use zeal_lib::profile::{self, Bar, Phase, Profile};

fn fixture_path(case: &str) -> String {
    format!(
        "{}/tests/fixtures/run/{case}/profile.gz",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn fixture(case: &str) -> Profile {
    profile::parse(File::open(fixture_path(case)).unwrap()).unwrap()
}

fn phase(name: &str, start_us: i64, end_us: i64) -> Phase {
    Phase {
        name: name.to_string(),
        start_us,
        end_us,
    }
}

fn bar(lane: usize, start_us: i64, dur_us: i64, name: &str, mnemonic: &str) -> Bar {
    Bar {
        lane,
        start_us,
        dur_us,
        name: name.to_string(),
        mnemonic: mnemonic.to_string(),
    }
}

/// Gzip a `traceEvents` array and parse it, for cases no fixture covers.
fn parse_events(events: serde_json::Value) -> Result<Profile, String> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    serde_json::to_writer(&mut gz, &serde_json::json!({ "traceEvents": events })).unwrap();
    profile::parse(gz.finish().unwrap().as_slice())
}

#[test]
fn ok() {
    let p = fixture("ok");
    assert_eq!(
        p.phases,
        [
            phase("Launch Blaze", -26000, 0),
            phase("Initialize command", 0, 2517800),
            phase("Evaluate target patterns", 2517800, 2877948),
            phase(
                "Load, analyze dependencies and build artifacts",
                2877948,
                5215357
            ),
            phase("Complete build", 5215357, 5220346),
        ]
    );
    assert_eq!(p.lane_count, 7);
    assert_eq!(
        p.bars,
        [
            bar(
                0,
                2942575,
                13522,
                "BazelWorkspaceStatusAction stable-status.txt",
                "BazelWorkspaceStatusAction"
            ),
            bar(
                0,
                3402227,
                1447,
                "Symlinking //pkg:tool",
                "ExecutableSymlink"
            ),
            bar(
                2,
                3402487,
                2805,
                "Creating source manifest for //pkg:tool",
                "SourceSymlinkManifest"
            ),
            bar(
                3,
                3404452,
                8180,
                "Writing repo mapping manifest for //pkg:tool",
                "RepoMappingManifest"
            ),
            bar(
                4,
                3411946,
                704,
                "Writing repo mapping manifest for //pkg:tool [for tool]",
                "RepoMappingManifest"
            ),
            bar(
                5,
                3411983,
                783,
                "Creating source manifest for //pkg:tool [for tool]",
                "SourceSymlinkManifest"
            ),
            bar(
                0,
                3414657,
                1090,
                "Creating runfiles tree \
                 bazel-out/k8-opt-exec-ST-d57f47055a04/bin/pkg/tool.runfiles [for tool]",
                "SymlinkTree"
            ),
            bar(
                2,
                3413228,
                2553,
                "Creating runfiles tree bazel-out/k8-fastbuild/bin/pkg/tool.runfiles",
                "SymlinkTree"
            ),
            bar(
                6,
                3415940,
                425,
                "Symlinking //pkg:tool [for tool]",
                "ExecutableSymlink"
            ),
            bar(6, 3430644, 393, "Symlinking //pkg:t", "ExecutableSymlink"),
            bar(
                2,
                3430614,
                492,
                "Writing repo mapping manifest for //pkg:t",
                "RepoMappingManifest"
            ),
            bar(
                4,
                3430228,
                1420,
                "Creating source manifest for //pkg:t",
                "SourceSymlinkManifest"
            ),
            bar(
                4,
                3432025,
                698,
                "Creating runfiles tree bazel-out/k8-fastbuild/bin/pkg/t.runfiles",
                "SymlinkTree"
            ),
            bar(1, 3113276, 1027315, "Executing genrule //pkg:a", "Genrule"),
            bar(4, 4143586, 6216, "Executing genrule //pkg:c", "Genrule"),
            bar(2, 4141298, 1010681, "Executing genrule //pkg:b", "Genrule"),
            bar(2, 5152527, 7757, "Executing genrule //pkg:d", "Genrule"),
            bar(2, 5163879, 40676, "Testing //pkg:t", "TestRunner"),
        ]
    );
}

#[test]
fn loading_failed() {
    let p = fixture("loading_failed");
    assert_eq!(
        p.phases,
        [
            phase("Launch Blaze", -4000, 0),
            phase("Initialize command", 0, 26972),
            phase("Evaluate target patterns", 26972, 32812),
            phase("Complete build", 32812, 33155),
        ]
    );
    assert_eq!(p.lane_count, 0);
    assert!(p.bars.is_empty());
}

#[test]
fn broken_gz_is_err() {
    let bytes = std::fs::read(fixture_path("ok")).unwrap();
    let cut = &bytes[..bytes.len() / 2];
    assert!(profile::parse(cut).is_err());
}

#[test]
fn no_finishing_is_err() {
    assert!(
        parse_events(serde_json::json!([
            { "cat": "build phase marker", "name": "Launch Blaze", "ph": "X", "ts": -26000 },
        ]))
        .is_err()
    );
}

#[test]
fn actions_off_execution_lanes_are_dropped() {
    let p = parse_events(serde_json::json!([
        { "cat": "build phase marker", "name": "Initialize command", "ph": "X", "ts": 0 },
        { "name": "Finishing", "ph": "X", "ts": 10 },
        { "ph": "M", "name": "thread_name", "tid": 1, "args": { "name": "Main Thread" } },
        { "cat": "action processing", "name": "Executing genrule //pkg:a", "ph": "X",
          "ts": 1, "dur": 2, "tid": 1, "args": { "mnemonic": "Genrule" } },
    ]))
    .unwrap();
    assert_eq!(p.lane_count, 0);
    assert!(p.bars.is_empty());
}
