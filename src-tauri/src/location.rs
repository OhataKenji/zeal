//! Open Bazel source locations without exposing a general URL or file launcher.

use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn open(path: &Path) -> Result<(), String> {
    let path = validated_file(path)?;
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // The canonical absolute path cannot be interpreted as a URL or an option.
    std::process::Command::new(opener)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn validated_file(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || !is_bazel_source(path) {
        return Err("only absolute BUILD, BUILD.bazel, or .bzl file paths can be opened".into());
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("could not resolve source file: {error}"))?;
    let metadata = path
        .metadata()
        .map_err(|error| format!("could not inspect source file: {error}"))?;
    if !is_bazel_source(&path) || !metadata.is_file() {
        return Err(
            "the source location must resolve to a BUILD, BUILD.bazel, or .bzl file".into(),
        );
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 != 0 {
        return Err("executable source files cannot be opened".into());
    }
    // A macro's .bzl file can live in Bazel's external repository cache, outside
    // the workspace. File associations and the local filesystem remain trusted.
    Ok(path)
}

fn is_bazel_source(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "BUILD" || name == "BUILD.bazel")
        || path.extension() == Some(OsStr::new("bzl"))
}

#[cfg(test)]
mod tests {
    use super::validated_file;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("zeal-location-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, "# Bazel source\n").unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn accepts_existing_bazel_sources_including_external_macro_locations() {
        let fixture = Fixture::new();
        let build = fixture.file("BUILD");
        let build_bazel = fixture.file("BUILD.bazel");
        let macro_file = fixture.file("external_macro.bzl");
        assert_eq!(
            validated_file(&build).unwrap(),
            build.canonicalize().unwrap()
        );
        assert_eq!(
            validated_file(&build_bazel).unwrap(),
            build_bazel.canonicalize().unwrap()
        );
        assert_eq!(
            validated_file(&macro_file).unwrap(),
            macro_file.canonicalize().unwrap()
        );
    }

    #[test]
    fn rejects_urls_relative_paths_and_options() {
        assert!(validated_file(Path::new("https://example.com/BUILD")).is_err());
        assert!(validated_file(Path::new("file:///tmp/BUILD")).is_err());
        assert!(validated_file(Path::new("pkg/BUILD")).is_err());
        assert!(validated_file(Path::new("--args")).is_err());
    }

    #[test]
    fn rejects_missing_files_directories_and_other_file_types() {
        let fixture = Fixture::new();
        let directory = fixture.0.join("directory.bzl");
        fs::create_dir(&directory).unwrap();
        let command = fixture.file("launch.command");
        assert!(validated_file(&fixture.0.join("missing.bzl")).is_err());
        assert!(validated_file(&directory).is_err());
        assert!(validated_file(&command).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_executable_sources() {
        let fixture = Fixture::new();
        let executable = fixture.file("BUILD");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validated_file(&executable).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn validates_symlink_destinations() {
        let fixture = Fixture::new();
        let source = fixture.file("source.bzl");
        let safe_link = fixture.0.join("safe.bzl");
        symlink(&source, &safe_link).unwrap();
        assert_eq!(
            validated_file(&safe_link).unwrap(),
            source.canonicalize().unwrap()
        );

        let command = fixture.file("launch.command");
        let unsafe_link = fixture.0.join("BUILD");
        symlink(&command, &unsafe_link).unwrap();
        assert!(validated_file(&unsafe_link).is_err());

        fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validated_file(&safe_link).is_err());
    }
}
