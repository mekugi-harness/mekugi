mod lsp;
mod rust;

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use regex::Regex;
use url::Url;

use crate::ToolOutput;
use crate::common::MAX_SAFE_INT;
use crate::common::clean_path;
use crate::common::outside_workspace;
use crate::common::path_to_slash;
use crate::common::quote_json_string;
use crate::common::relative_to_root;
use crate::sourcekind;
use crate::structure;
use crate::structure::Position;

const SYMBOL_USAGE: &str = "msymbol [--workspace ROOT] (def|refs) PATH [LINE] SYMBOL [N] [(def|refs) PATH [LINE] SYMBOL [N] ...]";
pub(crate) const RETAINED_BYTES: usize = 16 << 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueryMode {
    Def,
    Refs,
}

impl QueryMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Def => "def",
            Self::Refs => "refs",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SymbolRange {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug)]
pub(crate) struct SymbolLocation {
    pub outside: bool,
    pub path: PathBuf,
    pub range: SymbolRange,
}

#[derive(Clone, Debug)]
pub(crate) struct SymbolSource {
    pub path: PathBuf,
    pub parsed: structure::Source,
}

#[derive(Clone, Debug)]
pub(crate) struct SymbolQuery {
    pub mode: QueryMode,
    pub raw_path: String,
    pub name: String,
    pub line: usize,
    pub occurrence: usize,
    pub offset: usize,
    pub file: Option<Arc<SymbolSource>>,
    pub err: Option<String>,
    pub coverage: Option<String>,
    pub locations: Vec<SymbolLocation>,
    pub stderr: String,
}

#[derive(Clone, Debug)]
struct ParsedArgs {
    queries: Vec<SymbolQuery>,
    workspace: Option<String>,
}

#[derive(Clone, Debug)]
struct SourceLoadError {
    reason: String,
    message: String,
}

fn digits_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        #[allow(clippy::expect_used)]
        Regex::new(r"^[0-9]+$").expect("valid digits regex")
    })
}

fn combined_path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        #[allow(clippy::expect_used)]
        Regex::new(r"^(.*):([1-9][0-9]*)$").expect("valid combined path regex")
    })
}

fn positive_symbol_int(raw: &str, label: &str) -> Result<usize, String> {
    let val = raw
        .parse::<u64>()
        .map_err(|_| format!("{label} is too large"))?;
    if val > MAX_SAFE_INT {
        return Err(format!("{label} is too large"));
    }
    if val < 1 || raw != val.to_string() {
        return Err(format!("{label} must be a positive decimal integer"));
    }
    usize::try_from(val).map_err(|_| format!("{label} is too large"))
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut operands: Vec<&str> = Vec::new();
    let mut workspace: Option<String> = None;
    let mut ended = false;
    let mut idx = 0usize;

    while idx < args.len() {
        let arg = args[idx].as_str();
        idx += 1;
        if !ended && arg == "--" {
            ended = true;
            continue;
        }
        if !ended && arg == "--workspace" {
            if workspace.is_some()
                || idx >= args.len()
                || args[idx].is_empty()
                || args[idx].contains('\0')
            {
                return Err("--workspace requires one usable directory".to_string());
            }
            workspace = Some(args[idx].clone());
            idx += 1;
            continue;
        }
        if !ended && (arg == "--max-tokens" || arg.starts_with("--max-tokens=")) {
            return Err("unknown option \"--max-tokens\"".to_string());
        }
        operands.push(arg);
    }

    let mut queries = Vec::new();
    let mut i = 0usize;
    while i < operands.len() {
        let mode = match operands[i] {
            "def" => QueryMode::Def,
            "refs" => QueryMode::Refs,
            _ => return Err("mode must be def or refs".to_string()),
        };
        i += 1;
        if i >= operands.len() || operands[i].is_empty() || operands[i].contains('\0') {
            return Err("path must be usable".to_string());
        }
        let mut raw_path = operands[i].to_string();
        i += 1;

        let mut line = 0usize;
        if let Some(caps) = combined_path_regex().captures(&raw_path) {
            let mut extracted = caps.get(1).map_or("", |m| m.as_str()).to_string();
            let line_str = caps.get(2).map_or("", |m| m.as_str());
            if extracted.starts_with('"') {
                extracted = serde_json::from_str::<String>(&extracted)
                    .map_err(|_| "invalid quoted PATH:LINE".to_string())?;
            }
            line = positive_symbol_int(line_str, "line")?;
            raw_path = extracted;
        } else if i < operands.len() && digits_regex().is_match(operands[i]) {
            line = positive_symbol_int(operands[i], "line")?;
            i += 1;
        }

        if i >= operands.len() {
            return Err(format!("usage: {SYMBOL_USAGE}"));
        }
        let mut name = operands[i].to_string();
        i += 1;
        if let Some((_, last)) = name.rsplit_once('.') {
            name = last.to_string();
        }
        if name.is_empty() {
            return Err("SYMBOL must end with a usable name".to_string());
        }

        let mut occurrence = 0usize;
        if i < operands.len() && digits_regex().is_match(operands[i]) {
            occurrence = positive_symbol_int(operands[i], "N")?;
            i += 1;
            if line == 0 {
                return Err("N requires an explicit LINE".to_string());
            }
        }

        queries.push(SymbolQuery {
            mode,
            raw_path,
            name,
            line,
            occurrence,
            offset: 0,
            file: None,
            err: None,
            coverage: None,
            locations: Vec::new(),
            stderr: String::new(),
        });
    }

    if queries.is_empty() {
        return Err(format!("usage: {SYMBOL_USAGE}"));
    }

    Ok(ParsedArgs { queries, workspace })
}

