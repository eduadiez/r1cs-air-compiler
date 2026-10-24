//! Tests of the `air` executable, run from a temporary directory outside the
//! repository.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

const FIXTURES: &[&str] = &["xor", "range", "mux", "adder"];

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Absolute path of a file in `fixtures/<name>/`.
fn fixture(name: &str, file: &str) -> String {
    let path = repository().join("fixtures").join(name).join(file);
    path.to_str().unwrap().to_owned()
}

fn sha256(path: impl AsRef<Path>) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

/// A fresh working directory, removed when dropped, in which `air` runs.
struct Cli {
    dir: PathBuf,
}
impl Cli {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("r1cs-air-cli-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        Self { dir }
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }
    /// Run `air` and return its exit status and JSON report. A report goes to
    /// stdout on success and to stderr on failure, and the other stream is empty.
    fn run(&self, args: &[&str]) -> (bool, Value) {
        let output = Command::new(env!("CARGO_BIN_EXE_air"))
            .args(args)
            .current_dir(&self.dir)
            .output()
            .unwrap();
        let success = output.status.success();
        let (report, other) = if success {
            (output.stdout, output.stderr)
        } else {
            (output.stderr, output.stdout)
        };
        assert!(other.is_empty(), "unexpected output on the other stream");
        let report: Value = serde_json::from_slice(&report).unwrap();
        assert_eq!(report["ok"], success);
        assert_eq!(report["command"], args.first().copied().unwrap_or(""));
        (success, report)
    }
    /// Run `air`, require success and return the `result` field.
    fn ok(&self, args: &[&str]) -> Value {
        let (success, report) = self.run(args);
        assert!(success, "{args:?} failed: {report}");
        report["result"].clone()
    }
    /// Run `air` and require a failure whose message contains `expected`.
    fn fails(&self, args: &[&str], expected: &str) {
        let (success, report) = self.run(args);
        assert!(!success, "{args:?} succeeded");
        let message = report["error"].as_str().unwrap();
        assert!(message.contains(expected), "{message:?} lacks {expected:?}");
    }
}
impl Drop for Cli {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Compile, check, plan and project one fixture, and compare every output
/// with the hashes in its `expected.json`.
fn check_fixture(name: &str) {
    let cli = Cli::new();
    let source = fixture(name, "kernel.r1cs");
    let layout = fixture(name, "layout.json");
    let expected: Value =
        serde_json::from_slice(&fs::read(fixture(name, "expected.json")).unwrap()).unwrap();
    let assert_hash = |file: &str, key: &str| {
        assert_eq!(
            sha256(cli.path(file)),
            expected[key],
            "{name}: {key} changed"
        );
    };
    for mode in ["reference", "optimized", "packed"] {
        let mut args = vec!["compile", source.as_str(), mode];
        match mode {
            "reference" => args.extend(["--rows", "64"]),
            "optimized" => args.extend(["--rows", "64", "--optimize"]),
            _ => args.extend(["--layout", layout.as_str()]),
        }
        cli.ok(&args);
        assert_hash(&format!("{mode}/kernel.pilout"), mode);
        assert_hash(
            &format!("{mode}/certificate.json"),
            &format!("{mode}Certificate"),
        );
        let pilout = format!("{mode}/kernel.pilout");
        let certificate = format!("{mode}/certificate.json");
        assert_eq!(
            cli.ok(&["check", &source, &pilout, &certificate])["accepted"],
            true
        );
        // An existing output directory is never overwritten.
        cli.fails(&args, &format!("output '{mode}'"));
        assert_hash(&pilout, mode);
    }
    assert_hash("packed/map.bin", "map");
    cli.ok(&["plan", &source, "plan", "--layout", &layout]);
    assert_hash("plan/plan.json", "plan");
    assert_hash("plan/map.bin", "map");
    cli.ok(&[
        "witness",
        &source,
        "packed",
        &fixture(name, "input.wtns"),
        "witness",
    ]);
    assert_hash("witness/trace.bin", "trace");
    assert_hash("witness/public.bin", "public");

    let mut changed = fs::read(cli.path("packed/kernel.pilout")).unwrap();
    changed.push(0);
    fs::write(cli.path("changed.pilout"), changed).unwrap();
    cli.fails(
        &[
            "check",
            &source,
            "changed.pilout",
            "packed/certificate.json",
        ],
        "certificate version, relation or hashes",
    );
    fs::write(cli.path("invalid.wtns"), b"invalid").unwrap();
    cli.fails(
        &["witness", &source, "packed", "invalid.wtns", "rejected"],
        "not a WTNS file",
    );
    assert!(!cli.path("rejected").exists());
}

macro_rules! fixture_tests {
    ($($name:ident),*) => {
        $(
            #[test]
            fn $name() {
                check_fixture(stringify!($name));
            }
        )*
    };
}
fixture_tests!(xor, range, mux, adder);

#[test]
fn every_fixture_directory_is_tested() {
    let mut directories: Vec<_> = fs::read_dir(repository().join("fixtures"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect();
    directories.sort();
    let mut tested = FIXTURES.to_vec();
    tested.sort();
    assert_eq!(directories, tested);
}

/// Every fixture and vendored file is recorded in `docs/provenance.json`.
#[test]
fn provenance_records_every_file_hash() {
    let root = repository();
    let provenance: Value =
        serde_json::from_slice(&fs::read(root.join("docs/provenance.json")).unwrap()).unwrap();
    let list = |directory: &Path, prefix: &str| -> Vec<String> {
        let mut files: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|e| format!("{prefix}/{}", e.unwrap().file_name().to_str().unwrap()))
            .collect();
        files.sort();
        files
    };
    let keys = |section: &Value| -> Vec<String> {
        let mut keys: Vec<_> = section.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    let fixtures = &provenance["fixtures"];
    let mut files = vec![];
    for name in FIXTURES {
        files.extend(list(
            &root.join("fixtures").join(name),
            &format!("fixtures/{name}"),
        ));
    }
    files.sort();
    assert_eq!(keys(fixtures), files);
    let circomlib = &provenance["circomlib"]["files"];
    assert_eq!(
        keys(circomlib),
        list(&root.join("vendor/circomlib"), "vendor/circomlib")
    );
    let schema = &provenance["schema"];
    let recorded = fixtures
        .as_object()
        .unwrap()
        .iter()
        .chain(circomlib.as_object().unwrap())
        .map(|(path, entry)| (path.as_str(), &entry["sha256"]))
        .chain([(schema["path"].as_str().unwrap(), &schema["sha256"])])
        .chain(
            schema["licenses"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(path, entry)| (path.as_str(), &entry["sha256"])),
        );
    for (path, digest) in recorded {
        assert_eq!(&sha256(root.join(path)), digest, "{path}");
    }
}

#[test]
fn help_and_version_are_available() {
    let air = env!("CARGO_BIN_EXE_air");
    let stdout = |args: &[&str]| {
        let output = Command::new(air).args(args).output().unwrap();
        assert!(
            output.status.success() && output.stderr.is_empty(),
            "{args:?}"
        );
        String::from_utf8(output.stdout).unwrap()
    };
    assert_eq!(
        stdout(&["--version"]),
        format!("air {}\n", env!("CARGO_PKG_VERSION"))
    );
    let all = stdout(&["--help"]);
    for command in ["compile", "check", "plan", "witness"] {
        assert!(all.contains(&format!("Usage: air {command}")));
        let help = stdout(&[command, "--help"]);
        assert!(help.contains(&format!("Usage: air {command}")));
    }
}

#[test]
fn reports_have_the_documented_fields() {
    let cli = Cli::new();
    let source = fixture("xor", "kernel.r1cs");
    let layout = fixture("xor", "layout.json");
    let compiled = cli.ok(&["compile", &source, "reference", "--rows", "64"]);
    assert_eq!(compiled["mode"], "reference");
    assert!(compiled["piloutBytes"].as_u64().unwrap() > 0);
    assert_eq!(
        compiled["certificate"]["relation"],
        "r1cs-per-row-publics-at-row-zero"
    );
    let checked = cli.ok(&[
        "check",
        &source,
        "reference/kernel.pilout",
        "reference/certificate.json",
    ]);
    assert_eq!(checked["accepted"], true);
    assert_eq!(checked["relation"], "r1cs-per-row-publics-at-row-zero");
    assert_eq!(checked["formallyVerified"], false);
    assert_eq!(checked["rows"], 64);
    let packed = cli.ok(&["compile", &source, "packed", "--layout", &layout]);
    assert_eq!(packed["mode"], "packed");
    let checked = cli.ok(&[
        "check",
        &source,
        "packed/kernel.pilout",
        "packed/certificate.json",
    ]);
    assert_eq!(checked["relation"], "r1cs-packed-static-copy");
    assert_eq!(checked["requiresCopy"], true);
    let plan = cli.ok(&["plan", &source, "plan", "--layout", &layout]);
    for key in [
        "rows",
        "width",
        "helpers",
        "requiresCopy",
        "checkedEquations",
    ] {
        assert!(!plan[key].is_null(), "plan report lacks {key}");
    }
    let width = plan["width"].as_u64().unwrap();
    let rows = plan["rows"].as_u64().unwrap();
    let witness = cli.ok(&[
        "witness",
        &source,
        "packed",
        &fixture("xor", "input.wtns"),
        "w",
    ]);
    assert_eq!(witness["traceWords"].as_u64(), Some(rows * width));
    assert_eq!(witness["publicWords"], 3);
}

#[test]
fn errors_name_the_failing_input() {
    let cli = Cli::new();
    let source = fixture("xor", "kernel.r1cs");
    let layout = fixture("xor", "layout.json");
    cli.fails(
        &["compile", "missing.r1cs", "out", "--rows", "64"],
        "read 'missing.r1cs'",
    );
    assert!(!cli.path("out").exists());
    fs::write(
        cli.path("bad-layout.json"),
        r#"{"options":{"width":2,"chunk":4,"reuse":"prefix","fold":true},"strategy":"local","minRows":64}"#,
    )
    .unwrap();
    cli.fails(
        &["plan", &source, "out", "--layout", "bad-layout.json"],
        "layout configuration 'bad-layout.json': layout options: options.width must be 3..=1024",
    );
    fs::write(cli.path("malformed.json"), "{\n").unwrap();
    cli.fails(
        &["plan", &source, "out", "--layout", "malformed.json"],
        "JSON 'malformed.json': EOF while parsing an object at line 2 column 0",
    );
    assert!(!cli.path("out").exists());
    cli.ok(&["compile", &source, "packed", "--layout", &layout]);
    fs::write(
        cli.path("unknown.json"),
        r#"{"version":1,"relation":"unknown"}"#,
    )
    .unwrap();
    cli.fails(
        &["check", &source, "packed/kernel.pilout", "unknown.json"],
        "certificate 'unknown.json': unknown relation",
    );
    cli.ok(&["compile", &source, "reference", "--rows", "64"]);
    cli.fails(
        &[
            "witness",
            &source,
            "reference",
            &fixture("xor", "input.wtns"),
            "w",
        ],
        "witness requires a packed artifact",
    );
}

#[test]
fn only_documented_argument_forms_are_accepted() {
    let cli = Cli::new();
    let source = fixture("xor", "kernel.r1cs");
    let layout = fixture("xor", "layout.json");
    for args in [
        &["compile", source.as_str(), "out", "64"][..],
        &["compile", source.as_str(), "out", "--rows"][..],
        &["plan", source.as_str(), layout.as_str(), "out"][..],
        &["check", source.as_str()][..],
        &["witness", source.as_str(), "packed"][..],
    ] {
        cli.fails(args, "invalid arguments");
    }
    assert!(!cli.path("out").exists());
    cli.fails(&[], "expected a command");
    cli.fails(&["frobnicate"], "expected a command");
}

#[cfg(unix)]
#[test]
fn arguments_that_are_not_utf8_get_a_json_error() {
    use std::os::unix::ffi::OsStrExt;
    let output = Command::new(env!("CARGO_BIN_EXE_air"))
        .arg("check")
        .arg(std::ffi::OsStr::from_bytes(b"\xff"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let report: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(report["ok"], false);
    assert!(report["error"]
        .as_str()
        .unwrap()
        .contains("not valid UTF-8"));
}
