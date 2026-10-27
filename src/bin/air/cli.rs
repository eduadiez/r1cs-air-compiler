//! The `air` command. It parses arguments, calls the public library API and
//! prints one JSON report; it contains no compilation or checking logic.
use r1cs_air_compiler::{
    self as air, CompositionBinding, LayoutConfig, ModuleCertificate, PackedCertificate,
    ReferenceCertificate,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{self, Write},
    path::Path,
};

/// A failed command: the report's `kind` and `error` fields.
struct Failure {
    kind: Kind,
    message: String,
}

/// Why a command failed; the report's `kind` field.
#[derive(Clone, Copy)]
enum Kind {
    /// The arguments do not match any form of the command.
    Usage,
    /// A file or directory could not be read, written or created.
    Io,
    /// An input is malformed, unsupported or inconsistent, or an artifact
    /// does not encode its source's relation.
    Invalid,
    /// An input exceeds a size limit of the library.
    Limit,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Usage => "usage",
            Kind::Io => "io",
            Kind::Invalid => "invalid",
            Kind::Limit => "limit",
        }
    }
}

impl Failure {
    fn new(kind: Kind, message: String) -> Self {
        Self { kind, message }
    }
}

/// Messages built by the command itself describe invalid input.
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::new(Kind::Invalid, message)
    }
}

type CommandResult<T> = Result<T, Failure>;

/// Each command and its accepted argument forms.
const COMMANDS: &[(&str, &[&str])] = &[
    (
        "compile",
        &[
            "SOURCE OUT [--rows N] [--optimize]",
            "SOURCE OUT --packed",
            "SOURCE OUT --layout CONFIG.json",
        ],
    ),
    ("check", &["SOURCE KERNEL.pilout CERTIFICATE.json"]),
    ("plan", &["SOURCE OUT [--layout CONFIG.json]"]),
    ("witness", &["SOURCE ARTIFACT_DIR INPUT.wtns OUT"]),
    ("module", &["SOURCE OUT --export WIRES [--optimize]"]),
    (
        "check-composition",
        &["SOURCE HOST.pilout CERTIFICATE.json BINDING.json"],
    ),
];

/// Run the command named by the process arguments and exit with status 1 on error.
pub fn run() {
    let mut args = Vec::new();
    for arg in env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(arg) => fail(
                "",
                &Failure::new(Kind::Usage, format!("argument {arg:?} is not valid UTF-8")),
            ),
        }
    }
    if args.as_slice() == ["--version"] {
        print(&format!("air {}", env!("CARGO_PKG_VERSION")));
        return;
    }
    let command = if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        String::new()
    } else {
        args.remove(0)
    };
    if args.iter().any(|arg| arg == "--help" || arg == "-h")
        && (command.is_empty() || COMMANDS.iter().any(|(name, _)| *name == command))
    {
        print(&help(&command));
        return;
    }
    let result = match command.as_str() {
        "compile" => compile(&args),
        "check" => check(&args),
        "plan" => plan(&args),
        "witness" => witness(&args),
        "module" => module(&args),
        "check-composition" => check_composition(&args),
        _ => Err(Failure::new(
            Kind::Usage,
            "expected a command: compile, check, plan, witness, module, check-composition; \
             use --help"
                .into(),
        )),
    };
    match result {
        Ok(result) => print(&json!({"ok": true, "command": command, "result": result}).to_string()),
        Err(failure) => fail(&command, &failure),
    }
}

/// Print `text` on stdout. A closed stdout (for example a pipe into `head`)
/// ends the program quietly instead of panicking.
fn print(text: &str) {
    let mut stdout = io::stdout().lock();
    if writeln!(stdout, "{text}")
        .and_then(|()| stdout.flush())
        .is_err()
    {
        std::process::exit(1);
    }
}