fn load_symbol_source(
    root: &Path,
    raw_path: &str,
    expected_resolver: Option<&str>,
    cache: &mut HashMap<PathBuf, Arc<SymbolSource>>,
) -> Result<Arc<SymbolSource>, SourceLoadError> {
    let candidate = if Path::new(raw_path).is_absolute() {
        PathBuf::from(clean_path(raw_path))
    } else {
        let joined = root.join(raw_path);
        PathBuf::from(clean_path(&joined.to_string_lossy()))
    };
    if outside_workspace(root, &candidate) {
        return Err(SourceLoadError {
            reason: "outside workspace".to_string(),
            message: "path is outside the workspace".to_string(),
        });
    }
    let canonical = fs::canonicalize(&candidate).map_err(|err| SourceLoadError {
        reason: "unavailable".to_string(),
        message: format!("cannot read path: {err}"),
    })?;
    if outside_workspace(root, &canonical) {
        return Err(SourceLoadError {
            reason: "outside workspace".to_string(),
            message: "path resolves outside the workspace".to_string(),
        });
    }

    let canonical_str = canonical.to_string_lossy();
    let format = sourcekind::classify(&canonical_str);
    let valid_resolver = format
        .and_then(|f| f.semantic_resolver)
        .is_some_and(|r| expected_resolver.is_none_or(|exp| exp == r));
    let Some(format) = format.filter(|_| valid_resolver) else {
        let reason = match expected_resolver {
            Some("gopls") => "not Go",
            Some("python") => "not Python",
            Some("rust") => "not Rust",
            _ => "not TypeScript",
        };
        return Err(SourceLoadError {
            reason: reason.to_string(),
            message: "path has an unsupported msymbol source format".to_string(),
        });
    };

    if let Some(cached) = cache.get(&canonical) {
        return Ok(Arc::clone(cached));
    }

    let meta = fs::metadata(&canonical).map_err(|err| SourceLoadError {
        reason: "unavailable".to_string(),
        message: err.to_string(),
    })?;
    if !meta.is_file() {
        return Err(SourceLoadError {
            reason: "not regular".to_string(),
            message: "path is not a regular file".to_string(),
        });
    }

    let bytes = fs::read(&canonical).map_err(|err| SourceLoadError {
        reason: "unavailable".to_string(),
        message: err.to_string(),
    })?;
    let text = String::from_utf8(bytes).map_err(|_| SourceLoadError {
        reason: "not UTF-8".to_string(),
        message: "path is not UTF-8".to_string(),
    })?;
    let parsed = structure::parse(text, format).map_err(|err| SourceLoadError {
        reason: "unavailable".to_string(),
        message: err,
    })?;

    let source = Arc::new(SymbolSource {
        path: canonical.clone(),
        parsed,
    });
    cache.insert(canonical, Arc::clone(&source));
    Ok(source)
}

