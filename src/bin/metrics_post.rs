use std::{
    collections::HashMap,
    fmt::Write as _,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde_json::{Map, Value};
use walkdir::WalkDir;

#[derive(Parser)]
#[command(
    name = "metrics-post",
    about = "Helper for post-processing metrics outputs"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Coverage {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Geiger {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = "rusty-mem")]
        crate_name: String,
    },
    Tokei {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Rca {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Debtmap {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Churn {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        json_output: PathBuf,
        #[arg(long)]
        md_output: PathBuf,
        #[arg(long)]
        since: String,
    },
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Coverage { input, output } => process_coverage(&input, &output),
        Command::Geiger {
            input,
            output,
            crate_name,
        } => process_geiger(&input, &output, &crate_name),
        Command::Tokei { input, output } => process_tokei(&input, &output),
        Command::Rca { input, output } => process_rca(&input, &output),
        Command::Debtmap { input, output } => process_debtmap(&input, &output),
        Command::Churn {
            input,
            json_output,
            md_output,
            since,
        } => process_churn(&input, &json_output, &md_output, &since),
    }
}

fn process_coverage(input: &Path, output: &Path) -> Result<()> {
    let content = fs::read_to_string(input)
        .with_context(|| format!("failed to read coverage json at {}", input.display()))?;
    let value: Value = serde_json::from_str(&content)
        .with_context(|| "failed to parse coverage json".to_string())?;

    let totals = value
        .get("total")
        .cloned()
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| data.as_array())
                .and_then(|arr| arr.first())
                .and_then(|first| first.get("totals"))
                .cloned()
        })
        .unwrap_or_else(|| Value::Object(Map::default()));

    let percent = totals
        .get("lines")
        .and_then(|lines| lines.get("percent"))
        .and_then(Value::as_f64);

    let text = percent.map_or_else(
        || "Line coverage: unavailable\n".to_string(),
        |p| format!("Line coverage: {p:.2}%\n"),
    );

    write_string(output, &text)
}

#[derive(Default, Deserialize)]
struct CountEntry {
    #[serde(rename = "unsafe_", default)]
    unsafe_count: Option<u64>,
    #[serde(default)]
    safe: Option<u64>,
}

#[derive(Default, Deserialize)]
struct UnsafetyCounts {
    #[serde(default)]
    functions: CountEntry,
    #[serde(default)]
    methods: CountEntry,
    #[serde(default)]
    item_impls: CountEntry,
    #[serde(default)]
    item_traits: CountEntry,
    #[serde(default)]
    exprs: CountEntry,
}

#[derive(Default, Deserialize)]
struct Unsafety {
    #[serde(default)]
    used: UnsafetyCounts,
    #[serde(default)]
    unused: UnsafetyCounts,
}

#[derive(Default, Deserialize)]
struct PackageId {
    #[serde(default)]
    name: String,
}

#[derive(Default, Deserialize)]
struct PackageInfo {
    #[serde(default)]
    id: PackageId,
}

#[derive(Default, Deserialize)]
struct PackageEntry {
    #[serde(default)]
    package: PackageInfo,
    #[serde(default)]
    unsafety: Unsafety,
}

#[derive(Default, Deserialize)]
struct GeigerReport {
    #[serde(default)]
    packages: Vec<PackageEntry>,
    #[serde(default)]
    used_but_not_scanned_files: Vec<String>,
}

fn process_geiger(input: &Path, output: &Path, crate_name: &str) -> Result<()> {
    let raw = fs::read_to_string(input)
        .with_context(|| format!("failed to read geiger output at {}", input.display()))?;
    let json_start = raw
        .find("{\"packages\"")
        .ok_or_else(|| anyhow!("unable to locate JSON payload inside geiger output"))?;
    let json_slice = &raw[json_start..];
    let report: GeigerReport = serde_json::from_str(json_slice)
        .with_context(|| "failed to parse geiger JSON payload".to_string())?;

    let mut matched = None;
    for pkg in &report.packages {
        if pkg.package.id.name == crate_name {
            matched = Some(pkg);
            break;
        }
    }

    let mut out = String::from("# Unsafe Code Report\n\n");
    if let Some(pkg) = matched {
        let rows = [
            (
                "Functions",
                &pkg.unsafety.used.functions,
                &pkg.unsafety.unused.functions,
            ),
            (
                "Methods",
                &pkg.unsafety.used.methods,
                &pkg.unsafety.unused.methods,
            ),
            (
                "Impls",
                &pkg.unsafety.used.item_impls,
                &pkg.unsafety.unused.item_impls,
            ),
            (
                "Traits",
                &pkg.unsafety.used.item_traits,
                &pkg.unsafety.unused.item_traits,
            ),
            (
                "Expressions",
                &pkg.unsafety.used.exprs,
                &pkg.unsafety.unused.exprs,
            ),
        ];

        let _ = writeln!(out, "Summary for crate `{crate_name}` (includes tests):\n");
        out.push_str("| Item | Used unsafe | Used safe | Unused unsafe |\n");
        out.push_str("| --- | --- | --- | --- |\n");
        let mut all_zero = true;
        for (title, used, unused) in rows {
            let used_unsafe = used.unsafe_count.unwrap_or(0);
            let used_safe = used.safe.unwrap_or(0);
            let unused_unsafe = unused.unsafe_count.unwrap_or(0);
            if used_unsafe != 0 || unused_unsafe != 0 {
                all_zero = false;
            }
            let _ = writeln!(
                out,
                "| {title} | {used_unsafe} | {used_safe} | {unused_unsafe} |"
            );
        }
        out.push('\n');
        if all_zero {
            out.push_str("No unsafe code detected in the crate or its tests.\n");
        } else {
            out.push_str("Unsafe code detected. Review the table above for counts.\n");
        }
    } else {
        out.push_str("No entry for the crate found in geiger output.\n");
    }

    if !report.used_but_not_scanned_files.is_empty() {
        out.push_str(
            "\n`cargo geiger` skipped dependency files (build scripts, generated code, etc.).\n",
        );
        out.push_str("First few entries:\n");
        for entry in report.used_but_not_scanned_files.iter().take(5) {
            let _ = writeln!(out, "- {entry}");
        }
    }

    write_string(output, &out)
}

