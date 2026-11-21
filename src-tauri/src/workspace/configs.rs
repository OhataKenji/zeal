//! Read config definitions from the workspace only, independently of Bazel.

use super::{BazelConfig, ConfigEntry};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 32;
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_ENTRIES: usize = 1000;

struct Workspace {
    path: PathBuf,
    #[cfg(unix)]
    dir: File,
}

impl Workspace {
    fn open(path: &Path) -> io::Result<Self> {
        let path = fs::canonicalize(path)?;
        #[cfg(unix)]
        let dir = {
            // Anchor at / rather than trusting intermediate components of a
            // canonical path not to turn into symlinks before open().
            let root = File::open("/")?;
            open_components(root, path.strip_prefix("/").expect("canonical path"), true)?
        };
        Ok(Self {
            path,
            #[cfg(unix)]
            dir,
        })
    }

    fn read(&self, relative: &Path) -> Option<String> {
        #[cfg(unix)]
        let file = open_components(self.dir.try_clone().ok()?, relative, false).ok()?;
        #[cfg(not(unix))]
        let file = File::open(self.path.join(relative)).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        file.take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > MAX_FILE_BYTES {
            return None;
        }
        String::from_utf8(bytes).ok()
    }
}

#[cfg(unix)]
fn open_components(mut dir: File, relative: &Path, directory: bool) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, openat};
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        let std::path::Component::Normal(name) = part else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        if directory || parts.peek().is_some() {
            flags |= OFlags::DIRECTORY;
        }
        dir = openat(&dir, name, flags, Mode::empty())?.into();
    }
    Ok(dir)
}

#[derive(Default)]
struct Definitions {
    visited: HashSet<PathBuf>,
    entries: BTreeMap<String, Vec<ConfigEntry>>,
    count: usize,
}

pub(super) fn load_workspace_configs(path: &Path) -> Vec<BazelConfig> {
    let Ok(ws) = Workspace::open(path) else {
        return Vec::new();
    };
    let mut defs = Definitions::default();
    defs.visit(&ws, ".bazelrc", 0);
    defs.entries
        .into_iter()
        .map(|(name, entries)| BazelConfig { name, entries })
        .collect()
}

impl Definitions {
    fn visit(&mut self, ws: &Workspace, import: &str, depth: usize) {
        if depth > MAX_DEPTH || self.visited.len() >= MAX_FILES || self.count >= MAX_ENTRIES {
            return;
        }
        let path = import
            .strip_prefix("%workspace%/")
            .or_else(|| import.strip_prefix("%workspace%\\"))
            .map(|rest| rest.trim_start_matches('/'))
            .unwrap_or(import);
        let Ok(path) = fs::canonicalize(ws.path.join(path)) else {
            return;
        };
        let Ok(relative) = path.strip_prefix(&ws.path) else {
            return;
        };
        if !self.visited.insert(relative.to_owned()) {
            return;
        }
        let Some(text) = ws.read(relative) else {
            return;
        };
        let source = relative.to_string_lossy();
        let mut lines = logical_lines(&text);
        while self.count < MAX_ENTRIES
            && let Some((i, line)) = lines.next()
        {
            self.parse_line(ws, depth, &source, i, &line);
        }
    }

    fn parse_line(&mut self, ws: &Workspace, depth: usize, source: &str, i: usize, line: &str) {
        let Some(words) = tokenize(line) else { return };
        match words.as_slice() {
            [cmd, path] if cmd == "import" || cmd == "try-import" => {
                self.visit(ws, path, depth + 1)
            }
            [head, flags @ ..] => self.add(head, flags, source, i),
            [] => {}
        }
    }

    fn add(&mut self, head: &str, flags: &[String], source: &str, line: usize) {
        let Some((command, name)) = head.split_once(':') else {
            return;
        };
        if command.is_empty()
            || name.is_empty()
            || name.len() > 128
            || name.starts_with('-')
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        {
            return;
        }
        self.entries
            .entry(name.to_owned())
            .or_default()
            .push(ConfigEntry {
                command: command.to_owned(),
                flags: flags.join(" "),
                source: source.to_owned(),
                line,
            });
        self.count += 1;
    }
}

fn logical_lines(text: &str) -> impl Iterator<Item = (usize, String)> {
    let mut lines = text.lines().enumerate();
    std::iter::from_fn(move || {
        let (index, first) = lines.next()?;
        let mut line = first.to_owned();
        while line.ends_with('\\')
            && let Some((_, next)) = lines.next()
        {
            line.pop();
            line.push_str(next);
        }
        Some((index + 1, line))
    })
}