fn nearby_symbol_lines(source: &structure::Source, name: &str, target_line: usize) -> String {
    let mut lines: Vec<usize> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for tok in &source.tokens {
        if tok.text == name {
            let n = source.lines.at(tok.from);
            if seen.insert(n) {
                lines.push(n);
            }
        }
    }
    if target_line > 0 {
        lines.sort_by_key(|&n| n.abs_diff(target_line));
    }
    lines.truncate(5);
    lines.sort_unstable();
    lines
        .into_iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn select_symbol(q: &mut SymbolQuery) -> Result<(), String> {
    let Some(file) = q.file.as_ref() else {
        return Err("missing source".to_string());
    };
    let s = &file.parsed;

    if q.line == 0 {
        let entries: Vec<&structure::Entry> = s
            .entries
            .iter()
            .filter(|e| e.kind != "parse_error" && e.name == q.name && e.name_from.is_some())
            .collect();
        if entries.len() > 1 {
            let lines: Vec<String> = entries
                .iter()
                .filter_map(|e| e.name_from.map(|off| s.lines.at(off).to_string()))
                .collect();
            return Err(format!(
                "{} has {} outline matches (lines {}); supply LINE",
                q.name,
                entries.len(),
                lines.join(", ")
            ));
        }
        if entries.is_empty() {
            let nearby = nearby_symbol_lines(s, &q.name, 0);
            let hint = if nearby.is_empty() {
                "no matching token in this file".to_string()
            } else {
                format!("supply LINE, such as a token line: {nearby}")
            };
            return Err(format!("{} has no outline declaration; {hint}", q.name));
        }
        let entry = entries[0];
        if !entry.complete {
            return Err(format!(
                "{} has an incomplete outline declaration; supply LINE",
                q.name
            ));
        }
        if let Some(name_from) = entry.name_from {
            q.offset = name_from;
            q.line = s.lines.at(name_from);
        }
        return Ok(());
    }

    if s.lines.row(q.line).is_none() {
        return Err(format!("line {} is past EOF", q.line));
    }
    if s.format.language == Some("go") && !structure::is_go_identifier(&q.name) {
        return Err("SYMBOL must be a non-keyword Go identifier".to_string());
    }

    let offsets: Vec<usize> = s
        .tokens
        .iter()
        .filter(|tok| tok.text == q.name && s.lines.at(tok.from) == q.line)
        .map(|tok| tok.from)
        .collect();
    if offsets.is_empty() {
        let nearby = nearby_symbol_lines(s, &q.name, q.line);
        let hint = if nearby.is_empty() {
            "no matching token in this file".to_string()
        } else {
            format!("nearby lines: {nearby}")
        };
        return Err(format!(
            "{} is not a symbol token on the selected line; {hint}",
            q.name
        ));
    }

    if q.occurrence == 0 {
        if offsets.len() > 1 {
            return Err(format!(
                "{} is ambiguous on the selected line ({} occurrences); supply N",
                q.name,
                offsets.len()
            ));
        }
        q.offset = offsets[0];
    } else {
        if q.occurrence > offsets.len() {
            return Err(format!("symbol occurrence {} is missing", q.occurrence));
        }
        q.offset = offsets[q.occurrence - 1];
    }
    Ok(())
}

pub(crate) fn symbol_file_uri(path: &Path) -> String {
    Url::from_file_path(path)
        .map(String::from)
        .unwrap_or_else(|()| format!("file://{}", path_to_slash(path)))
}

pub(crate) fn symbol_uri_path(uri: &str) -> Result<PathBuf, ()> {
    let parsed = Url::parse(uri).map_err(|_| ())?;
    if parsed.scheme() != "file"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !matches!(parsed.host_str(), None | Some("") | Some("localhost"))
    {
        return Err(());
    }
    parsed.to_file_path()
}

pub fn execute_standalone(args: &[String]) -> ToolOutput {
    execute(args, /*dedicated_process*/ true)
}

struct RefRow {
    key: String,
    source: String,
    line: usize,
    queries: String,
}

pub fn execute(args: &[String], dedicated_process: bool) -> ToolOutput {
    execute_with_path(args, dedicated_process, None)
}

pub(crate) fn execute_with_path(
    args: &[String],
    dedicated_process: bool,
    path_override: Option<&std::ffi::OsStr>,
) -> ToolOutput {
    if dedicated_process {
        let _ = lsp::prepare_frontend_cleanup();
    }

    let parsed = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(err) => {
            return ToolOutput {
                stdout: String::new(),
                stderr: format!("msymbol: {err}\n"),
                exit_code: 1,
            };
        }
    };

    let explicit_workspace = parsed.workspace.is_some();
    let root_candidate = match parsed.workspace {
        Some(ws) => PathBuf::from(ws),
        None => match std::env::current_dir() {
            Ok(dir) => dir,
            Err(err) => {
                return ToolOutput {
                    stdout: String::new(),
                    stderr: format!("msymbol: {err}\n"),
                    exit_code: 1,
                };
            }
        },
    };
    let root = match fs::canonicalize(&root_candidate) {
        Ok(canonical) => {
            if !canonical.is_dir() {
                return ToolOutput {
                    stdout: String::new(),
                    stderr: "msymbol: workspace is not a directory\n".to_string(),
                    exit_code: 1,
                };
            }
            canonical
        }
        Err(err) => {
            return ToolOutput {
                stdout: String::new(),
                stderr: format!("msymbol: {err}\n"),
                exit_code: 1,
            };
        }
    };

    let mut queries = parsed.queries;
    let ref_queries = queries.iter().filter(|q| q.mode == QueryMode::Refs).count();

    let mut cache: HashMap<PathBuf, Arc<SymbolSource>> = HashMap::new();
    let mut group_indices: HashMap<&'static str, Vec<usize>> = HashMap::new();

    for (idx, q) in queries.iter_mut().enumerate() {
        match load_symbol_source(&root, &q.raw_path, None, &mut cache) {
            Ok(file) => {
                q.file = Some(file);
                if let Err(err) = select_symbol(q) {
                    q.err = Some(err);
                } else if let Some(resolver) = q
                    .file
                    .as_ref()
                    .and_then(|f| f.parsed.format.semantic_resolver)
                {
                    group_indices.entry(resolver).or_default().push(idx);
                }
            }
            Err(err) => {
                q.err = Some(err.message);
            }
        }
    }

    if !group_indices.is_empty() {
        let scratch = match tempfile::Builder::new().prefix("mekugi-symbol-").tempdir() {
            Ok(dir) => dir,
            Err(err) => {
                return ToolOutput {
                    stdout: String::new(),
                    stderr: format!("msymbol: create resolver scratch: {err}\n"),
                    exit_code: 1,
                };
            }
        };

        // Run per-language groups concurrently using scoped threads.
        let mut grouped_queries: Vec<(&'static str, Vec<(usize, SymbolQuery)>)> = Vec::new();
        for (resolver, indices) in group_indices {
            let mut items = Vec::with_capacity(indices.len());
            for idx in indices {
                items.push((idx, queries[idx].clone()));
            }
            grouped_queries.push((resolver, items));
        }

        let scratch_path = scratch.path().to_path_buf();
        let root_ref = &root;
        let scratch_ref = &scratch_path;
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for (resolver, mut items) in grouped_queries {
                handles.push(scope.spawn(move || {
                    let mut refs: Vec<&mut SymbolQuery> =
                        items.iter_mut().map(|(_, q)| q).collect();
                    lsp::run_lsp(
                        root_ref,
                        resolver,
                        scratch_ref,
                        &mut refs,
                        dedicated_process,
                        path_override,
                    );
                    items
                }));
            }
            for handle in handles {
                if let Ok(items) = handle.join() {
                    for (idx, updated) in items {
                        queries[idx] = updated;
                    }
                }
            }
        });

        if dedicated_process {
            lsp::cleanup_frontend_orphans();
        }
        if let Err(err) = scratch.close()
            && let Some(first) = queries.first_mut()
        {
            let _ = writeln!(first.stderr, "msymbol: resolver scratch cleanup: {err}");
        }
    }

    // Post-resolution source-change race check: never emit semantic results if an input changed.
    let mut verified: HashMap<PathBuf, bool> = HashMap::new();
    let mut result_cache: HashMap<PathBuf, Arc<SymbolSource>> = HashMap::new();
    for q in &mut queries {
        if q.err.is_some() {
            continue;
        }
        let Some(file) = q.file.as_ref() else {
            continue;
        };
        let unchanged = if let Some(&status) = verified.get(&file.path) {
            status
        } else {
            let canon = fs::canonicalize(&file.path).ok();
            let data = fs::read_to_string(&file.path).ok();
            let ok = canon.as_deref() == Some(file.path.as_path())
                && data.as_deref() == Some(file.parsed.source.as_str());
            verified.insert(file.path.clone(), ok);
            if ok {
                result_cache.insert(file.path.clone(), Arc::clone(file));
            }
            ok
        };
        if !unchanged {
            q.err = Some("input changed during query".to_string());
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = 0;

    let mut skipped: HashMap<String, usize> = HashMap::new();
    let mut skip_order: Vec<String> = Vec::new();
    let mut record_skip = |reason: &str| {
        let count = skipped.entry(reason.to_string()).or_insert_with(|| {
            skip_order.push(reason.to_string());
            0
        });
        *count += 1;
    };

    let mut shared_rows: Vec<RefRow> = Vec::new();
    let mut shared_key_to_idx: HashMap<String, usize> = HashMap::new();
    let mut shared_files: HashMap<String, Vec<usize>> = HashMap::new();
    let mut shared_order: Vec<String> = Vec::new();

    let total_queries = queries.len();
    let emit_fail = |stderr: &mut String, exit_code: &mut i32, q: &SymbolQuery, msg: &str| {
        let label = if total_queries > 1 {
            let line = if q.line > 0 {
                q.line.to_string()
            } else {
                String::new()
            };
            format!(
                "{} {} {line} {}: ",
                q.mode.as_str(),
                quote_json_string(&q.raw_path),
                q.name
            )
        } else {
            String::new()
        };
        let _ = writeln!(stderr, "msymbol: {label}{msg}");
        *exit_code = 1;
    };

    for (query_index, q) in queries.iter().enumerate() {
        if !q.stderr.is_empty() {
            stderr.push_str(&q.stderr);
            if !stderr.ends_with('\n') {
                stderr.push('\n');
            }
        }
        if let Some(err) = &q.err {
            emit_fail(&mut stderr, &mut exit_code, q, err);
            continue;
        }
        let Some(q_file) = q.file.as_ref() else {
            continue;
        };
        let expected_resolver = q_file.parsed.format.semantic_resolver;

        let mut seen: HashSet<String> = HashSet::new();
        let mut references: HashMap<String, Vec<RefRow>> = HashMap::new();
        let mut reference_order: Vec<String> = Vec::new();
        let mut emitted = false;

        for loc in &q.locations {
            if loc.outside {
                record_skip("outside workspace");
                continue;
            }
            let loc_path_str = loc.path.to_string_lossy();
            let file = match load_symbol_source(
                &root,
                &loc_path_str,
                expected_resolver,
                &mut result_cache,
            ) {
                Ok(file) => file,
                Err(err) => {
                    record_skip(&err.reason);
                    continue;
                }
            };
            let (Some(from), Some(to)) = (
                file.parsed.lines.offset(loc.range.start),
                file.parsed.lines.offset(loc.range.end),
            ) else {
                record_skip("unavailable");
                continue;
            };
            if to < from {
                record_skip("unavailable");
                continue;
            }

            let mut start = loc.range.start.line + 1;
            let mut end = start;
            if q.mode == QueryMode::Def {
                for entry in &file.parsed.entries {
                    if entry.kind != "import"
                        && entry.complete
                        && entry.name_from == Some(from)
                        && entry.name_to == Some(to)
                    {
                        start = entry.line;
                        end = entry.end_line;
                        break;
                    }
                }
            }

            let mut header_end = start.saturating_sub(1);
            for line in start..=end {
                let Some(row) = file.parsed.lines.row(line) else {
                    record_skip("unavailable");
                    break;
                };
                let key = format!("{}\0{line}", file.path.display());
                if !seen.insert(key.clone()) {
                    continue;
                }
                let display_path = if explicit_workspace {
                    file.path.clone()
                } else {
                    relative_to_root(&root, &file.path)
                };
                let label = quote_json_string(&path_to_slash(&display_path));

                if q.mode == QueryMode::Def {
                    if line > header_end {
                        header_end = line;
                        while header_end < end
                            && !seen.contains(&format!(
                                "{}\0{}",
                                file.path.display(),
                                header_end + 1
                            ))
                        {
                            header_end += 1;
                        }
                        let _ = writeln!(stdout, "{label}:{line}-{header_end}");
                    }
                    stdout.push_str(row);
                    stdout.push('\n');
                } else {
                    if !references.contains_key(&label) {
                        reference_order.push(label.clone());
                    }
                    references.entry(label).or_default().push(RefRow {
                        key,
                        source: row.to_string(),
                        line,
                        queries: String::new(),
                    });
                }
                emitted = true;
            }
        }

        if q.mode == QueryMode::Refs && ref_queries > 1 {
            let id = (query_index + 1).to_string();
            let display_path = if explicit_workspace {
                q_file.path.clone()
            } else {
                relative_to_root(&root, &q_file.path)
            };
            let _ = write!(
                stdout,
                "refs[{id}] {}:{} {}",
                quote_json_string(&path_to_slash(&display_path)),
                q.line,
                q.name
            );
            if q.occurrence > 0 {
                let _ = write!(stdout, " {}", q.occurrence);
            }
            stdout.push('\n');

            for label in reference_order {
                if let Some(rows) = references.remove(&label) {
                    for mut row in rows {
                        if let Some(&existing_idx) = shared_key_to_idx.get(&row.key) {
                            shared_rows[existing_idx].queries.push(',');
                            shared_rows[existing_idx].queries.push_str(&id);
                        } else {
                            let new_idx = shared_rows.len();
                            row.queries = id.clone();
                            shared_key_to_idx.insert(row.key.clone(), new_idx);
                            shared_rows.push(row);
                            if !shared_files.contains_key(&label) {
                                shared_order.push(label.clone());
                            }
                            shared_files.entry(label.clone()).or_default().push(new_idx);
                        }
                    }
                }
            }
        } else if q.mode == QueryMode::Refs {
            for label in reference_order {
                let _ = writeln!(stdout, "{label}:");
                if let Some(rows) = references.get(&label) {
                    for row in rows {
                        let _ = writeln!(stdout, "{} {}", row.line, row.source);
                    }
                }
            }
        }

        if q.mode == QueryMode::Def && !emitted {
            emit_fail(
                &mut stderr,
                &mut exit_code,
                q,
                "definition has no editable workspace location",
            );
        }
        if let Some(cov) = &q.coverage {
            emit_fail(&mut stderr, &mut exit_code, q, cov);
        }
    }

    for label in shared_order {
        let _ = writeln!(stdout, "{label}:");
        if let Some(indices) = shared_files.get(&label) {
            for &idx in indices {
                let row = &shared_rows[idx];
                let _ = writeln!(stdout, "{} [{}] {}", row.line, row.queries, row.source);
            }
        }
    }

    if !skip_order.is_empty() {
        let parts: Vec<String> = skip_order
            .iter()
            .map(|reason| {
                let count = skipped[reason];
                let noun = if count == 1 { "location" } else { "locations" };
                format!("{count} {noun} {reason}")
            })
            .collect();
        let _ = writeln!(stderr, "msymbol: skipped {}", parts.join(", "));
    }

    ToolOutput {
        stdout,
        stderr,
        exit_code,
    }
}

#[cfg(test)]
mod tests;