/// Print the failure report on stderr and exit with status 1.
fn fail(command: &str, failure: &Failure) -> ! {
    let report = json!({
        "ok": false,
        "command": command,
        "kind": failure.kind.name(),
        "error": failure.message,
    });
    let _ = writeln!(io::stderr(), "{report}");
    std::process::exit(1);
}

fn help(command: &str) -> String {
    let mut text = String::from("R1CS to PILout compiler and checker\n\n");
    for (name, forms) in COMMANDS {
        if command.is_empty() || command == *name {
            for form in *forms {
                text.push_str(&format!("Usage: air {name} {form}\n"));
            }
            text.push('\n');
        }
    }
    text.push_str(
        "Each OUT directory must not exist yet; it is created. \
         Reports are JSON: on stdout on success, on stderr on failure.",
    );
    text
}

fn usage<T>(command: &str) -> CommandResult<T> {
    Err(Failure::new(
        Kind::Usage,
        format!("invalid arguments for {command}; use --help"),
    ))
}

/// The full message chain of a library error, prefixed with `context`, and
/// its kind.
fn library_error(context: String) -> impl FnOnce(air::Error) -> Failure {
    move |error| {
        let kind = match error.kind() {
            air::ErrorKind::Limit => Kind::Limit,
            _ => Kind::Invalid,
        };
        Failure::new(kind, format!("{context}: {error:#}"))
    }
}

/// An I/O failure of `action` on `path`.
fn io_error<'a>(action: &'static str, path: &'a Path) -> impl FnOnce(io::Error) -> Failure + 'a {
    move |e| Failure::new(Kind::Io, format!("{action} '{}': {e}", path.display()))
}

fn read(path: impl AsRef<Path>) -> CommandResult<Vec<u8>> {
    let path = path.as_ref();
    fs::read(path).map_err(io_error("read", path))
}

fn parse_json<T: DeserializeOwned>(path: impl AsRef<Path>, bytes: &[u8]) -> CommandResult<T> {
    let path = path.as_ref();
    serde_json::from_slice(bytes).map_err(|e| format!("JSON '{}': {e}", path.display()).into())
}

fn read_json<T: DeserializeOwned>(path: impl AsRef<Path>) -> CommandResult<T> {
    let path = path.as_ref();
    parse_json(path, &read(path)?)
}

fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> CommandResult<()> {
    let path = path.as_ref();
    fs::write(path, bytes).map_err(io_error("write", path))
}

/// Write `value` as compact JSON followed by a newline.
fn write_json(path: impl AsRef<Path>, value: &impl Serialize) -> CommandResult<()> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write(path, bytes)
}

/// The `relation` field of a certificate file.
fn relation(path: impl AsRef<Path>, bytes: &[u8]) -> CommandResult<String> {
    let value: Value = parse_json(path, bytes)?;
    Ok(value["relation"].as_str().unwrap_or_default().to_owned())
}

/// Create the output directory; fails if it already exists.
fn fresh(path: &str) -> CommandResult<&Path> {
    let path = Path::new(path);
    fs::create_dir(path).map_err(io_error("create output", path))?;
    Ok(path)
}