fn process_tokei(input: &Path, output: &Path) -> Result<()> {
    let content = fs::read_to_string(input)
        .with_context(|| format!("failed to read tokei json at {}", input.display()))?;
    let value: Value =
        serde_json::from_str(&content).with_context(|| "failed to parse tokei json".to_string())?;
    let rust = value
        .get("Rust")
        .ok_or_else(|| anyhow!("tokei report missing 'Rust' entry"))?;
    let code = rust.get("code").and_then(Value::as_u64).unwrap_or(0);
    let comments = rust.get("comments").and_then(Value::as_u64).unwrap_or(0);
    let blanks = rust.get("blanks").and_then(Value::as_u64).unwrap_or(0);
    let text = format!("Rust LOC: {code}\nComments: {comments}\nBlanks: {blanks}\n");
    write_string(output, &text)
}

fn process_rca(input: &Path, output: &Path) -> Result<()> {
    let mut functions = Vec::new();
    for entry in WalkDir::new(input)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_file() && e.path().extension().is_some_and(|ext| ext == "json")
        })
    {
        let file = fs::File::open(entry.path())
            .with_context(|| format!("failed to open {}", entry.path().display()))?;
        let data: Value = serde_json::from_reader(file).with_context(|| {
            format!(
                "failed to parse rust-code-analysis file {}",
                entry.path().display()
            )
        })?;
        let file_name = data
            .get("name")
            .and_then(Value::as_str)
            .map_or_else(|| entry.path().display().to_string(), ToString::to_string);
        gather_functions(&data, &file_name, &mut functions);
    }

    if functions.is_empty() {
        return write_string(
            output,
            "# Rust Code Analysis Summary\n\nNo function metrics captured.\n",
        );
    }

    functions.sort_by(|a, b| b.cyclomatic.total_cmp(&a.cyclomatic));
    let top_cc = functions.iter().take(5).cloned().collect::<Vec<_>>();
    let mut top_mi = functions.clone();
    top_mi.sort_by(|a, b| a.mi.partial_cmp(&b.mi).unwrap_or(std::cmp::Ordering::Equal));
    let lowest_mi = top_mi.into_iter().take(5).collect::<Vec<_>>();
    let function_count =
        u32::try_from(functions.len()).context("number of functions exceeds 32-bit limit")?;
    let avg_cc =
        functions.iter().map(|item| item.cyclomatic).sum::<f64>() / f64::from(function_count);

    let mut out = String::new();
    out.push_str("# Rust Code Analysis Summary\n\n");
    let _ = writeln!(
        out,
        "Average cyclomatic complexity: {avg_cc:.2} (threshold: 5.00)\n"
    );
    out.push_str("## Top Cyclomatic Complexity (highest first)\n\n");
    out.push_str("| Function | File | CC | MI (VS) | SLOC |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for item in &top_cc {
        let _ = writeln!(
            out,
            "| {name} | {file} | {cyclomatic:.0} | {mi:.2} | {sloc:.0} |",
            name = item.name,
            file = item.file,
            cyclomatic = item.cyclomatic,
            mi = item.mi,
            sloc = item.sloc
        );
    }
    out.push_str("\n## Lowest Maintainability Index (Visual Studio variant)\n\n");
    out.push_str("| Function | File | MI (VS) | CC | SLOC |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for item in &lowest_mi {
        let _ = writeln!(
            out,
            "| {name} | {file} | {mi:.2} | {cyclomatic:.0} | {sloc:.0} |",
            name = item.name,
            file = item.file,
            mi = item.mi,
            cyclomatic = item.cyclomatic,
            sloc = item.sloc
        );
    }

    write_string(output, &out)?;

    if avg_cc > 5.0 {
        bail!("Average cyclomatic complexity {avg_cc:.2} exceeds threshold 5.00");
    }

    Ok(())
}

