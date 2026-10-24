//! The `air` command. It parses arguments, calls the public library API and
//! prints one JSON report; it contains no compilation or checking logic.
use r1cs_air_compiler::{self as air, LayoutConfig, PackedCertificate, ReferenceCertificate};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{self, Write},
    path::Path,
};

/// A failed command; the message becomes the report's `error` field.
type CommandResult<T> = Result<T, String>;

/// Each command and its accepted argument forms.
const COMMANDS: &[(&str, &[&str])] = &[
    (
        "compile",
        &[
            "SOURCE OUT --rows N [--optimize]",
            "SOURCE OUT --layout CONFIG.json",
        ],
    ),
    ("check", &["SOURCE KERNEL.pilout CERTIFICATE.json"]),
    ("plan", &["SOURCE OUT --layout CONFIG.json"]),
    ("witness", &["SOURCE ARTIFACT_DIR INPUT.wtns OUT"]),
];

/// Run the command named by the process arguments and exit with status 1 on error.
pub fn run() {
    let mut args = Vec::new();
    for arg in env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(arg) => fail("", &format!("argument {arg:?} is not valid UTF-8")),
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
        _ => Err("expected a command: compile, check, plan, witness; use --help".into()),
    };
    match result {
        Ok(result) => print(&json!({"ok": true, "command": command, "result": result}).to_string()),
        Err(error) => fail(&command, &error),
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
fn fail(command: &str, error: &str) -> ! {
    let report = json!({"ok": false, "command": command, "error": error});
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
    Err(format!("invalid arguments for {command}; use --help"))
}

/// The full message chain of a library error, prefixed with `context`.
fn library_error(context: String) -> impl FnOnce(air::Error) -> String {
    move |error| format!("{context}: {error:#}")
}

fn read(path: impl AsRef<Path>) -> CommandResult<Vec<u8>> {
    let path = path.as_ref();
    fs::read(path).map_err(|e| format!("read '{}': {e}", path.display()))
}

fn parse_json<T: DeserializeOwned>(path: impl AsRef<Path>, bytes: &[u8]) -> CommandResult<T> {
    serde_json::from_slice(bytes).map_err(|e| format!("JSON '{}': {e}", path.as_ref().display()))
}

fn read_json<T: DeserializeOwned>(path: impl AsRef<Path>) -> CommandResult<T> {
    let path = path.as_ref();
    parse_json(path, &read(path)?)
}

fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> CommandResult<()> {
    let path = path.as_ref();
    fs::write(path, bytes).map_err(|e| format!("write '{}': {e}", path.display()))
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
    fs::create_dir(path).map_err(|e| format!("create output '{path}': {e}"))?;
    Ok(Path::new(path))
}

fn u32_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn u64_bytes(words: &[u64]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn compile(args: &[String]) -> CommandResult<Value> {
    match args {
        [source, output, flag, config] if flag == "--layout" => {
            compile_packed(source, output, config)
        }
        [source, output, flag, rows] if flag == "--rows" => {
            compile_reference(source, output, rows, false)
        }
        [source, output, flag, rows, optimize] if flag == "--rows" && optimize == "--optimize" => {
            compile_reference(source, output, rows, true)
        }
        _ => usage("compile"),
    }
}

fn compile_reference(
    source: &str,
    output: &str,
    rows: &str,
    optimize: bool,
) -> CommandResult<Value> {
    let bytes = read(source)?;
    let rows = rows.parse().map_err(|e| format!("rows '{rows}': {e}"))?;
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
    Ok(json!({
        "mode": mode,
        "piloutBytes": artifact.pilout.len(),
        "certificate": artifact.certificate,
    }))
}

fn compile_packed(source: &str, output: &str, config: &str) -> CommandResult<Value> {
    let bytes = read(source)?;
    let settings: LayoutConfig = read_json(config)?;
    let artifact = air::compile_packed(&bytes, &settings).map_err(library_error(format!(
        "source '{source}', layout configuration '{config}'"
    )))?;
    let out = fresh(output)?;
    write(out.join("kernel.pilout"), &artifact.pilout)?;
    write_json(out.join("certificate.json"), &artifact.certificate)?;
    write(out.join("map.bin"), u32_bytes(&artifact.map))?;
    Ok(json!({
        "mode": "packed",
        "piloutBytes": artifact.pilout.len(),
        "certificate": artifact.certificate,
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
        _ => return Err(format!("certificate '{certificate}': unknown relation")),
    };
    report.map_err(|e| e.to_string())
}

fn plan(args: &[String]) -> CommandResult<Value> {
    let [source, output, flag, config] = args else {
        return usage("plan");
    };
    if flag != "--layout" {
        return usage("plan");
    }
    let settings: LayoutConfig = read_json(config)?;
    let plan = air::plan_layout(&read(source)?, &settings).map_err(library_error(format!(
        "source '{source}', layout configuration '{config}'"
    )))?;
    let out = fresh(output)?;
    write_json(out.join("plan.json"), &plan)?;
    write(out.join("map.bin"), u32_bytes(plan.map()))?;
    Ok(json!({
        "rows": plan.rows(),
        "width": plan.width(),
        "helpers": plan.helper_count(),
        "requiresCopy": plan.requires_copy(),
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
        ));
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