fn u32_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn u64_bytes(words: &[u64]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn compile(args: &[String]) -> CommandResult<Value> {
    match args {
        [source, output, flag] if flag == "--packed" => compile_packed(source, output, None),
        [source, output, flag, config] if flag == "--layout" => {
            compile_packed(source, output, Some(config))
        }
        [source, output, rest @ ..] => {
            let (rows, optimize) = match rest {
                [] => (None, false),
                [optimize] if optimize == "--optimize" => (None, true),
                [flag, rows] if flag == "--rows" => (Some(rows), false),
                [flag, rows, optimize] if flag == "--rows" && optimize == "--optimize" => {
                    (Some(rows), true)
                }
                _ => return usage("compile"),
            };
            compile_reference(source, output, rows, optimize)
        }
        _ => usage("compile"),
    }
}

/// Row count of a reference artifact when `--rows` is not given: the smallest
/// allowed, since every row holds a whole instance of the circuit.
const DEFAULT_ROWS: usize = 64;

fn compile_reference(
    source: &str,
    output: &str,
    rows: Option<&String>,
    optimize: bool,
) -> CommandResult<Value> {
    let bytes = read(source)?;
    let rows = match rows {
        Some(rows) => rows.parse().map_err(|e| format!("rows '{rows}': {e}"))?,
        None => DEFAULT_ROWS,
    };
    let (artifact, mode) = if optimize {
        let artifact = air::compile_optimized_reference(&bytes, rows);
        (artifact, "optimized-reference")
    } else {
        (air::compile_reference(&bytes, rows), "reference")
    };
    let artifact = artifact.map_err(library_error(format!("source '{source}'")))?;
    let out = fresh(output)?;
    write(out.join("kernel.pilout"), &artifact.pilout)?;
    write_json(out.join("certificate.json"), &artifact.certificate)?;
    let certificate = &artifact.certificate;
    Ok(json!({
        "mode": mode,
        "relation": certificate.relation(),
        "rows": certificate.rows(),
        "width": certificate.width(),
        "piloutBytes": artifact.pilout.len(),
        "piloutSha256": certificate.pilout_sha256(),
    }))
}

/// The layout configuration in the file `config`, or one that leaves every
/// setting to the compiler; and the context for errors.
fn layout_config(source: &str, config: Option<&String>) -> CommandResult<(LayoutConfig, String)> {
    Ok(match config {
        Some(config) => (
            read_json(config)?,
            format!("source '{source}', layout configuration '{config}'"),
        ),
        None => (LayoutConfig::default(), format!("source '{source}'")),
    })
}

fn compile_packed(source: &str, output: &str, config: Option<&String>) -> CommandResult<Value> {
    let bytes = read(source)?;
    let (settings, context) = layout_config(source, config)?;
    let artifact = air::compile_packed(&bytes, &settings).map_err(library_error(context))?;
    let out = fresh(output)?;
    write(out.join("kernel.pilout"), &artifact.pilout)?;
    write_json(out.join("certificate.json"), &artifact.certificate)?;
    write(out.join("map.bin"), u32_bytes(&artifact.map))?;
    let certificate = &artifact.certificate;
    Ok(json!({
        "mode": "packed",
        "relation": certificate.relation(),
        "layout": artifact.config,
        "rows": certificate.rows(),
        "width": certificate.width(),
        "auxiliaries": certificate.auxiliary_count(),
        "requiresPermutation": certificate.requires_permutation(),
        "piloutBytes": artifact.pilout.len(),
        "piloutSha256": certificate.pilout_sha256(),
    }))
}

fn check(args: &[String]) -> CommandResult<Value> {
    let [source, pilout, certificate] = args else {
        return usage("check");
    };
    let source_bytes = read(source)?;
    let pilout_bytes = read(pilout)?;
    let certificate_bytes = read(certificate)?;
    let context = format!("checking '{source}', '{pilout}' and '{certificate}'");
    let report = match relation(certificate, &certificate_bytes)?.as_str() {
        air::REFERENCE_RELATION => {
            let parsed: ReferenceCertificate = parse_json(certificate, &certificate_bytes)?;
            let report = air::check_reference(&source_bytes, &pilout_bytes, &parsed);
            serde_json::to_value(report.map_err(library_error(context))?)
        }
        air::PACKED_RELATION => {
            let parsed: PackedCertificate = parse_json(certificate, &certificate_bytes)?;
            let report = air::check_packed(&source_bytes, &pilout_bytes, &parsed);
            serde_json::to_value(report.map_err(library_error(context))?)
        }
        air::MODULE_RELATION => {
            let parsed: ModuleCertificate = parse_json(certificate, &certificate_bytes)?;
            let report = air::check_module(&source_bytes, &pilout_bytes, &parsed);
            serde_json::to_value(report.map_err(library_error(context))?)
        }
        _ => return Err(format!("certificate '{certificate}': unknown relation").into()),
    };
    Ok(report.map_err(|e| e.to_string())?)
}

fn plan(args: &[String]) -> CommandResult<Value> {
    let (source, output, config) = match args {
        [source, output] => (source, output, None),
        [source, output, flag, config] if flag == "--layout" => (source, output, Some(config)),
        _ => return usage("plan"),
    };
    let (settings, context) = layout_config(source, config)?;
    let plan = air::plan_layout(&read(source)?, &settings).map_err(library_error(context))?;
    let out = fresh(output)?;
    write_json(out.join("plan.json"), &plan)?;
    write(out.join("map.bin"), u32_bytes(plan.map()))?;
    Ok(json!({
        "layout": plan.config(),
        "rows": plan.rows(),
        "width": plan.width(),
        "auxiliaries": plan.auxiliary_count(),
        "requiresPermutation": plan.requires_permutation(),
        "checkedEquations": plan.checked_equations(),
    }))
}

fn witness(args: &[String]) -> CommandResult<Value> {
    let [source, artifact, input, output] = args else {
        return usage("witness");
    };
    let artifact = Path::new(artifact);
    let certificate_path = artifact.join("certificate.json");
    let certificate_bytes = read(&certificate_path)?;
    if relation(&certificate_path, &certificate_bytes)? != air::PACKED_RELATION {
        return Err(format!(
            "witness requires a packed artifact; '{}' is not one",
            artifact.display()
        )
        .into());
    }
    let certificate: PackedCertificate = parse_json(&certificate_path, &certificate_bytes)?;
    let pilout = read(artifact.join("kernel.pilout"))?;
    let witness = air::project_witness(&read(source)?, &pilout, &certificate, &read(input)?)
        .map_err(library_error(format!(
            "source '{source}', artifact '{}', witness '{input}'",
            artifact.display()
        )))?;
    let out = fresh(output)?;
    write(out.join("trace.bin"), u64_bytes(&witness.trace))?;
    write(out.join("public.bin"), u64_bytes(&witness.publics))?;
    Ok(json!({
        "traceWords": witness.trace.len(),
        "publicWords": witness.publics.len(),
    }))
}

fn module(args: &[String]) -> CommandResult<Value> {
    let (source, output, wires, optimize) = match args {
        [source, output, flag, wires] if flag == "--export" => (source, output, wires, false),
        [source, output, flag, wires, optimize]
            if flag == "--export" && optimize == "--optimize" =>
        {
            (source, output, wires, true)
        }
        _ => return usage("module"),
    };
    let exports = wires
        .split(',')
        .map(|wire| {
            wire.trim()
                .parse::<u32>()
                .map_err(|e| format!("export '{wire}': {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let artifact = air::compile_module(&read(source)?, &exports, optimize)
        .map_err(library_error(format!("source '{source}'")))?;
    let out = fresh(output)?;
    write(out.join("module.pilout"), &artifact.pilout)?;
    write_json(out.join("certificate.json"), &artifact.certificate)?;
    let certificate = &artifact.certificate;
    Ok(json!({
        "relation": certificate.relation(),
        "width": certificate.width(),
        "exports": certificate.exports(),
        "piloutBytes": artifact.pilout.len(),
        "piloutSha256": certificate.pilout_sha256(),
    }))
}

fn check_composition(args: &[String]) -> CommandResult<Value> {
    let [source, host, certificate, binding] = args else {
        return usage("check-composition");
    };
    let parsed: ModuleCertificate = read_json(certificate)?;
    let parsed_binding: CompositionBinding = read_json(binding)?;
    let report = air::check_composition(&read(source)?, &read(host)?, &parsed, &parsed_binding)
        .map_err(library_error(format!(
            "checking '{host}' against '{source}', '{certificate}' and '{binding}'"
        )))?;
    Ok(serde_json::to_value(report).map_err(|e| e.to_string())?)
}