#[derive(Clone)]
struct FunctionMetrics {
    file: String,
    name: String,
    cyclomatic: f64,
    mi: f64,
    sloc: f64,
}

fn gather_functions(node: &Value, file: &str, out: &mut Vec<FunctionMetrics>) {
    if node
        .get("kind")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "function")
        && let (Some(name), Some(cyclomatic), Some(mi), Some(sloc)) = (
            node.get("name").and_then(Value::as_str),
            node.get("metrics")
                .and_then(|m| m.get("cyclomatic"))
                .and_then(|c| c.get("sum"))
                .and_then(Value::as_f64),
            node.get("metrics")
                .and_then(|m| m.get("mi"))
                .and_then(|mi| mi.get("mi_visual_studio"))
                .and_then(Value::as_f64),
            node.get("metrics")
                .and_then(|m| m.get("loc"))
                .and_then(|loc| loc.get("sloc"))
                .and_then(Value::as_f64),
        )
    {
        out.push(FunctionMetrics {
            file: file.to_string(),
            name: name.to_string(),
            cyclomatic,
            mi,
            sloc,
        });
    }

    if let Some(children) = node.get("spaces").and_then(Value::as_array) {
        for child in children {
            gather_functions(child, file, out);
        }
    }
}

fn process_debtmap(input: &Path, output: &Path) -> Result<()> {
    let content = fs::read_to_string(input)
        .with_context(|| format!("failed to read debtmap json at {}", input.display()))?;
    let value: Value = serde_json::from_str(&content)
        .with_context(|| "failed to parse debtmap json".to_string())?;
    let mut items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    items.sort_by(|a, b| {
        let score_a = a
            .get("unified_score")
            .and_then(|s| s.get("final_score"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let score_b = b
            .get("unified_score")
            .and_then(|s| s.get("final_score"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut out = String::from("# Debtmap Summary\n\n## Top Complexity Hotspots\n\n");
    out.push_str("| Function | File | Score | Cyclomatic | Cognitive | Length |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- |\n");

    for item in items.iter().take(5) {
        let loc = item.get("location").and_then(Value::as_object);
        let function = loc
            .and_then(|o| o.get("function"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let file = loc
            .and_then(|o| o.get("file"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let score = item
            .get("unified_score")
            .and_then(|s| s.get("final_score"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let cyclo = item
            .get("cyclomatic_complexity")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let cognitive = item
            .get("cognitive_complexity")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let length = item
            .get("function_length")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let _ = writeln!(
            out,
            "| {function} | {file} | {score:.2} | {cyclo:.0} | {cognitive:.0} | {length:.0} |"
        );
    }

    if items.is_empty() {
        out.push_str("(No hotspots detected)\n");
    }

    write_string(output, &out)
}

fn process_churn(input: &Path, json_output: &Path, md_output: &Path, since: &str) -> Result<()> {
    let content = fs::read_to_string(input)
        .with_context(|| format!("failed to read raw churn data from {}", input.display()))?;
    let mut commits = 0u64;
    let mut additions = 0u64;
    let mut deletions = 0u64;
    let mut file_counter: HashMap<String, u64> = HashMap::new();
    let mut current_commit = None::<String>;

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("commit:") {
            commits += 1;
            current_commit = Some(rest.split(':').next().unwrap_or_default().to_string());
            continue;
        }

        if current_commit.is_some() {
            let mut parts = line.split('\t');
            if let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) {
                if let Ok(value) = add.parse::<u64>() {
                    additions += value;
                    *file_counter.entry(path.to_string()).or_default() += value;
                }
                if let Ok(value) = del.parse::<u64>() {
                    deletions += value;
                    *file_counter.entry(path.to_string()).or_default() += value;
                }
            }
        }
    }

    let mut top_files: Vec<_> = file_counter.into_iter().collect();
    top_files.sort_by(|a, b| b.1.cmp(&a.1));
    top_files.truncate(5);

    let summary = serde_json::json!({
        "since": since,
        "commits": commits,
        "files_changed": top_files.len(),
        "total_additions": additions,
        "total_deletions": deletions,
        "top_files": top_files
            .iter()
            .map(|(path, changes)| serde_json::json!({"path": path, "changes": changes}))
            .collect::<Vec<_>>(),
    });

    write_string(json_output, &serde_json::to_string_pretty(&summary)?)?;

    let mut md = format!(
        "# Git Churn Summary\n\nCommits analyzed: {}\nTotal additions: {}\nTotal deletions: {}\nFiles touched: {}\n\n## Top files by changes\n",
        commits,
        additions,
        deletions,
        summary
            .get("files_changed")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    );

    if top_files.is_empty() {
        md.push_str("- No file changes in window\n");
    } else {
        for (path, changes) in top_files {
            let _ = writeln!(md, "- {path}: {changes} lines changed");
        }
    }

    write_string(md_output, &md)
}

fn write_string(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create parent directories for {}",
                parent.display()
            )
        })?;
    }
    let mut file = fs::File::create(path)
        .with_context(|| format!("failed to create file at {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write data to {}", path.display()))?;
    Ok(())
}