/// Unlike shell comments, an unquoted, unescaped `#` also ends a partial word.
fn tokenize(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == '#' {
            break;
        }
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        words.push(word(&mut chars)?);
    }
    Some(words)
}

fn word(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let mut word = String::new();
    let mut quote = None;
    while let Some(&c) = chars.peek() {
        if quote.is_none() && (c.is_whitespace() || c == '#') {
            break;
        }
        chars.next();
        match c {
            // Accept the documented backslash workspace separator, too.
            '\\' if word == "%workspace%" => word.push('/'),
            '\\' => word.push(chars.next()?),
            '"' | '\'' if quote == Some(c) => quote = None,
            '"' | '\'' if quote.is_none() => quote = Some(c),
            c => word.push(c),
        }
    }
    quote.is_none().then_some(word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::test_support::TempDir;

    #[test]
    fn groups_entries_in_import_order_with_physical_line_numbers() {
        let dir = TempDir::new();
        let ws = dir.path();
        fs::create_dir(ws.join("rc")).unwrap();
        fs::write(
            ws.join(".bazelrc"),
            "\
# comment
build:ci --first \\
    --second # ignored
try-import \"%workspace%/rc/team defaults\"
try-import missing
run:local --tool=debug
",
        )
        .unwrap();
        fs::write(ws.join("rc/team defaults"), "import rc/nested\n").unwrap();
        fs::write(ws.join("rc/nested"), "test:ci --define='x=#42' # ignored\n").unwrap();
        let configs = load_workspace_configs(ws);
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name, "ci");
        assert_eq!(
            configs[0].entries,
            [
                ConfigEntry {
                    command: "build".into(),
                    flags: "--first --second".into(),
                    source: ".bazelrc".into(),
                    line: 2
                },
                ConfigEntry {
                    command: "test".into(),
                    flags: "--define=x=#42".into(),
                    source: "rc/nested".into(),
                    line: 1
                },
            ]
        );
        assert_eq!(configs[1].name, "local");
        assert_eq!(configs[1].entries[0].command, "run");
        assert_eq!(configs[1].entries[0].line, 6);
    }

    #[test]
    fn quoted_and_escaped_hash_imports_each_resolve() {
        let dir = TempDir::new();
        let ws = dir.path();
        fs::create_dir(ws.join("rc")).unwrap();
        fs::write(ws.join("rc/team#1.bazelrc"), "build:team --foo").unwrap();
        for import in [
            r#"import "%workspace%/rc/team#1.bazelrc" # team defaults"#,
            r"import %workspace%/rc/team\#1.bazelrc # team defaults",
            r#"try-import '%workspace%\rc/team#1.bazelrc'"#,
        ] {
            fs::write(ws.join(".bazelrc"), import).unwrap();
            let configs = load_workspace_configs(ws);
            assert_eq!(configs.len(), 1, "{import}");
            assert_eq!(configs[0].entries[0].source, "rc/team#1.bazelrc");
        }
    }

    #[test]
    fn absolute_import_inside_workspace_and_nonprefix_percent_are_literal_paths() {
        let dir = TempDir::new();
        let ws = dir.path();
        let literal = ws.join("team %workspace%");
        fs::create_dir(&literal).unwrap();
        let nested = literal.join("sub.rc");
        fs::write(&nested, "test:local --ok").unwrap();
        fs::write(
            ws.join(".bazelrc"),
            format!("import \"{}\"", nested.display()),
        )
        .unwrap();
        let configs = load_workspace_configs(ws);
        assert_eq!(configs[0].entries[0].source, "team %workspace%/sub.rc");
    }

    #[cfg(unix)]
    #[test]
    fn never_creates_or_changes_permissions_on_a_workspace() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let ws = dir.path().join("shared");
        assert!(load_workspace_configs(&ws).is_empty());
        assert!(!ws.exists());
        fs::create_dir(&ws).unwrap();
        fs::set_permissions(&ws, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(load_workspace_configs(&ws).is_empty());
        assert_eq!(
            fs::metadata(&ws).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn ignores_invalid_or_oversized_config_names() {
        let dir = TempDir::new();
        let longest = "a".repeat(128);
        let good = "AZaz09._+-";
        let lines = format!(
            r#"build:"two words" --foo
build:\"quote --foo
build:-dash --foo
build: --foo
build:é --foo
build:{longest}x --foo
build:{longest} --foo
test:{good} --foo
"#
        );
        fs::write(dir.path().join(".bazelrc"), lines).unwrap();
        let configs = load_workspace_configs(dir.path());
        assert_eq!(
            configs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            [good, &longest]
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_external_root_imports_and_special_files() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new();
        let ws = dir.path().join("ws");
        let sibling = dir.path().join("ws-other");
        fs::create_dir(&ws).unwrap();
        fs::create_dir(&sibling).unwrap();
        let outside = sibling.join("outside.bazelrc");
        fs::write(&outside, "build:outside --bad").unwrap();
        symlink(&outside, ws.join(".bazelrc")).unwrap();
        assert!(load_workspace_configs(&ws).is_empty());
        fs::remove_file(ws.join(".bazelrc")).unwrap();
        symlink(&sibling, ws.join("external")).unwrap();
        symlink("legit", ws.join("internal")).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(ws.join("fifo"))
            .status()
            .unwrap();
        assert!(status.success());
        fs::write(ws.join("legit"), "test:inside --ok").unwrap();
        fs::write(ws.join(".bazelrc"), format!("import external/outside.bazelrc\nimport ../ws-other/outside.bazelrc\nimport /etc/passwd\nimport {}\nimport fifo\nimport internal\nimport legit\n", outside.display())).unwrap();
        let configs = load_workspace_configs(&ws);
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "inside");
        assert_eq!(configs[0].entries.len(), 1);
        assert_eq!(configs[0].entries[0].source, "legit");
    }

    #[cfg(unix)]
    #[test]
    fn fd_walk_refuses_links_swapped_in_after_canonicalization() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new();
        let ws = Workspace::open(dir.path()).unwrap();
        fs::create_dir(ws.path.join("rc")).unwrap();
        fs::write(ws.path.join("rc/config"), "test:ci --ok").unwrap();
        let path = fs::canonicalize(ws.path.join("rc/config")).unwrap();
        let relative = path.strip_prefix(&ws.path).unwrap();
        assert!(ws.read(relative).is_some());
        fs::rename(ws.path.join("rc/config"), ws.path.join("config")).unwrap();
        symlink("../config", ws.path.join("rc/config")).unwrap();
        assert!(ws.read(relative).is_none());
        fs::rename(ws.path.join("rc"), ws.path.join("real")).unwrap();
        fs::write(ws.path.join("real/safe"), "build:ci").unwrap();
        symlink("real", ws.path.join("rc")).unwrap();
        assert!(ws.read(Path::new("rc/safe")).is_none());
    }

    #[test]
    fn cycle_and_depth_bounds() {
        let dir = TempDir::new();
        let ws = dir.path();
        fs::write(ws.join(".bazelrc"), "import 1\nbuild:ci --root").unwrap();
        for n in 1..=10 {
            fs::write(
                ws.join(n.to_string()),
                format!("import .bazelrc\nbuild:ci --depth={n}\nimport {}", n + 1),
            )
            .unwrap();
        }
        let configs = load_workspace_configs(ws);
        let entries = &configs[0].entries;
        assert_eq!(entries.len(), 9);
        assert_eq!(entries[7].flags, "--depth=8");
        assert_eq!(entries[8].flags, "--root");
    }

    #[test]
    fn file_and_total_entry_budgets_apply_across_imports() {
        let dir = TempDir::new();
        let ws = dir.path();
        let mut imports = String::new();
        for n in 1..=35 {
            imports.push_str(&format!("import {n}\n"));
            fs::write(ws.join(n.to_string()), format!("build:ci --n={n}")).unwrap();
        }
        fs::write(ws.join(".bazelrc"), &imports).unwrap();
        let configs = load_workspace_configs(ws);
        assert_eq!(configs[0].entries.len(), 31);
        assert_eq!(configs[0].entries.last().unwrap().flags, "--n=31");
        fs::write(ws.join("1"), "build:other --flag\n".repeat(995)).unwrap();
        let configs = load_workspace_configs(ws);
        assert_eq!(configs[0].entries.len(), 5);
        assert_eq!(configs[1].entries.len(), 995);
    }

    #[test]
    fn root_and_imports_obey_identical_size_bounds() {
        let dir = TempDir::new();
        let ws = dir.path();
        let root = ws.join(".bazelrc");
        let prefix = "build:ok --first\n#";
        let at_limit = format!("{prefix}{}", "x".repeat(MAX_FILE_BYTES - prefix.len()));
        fs::write(&root, &at_limit).unwrap();
        assert_eq!(load_workspace_configs(ws)[0].name, "ok");
        let over_limit = format!("{at_limit}x");
        fs::write(&root, &over_limit).unwrap();
        assert!(load_workspace_configs(ws).is_empty());
        fs::write(ws.join("huge"), over_limit).unwrap();
        fs::write(&root, "import huge\ntest:small --ok").unwrap();
        let configs = load_workspace_configs(ws);
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "small");
    }
}
