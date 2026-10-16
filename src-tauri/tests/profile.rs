//! Golden tests for the profile parser, on the fixtures from
//! `tests/fixtures/run/gen.sh`.

use std::fs::File;
use zeal_lib::profile::{self, Profile};

fn fixture(case: &str) -> Profile {
    let path = format!(
        "{}/tests/fixtures/run/{case}/profile.gz",
        env!("CARGO_MANIFEST_DIR")
    );
    profile::parse(File::open(path).unwrap()).unwrap()
}

fn phase_names(p: &Profile) -> Vec<&str> {
    p.phases.iter().map(|ph| ph.name.as_str()).collect()
}

#[test]
fn ok() {
    let p = fixture("ok");
    assert_eq!(
        phase_names(&p),
        [
            "Launch Blaze",
            "Initialize command",
            "Evaluate target patterns",
            "Load, analyze dependencies and build artifacts",
            "Complete build",
        ]
    );
    for w in p.phases.windows(2) {
        assert_eq!(w[0].end_us, w[1].start_us);
    }
    assert_eq!(p.lanes.len(), 7);
    assert_eq!(p.bars.len(), 18);
    assert_eq!(p.bars.iter().filter(|b| b.mnemonic == "Genrule").count(), 4);
}

#[test]
fn loading_failed() {
    let p = fixture("loading_failed");
    assert_eq!(
        phase_names(&p),
        [
            "Launch Blaze",
            "Initialize command",
            "Evaluate target patterns",
            "Complete build",
        ]
    );
    assert!(p.bars.is_empty());
}

#[test]
fn broken_gz_is_err() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/run/ok/profile.gz"
    ))
    .unwrap();
    let cut = &bytes[..bytes.len() / 2];
    assert!(profile::parse(cut).is_err());
}
