use {
    clap::Parser,
    ll_core::Config,
    serde::Deserialize,
    serde_json::{json, Value},
    std::{
        collections::{BTreeSet, HashSet},
        fs,
        io::{self, BufRead, Write},
        path::{Path, PathBuf},
    },
};

const DEFAULT_REQUIREMENTS: [&str; 3] = ["symbol", "footprint", "3d"];
const DEFAULT_MODEL_EXTENSIONS: [&str; 4] = ["stp", "step", "wrl", "stl"];
const MAX_SCAN_DEPTH: usize = 8;
const MAX_SCAN_ENTRIES: usize = 30_000;
const MAX_MATCHES: usize = 100;
const MAX_TEXT_FILE_BYTES: u64 = 16 * 1024 * 1024;

type AppResult<T> = std::result::Result<T, String>;

#[derive(Parser)]
#[command(version, about = "MCP server for SamacSys Library Loader")]
struct Cli {
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct RpcMessage {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Deserialize)]
struct ConfigArgs {
    config_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DownloadEpwArgs {
    path: String,
    config_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DownloadPartIdArgs {
    part_id: u32,
    config_path: Option<String>,
    mpn: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ServerProbeArgs {
    part_id: u32,
    config_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ServerFormatProbeArgs {
    part_id: u32,
    requirements: Option<Vec<String>>,
    config_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExistsArgs {
    mpn: String,
    config_path: Option<String>,
    requirements: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct HasRequiredFormatsArgs {
    mpn: String,
    requirements: Vec<String>,
    config_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VetPartArgs {
    mpn: String,
    manufacturer: Option<String>,
    requirements: Option<Vec<String>>,
    config_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Requirement {
    Symbol,
    Footprint,
    Model3d,
    Legacy,
    Zip,
}

impl Requirement {
    fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "symbol" | "symbols" | "schematic" => Some(Self::Symbol),
            "footprint" | "footprints" | "pcb" => Some(Self::Footprint),
            "3d" | "model" | "models" | "3d_model" | "3d_models" => Some(Self::Model3d),
            "legacy" | "legacy_symbol" | "legacy_symbols" => Some(Self::Legacy),
            "zip" | "archive" => Some(Self::Zip),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::Footprint => "footprint",
            Self::Model3d => "3d",
            Self::Legacy => "legacy",
            Self::Zip => "zip",
        }
    }
}

#[derive(Default)]
struct ScanState {
    matches: BTreeSet<String>,
    roots_checked: BTreeSet<String>,
    errors: Vec<String>,
    entries_seen: usize,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("library-loader-mcp: {err}");
        std::process::exit(1);
    }
}

fn run() -> AppResult<()> {
    let cli = Cli::parse();
    let default_config = cli
        .config
        .map(|path| expand_path_buf(path.as_path()))
        .transpose()?;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line.map_err(|err| err.to_string())?;
        if line.trim().is_empty() {
            continue;
        }

        let response = handle_line(&default_config, &line);
        if let Some(response) = response {
            let encoded = serde_json::to_string(&response).map_err(|err| err.to_string())?;
            writeln!(stdout, "{encoded}").map_err(|err| err.to_string())?;
            stdout.flush().map_err(|err| err.to_string())?;
        }
    }

    Ok(())
}

fn handle_line(default_config: &Option<PathBuf>, line: &str) -> Option<Value> {
    let msg: RpcMessage = match serde_json::from_str(line) {
        Ok(msg) => msg,
        Err(err) => {
            return Some(rpc_error(
                Value::Null,
                -32700,
                "Parse error",
                Some(json!(err.to_string())),
            ));
        }
    };

    let is_notification = msg.id.is_none();
    let id = msg.id.unwrap_or(Value::Null);
    let result = match msg.method.as_str() {
        "initialize" => Ok(initialize_result(&msg.params)),
        "notifications/initialized" => return None,
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call_tool(default_config, &msg.params),
        _ => Err(RpcToolError::method_not_found(format!(
            "Unknown method: {}",
            msg.method
        ))),
    };

    match result {
        Ok(value) => {
            if is_notification {
                None
            } else {
                Some(rpc_success(id, value))
            }
        }
        Err(err) => {
            if is_notification {
                None
            } else {
                Some(err.into_rpc(id))
            }
        }
    }
}

fn initialize_result(params: &Value) -> Value {
    let protocol_version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2024-11-05");

    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": "library-loader-mcp",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Use offline tools before downloads. validate_config and has_required_formats never contact ComponentSearchEngine. download_epw and download_part_id may download library assets."
    })
}

fn call_tool(default_config: &Option<PathBuf>, params: &Value) -> Result<Value, RpcToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcToolError::invalid_params("Missing tool name"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let result = match name {
        "validate_config" => {
            let args: ConfigArgs = parse_args(arguments)?;
            validate_config(default_config, args.config_path)
        }
        "list_formats" => {
            let args: ConfigArgs = parse_args(arguments)?;
            list_formats(default_config, args.config_path)
        }
        "exists" | "exists_locally" => {
            let args: ExistsArgs = parse_args(arguments)?;
            let requirements = args.requirements.unwrap_or_else(|| {
                DEFAULT_REQUIREMENTS
                    .iter()
                    .map(|req| (*req).to_owned())
                    .collect()
            });
            has_required_formats(default_config, args.config_path, &args.mpn, requirements)
        }
        "exists_on_server" => {
            let args: ServerProbeArgs = parse_args(arguments)?;
            exists_on_server(default_config, args)
        }
        "format_exists_server" => {
            let args: ServerFormatProbeArgs = parse_args(arguments)?;
            format_exists_server(default_config, args)
        }
        "has_required_formats" => {
            let args: HasRequiredFormatsArgs = parse_args(arguments)?;
            has_required_formats(
                default_config,
                args.config_path,
                &args.mpn,
                args.requirements,
            )
        }
        "vet_part" => {
            let args: VetPartArgs = parse_args(arguments)?;
            vet_part(default_config, args)
        }
        "download_epw" => {
            let args: DownloadEpwArgs = parse_args(arguments)?;
            download_epw(default_config, args)
        }
        "download_part_id" => {
            let args: DownloadPartIdArgs = parse_args(arguments)?;
            download_part_id(default_config, args)
        }
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(value) => Ok(tool_success(value)),
        Err(err) => Ok(tool_error(err)),
    }
}

fn parse_args<T: for<'de> Deserialize<'de>>(arguments: Value) -> Result<T, RpcToolError> {
    serde_json::from_value(arguments)
        .map_err(|err| RpcToolError::invalid_params(format!("Invalid tool arguments: {err}")))
}

fn validate_config(
    default_config: &Option<PathBuf>,
    config_path: Option<String>,
) -> AppResult<Value> {
    let (config, path) = read_config(default_config, config_path.as_deref())?;
    let formats = format_summaries(&config);
    let kicad_formats = formats
        .iter()
        .filter(|format| format.get("format").and_then(Value::as_str) == Some("kicad"))
        .count();
    let quartzpulse_paths = formats
        .iter()
        .filter(|format| {
            ["output_path", "model_output_path"]
                .iter()
                .any(|key| value_contains_quartzpulse(format.get(*key)))
        })
        .count();

    Ok(json!({
        "valid": true,
        "config_path": display_path(&path),
        "profile": {
            "username_configured": !config.profile.username.is_empty(),
            "password_configured": !config.profile.password.is_empty()
        },
        "settings": {
            "watch_path": config.settings.watch_path,
            "recursive": config.settings.recursive
        },
        "format_count": config.formats.len(),
        "kicad_format_count": kicad_formats,
        "quartzpulse_path_count": quartzpulse_paths,
        "formats": formats
    }))
}

fn list_formats(default_config: &Option<PathBuf>, config_path: Option<String>) -> AppResult<Value> {
    let (config, path) = read_config(default_config, config_path.as_deref())?;
    Ok(json!({
        "config_path": display_path(&path),
        "formats": format_summaries(&config)
    }))
}

fn has_required_formats(
    default_config: &Option<PathBuf>,
    config_path: Option<String>,
    mpn: &str,
    requested_requirements: Vec<String>,
) -> AppResult<Value> {
    let (config, path) = read_config(default_config, config_path.as_deref())?;
    let result = analyze_part_assets(&config, &path, mpn, requested_requirements)?;
    Ok(result)
}

fn vet_part(default_config: &Option<PathBuf>, args: VetPartArgs) -> AppResult<Value> {
    let requirements = args.requirements.unwrap_or_else(|| {
        DEFAULT_REQUIREMENTS
            .iter()
            .map(|req| (*req).to_owned())
            .collect()
    });
    let (config, path) = read_config(default_config, args.config_path.as_deref())?;
    let mut result = analyze_part_assets(&config, &path, &args.mpn, requirements)?;
    let missing = result
        .get("requirements")
        .and_then(Value::as_array)
        .map(|requirements| {
            requirements
                .iter()
                .filter_map(|requirement| {
                    let present = requirement.get("present").and_then(Value::as_bool)?;
                    if present {
                        None
                    } else {
                        requirement
                            .get("requirement")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    }
                })
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    let recommendation = if missing.is_empty() {
        "ready"
    } else {
        "missing_assets"
    };

    if let Some(obj) = result.as_object_mut() {
        obj.insert("manufacturer".to_owned(), json!(args.manufacturer));
        obj.insert("recommendation".to_owned(), json!(recommendation));
        obj.insert("missing".to_owned(), json!(missing));
    }

    Ok(result)
}

fn exists_on_server(default_config: &Option<PathBuf>, args: ServerProbeArgs) -> AppResult<Value> {
    if args.part_id == 0 {
        return Err("part_id must be greater than zero".to_owned());
    }
    let (config, path) = read_config(default_config, args.config_path.as_deref())?;
    let probe = ll_core::probe_part_id(&config, args.part_id).map_err(|err| err.to_string())?;
    Ok(json!({
        "config_path": display_path(&path),
        "probe": probe_to_json(probe),
        "query_method": "HEAD",
        "downloaded_body": false
    }))
}

fn format_exists_server(
    default_config: &Option<PathBuf>,
    args: ServerFormatProbeArgs,
) -> AppResult<Value> {
    if args.part_id == 0 {
        return Err("part_id must be greater than zero".to_owned());
    }
    let requirements = args.requirements.unwrap_or_else(|| {
        DEFAULT_REQUIREMENTS
            .iter()
            .map(|req| (*req).to_owned())
            .collect()
    });
    let (config, path) = read_config(default_config, args.config_path.as_deref())?;
    let probe = ll_core::probe_part_id(&config, args.part_id).map_err(|err| err.to_string())?;
    Ok(json!({
        "config_path": display_path(&path),
        "part_id": args.part_id,
        "requirements": requirements,
        "server_archive": probe_to_json(probe),
        "format_specific_supported": false,
        "format_specific_available": null,
        "reason": "ComponentSearchEngine HEAD confirms whether the part archive is available, but it does not expose symbol/footprint/3D contents. Per-format server validation requires downloading and inspecting the ZIP."
    }))
}

fn download_epw(default_config: &Option<PathBuf>, args: DownloadEpwArgs) -> AppResult<Value> {
    let (config, path) = read_config(default_config, args.config_path.as_deref())?;
    let epw_path = expand_string_path(&args.path)?;
    let saved_paths =
        ll_core::download_once(&config, epw_path.clone()).map_err(|err| err.to_string())?;
    Ok(json!({
        "config_path": display_path(&path),
        "input_path": display_path(&epw_path),
        "saved_paths": display_paths(saved_paths)
    }))
}

fn download_part_id(
    default_config: &Option<PathBuf>,
    args: DownloadPartIdArgs,
) -> AppResult<Value> {
    let (config, path) = read_config(default_config, args.config_path.as_deref())?;
    let saved_paths =
        ll_core::download_part_id(&config, args.part_id).map_err(|err| err.to_string())?;
    let mut result = json!({
        "config_path": display_path(&path),
        "part_id": args.part_id,
        "saved_paths": display_paths(saved_paths)
    });

    if let Some(mpn) = args.mpn {
        let post_download = analyze_part_assets(
            &config,
            &path,
            &mpn,
            DEFAULT_REQUIREMENTS
                .iter()
                .map(|req| (*req).to_owned())
                .collect(),
        )?;
        if let Some(obj) = result.as_object_mut() {
            obj.insert("post_download_assets".to_owned(), post_download);
        }
    }

    Ok(result)
}

fn analyze_part_assets(
    config: &Config,
    config_path: &Path,
    mpn: &str,
    requested_requirements: Vec<String>,
) -> AppResult<Value> {
    let clean_mpn = mpn.trim();
    if clean_mpn.is_empty() {
        return Err("mpn must not be empty".to_owned());
    }

    let mut unknown = Vec::new();
    let mut requirements = Vec::new();
    let mut seen = HashSet::new();
    for requested in requested_requirements {
        match Requirement::parse(&requested) {
            Some(requirement) if seen.insert(requirement.as_str()) => {
                requirements.push(requirement)
            }
            Some(_) => {}
            None => unknown.push(requested),
        }
    }

    if requirements.is_empty() {
        return Err("At least one valid requirement is required".to_owned());
    }

    let mut statuses = Vec::new();
    for requirement in requirements {
        let scan = scan_requirement(config, requirement, clean_mpn);
        statuses.push(json!({
            "requirement": requirement.as_str(),
            "present": !scan.matches.is_empty(),
            "matches": scan.matches.into_iter().collect::<Vec<String>>(),
            "roots_checked": scan.roots_checked.into_iter().collect::<Vec<String>>(),
            "errors": scan.errors
        }));
    }

    let all_present = statuses.iter().all(|status| {
        status
            .get("present")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });

    Ok(json!({
        "mpn": clean_mpn,
        "config_path": display_path(config_path),
        "exists": all_present,
        "all_required_present": all_present,
        "requirements": statuses,
        "unknown_requirements": unknown,
        "offline_only": true
    }))
}

fn scan_requirement(config: &Config, requirement: Requirement, mpn: &str) -> ScanState {
    let mut state = ScanState::default();

    for (name, format) in &config.formats {
        let output_path = match expand_string_path(&format.output_path) {
            Ok(path) => path,
            Err(err) => {
                state.errors.push(format!("{name}.output_path: {err}"));
                continue;
            }
        };

        let model_extensions = model_extensions(format);
        for root in roots_for_requirement(name, &output_path, format, requirement) {
            scan_root(&root, requirement, mpn, &model_extensions, &mut state, 0);
            if state.matches.len() >= MAX_MATCHES || state.entries_seen >= MAX_SCAN_ENTRIES {
                break;
            }
        }

        if state.matches.len() >= MAX_MATCHES || state.entries_seen >= MAX_SCAN_ENTRIES {
            break;
        }
    }

    if state.entries_seen >= MAX_SCAN_ENTRIES {
        state.errors.push(format!(
            "scan stopped after {MAX_SCAN_ENTRIES} filesystem entries"
        ));
    }

    state
}

fn roots_for_requirement(
    name: &str,
    output_path: &Path,
    format: &ll_core::Format,
    requirement: Requirement,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if format.format == ll_core::ECAD::KiCad {
        match requirement {
            Requirement::Symbol => {
                roots.push(output_path.join(format!("{name}.kicad_sym")));
                roots.push(output_path.join(format!("{name}.legacy")));
            }
            Requirement::Footprint => {
                roots.push(output_path.join(format!("{name}.pretty")));
            }
            Requirement::Model3d => {
                roots.push(model_root(name, output_path, format));
                roots.push(output_path.join(format!("{name}.pretty")));
            }
            Requirement::Legacy => {
                roots.push(output_path.join(format!("{name}.legacy")));
            }
            Requirement::Zip => {}
        }
    } else {
        match requirement {
            Requirement::Symbol
            | Requirement::Footprint
            | Requirement::Model3d
            | Requirement::Legacy => {
                roots.push(output_path.to_owned());
            }
            Requirement::Zip => {
                if format.format == ll_core::ECAD::Zip {
                    roots.push(output_path.to_owned());
                }
            }
        }
    }

    dedupe_paths(roots)
}

fn model_root(name: &str, output_path: &Path, format: &ll_core::Format) -> PathBuf {
    match &format.model_output_path {
        Some(path) => match expand_string_path(path) {
            Ok(expanded) if expanded.is_absolute() => expanded,
            Ok(expanded) => output_path.join(expanded),
            Err(_) => PathBuf::from(path),
        },
        None => output_path.join(format!("{name}.pretty")),
    }
}

fn scan_root(
    root: &Path,
    requirement: Requirement,
    mpn: &str,
    model_extensions: &[String],
    state: &mut ScanState,
    depth: usize,
) {
    if !root.exists() {
        return;
    }
    state.roots_checked.insert(display_path(root));
    scan_path(root, requirement, mpn, model_extensions, state, depth);
}

fn scan_path(
    root: &Path,
    requirement: Requirement,
    mpn: &str,
    model_extensions: &[String],
    state: &mut ScanState,
    depth: usize,
) {
    if state.matches.len() >= MAX_MATCHES || state.entries_seen >= MAX_SCAN_ENTRIES {
        return;
    }

    if root.is_file() {
        state.entries_seen += 1;
        if file_matches_requirement(root, requirement, mpn, model_extensions) {
            state.matches.insert(display_path(root));
        }
        return;
    }

    if depth >= MAX_SCAN_DEPTH {
        return;
    }

    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) => {
            state.errors.push(format!("{}: {err}", display_path(root)));
            return;
        }
    };

    for entry in entries {
        if state.matches.len() >= MAX_MATCHES || state.entries_seen >= MAX_SCAN_ENTRIES {
            return;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                state.errors.push(err.to_string());
                continue;
            }
        };
        let path = entry.path();
        state.entries_seen += 1;
        if path.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            scan_path(&path, requirement, mpn, model_extensions, state, depth + 1);
        } else if file_matches_requirement(&path, requirement, mpn, model_extensions) {
            state.matches.insert(display_path(&path));
        }
    }
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".git" | ".history" | "node_modules" | "target"))
}

fn file_matches_requirement(
    path: &Path,
    requirement: Requirement,
    mpn: &str,
    model_extensions: &[String],
) -> bool {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match requirement {
        Requirement::Symbol => {
            matches!(ext.as_str(), "kicad_sym" | "lib" | "dcm")
                && (path_matches_part(path, mpn) || text_file_contains(path, mpn))
        }
        Requirement::Footprint => {
            ext == "kicad_mod" && (path_matches_part(path, mpn) || text_file_contains(path, mpn))
        }
        Requirement::Model3d => {
            model_extensions.iter().any(|model_ext| model_ext == &ext)
                && path_matches_part(path, mpn)
        }
        Requirement::Legacy => {
            matches!(ext.as_str(), "lib" | "dcm")
                && (path_matches_part(path, mpn) || text_file_contains(path, mpn))
        }
        Requirement::Zip => ext == "zip" && path_matches_part(path, mpn),
    }
}

fn text_file_contains(path: &Path, mpn: &str) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if metadata.len() > MAX_TEXT_FILE_BYTES {
        return false;
    }
    let Ok(data) = fs::read(path) else {
        return false;
    };
    let text = String::from_utf8_lossy(&data);
    matches_part(&text, mpn)
}

fn path_matches_part(path: &Path, mpn: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches_part(name, mpn))
}

fn matches_part(haystack: &str, needle: &str) -> bool {
    let haystack_lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    haystack_lower.contains(&needle_lower)
        || normalize_for_match(&haystack_lower).contains(&normalize_for_match(&needle_lower))
}

fn normalize_for_match(input: &str) -> String {
    input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn read_config(
    default_config: &Option<PathBuf>,
    requested_path: Option<&str>,
) -> AppResult<(Config, PathBuf)> {
    let path = resolve_config_path(default_config, requested_path)?;
    let config = Config::read(Some(path.clone())).map_err(|err| err.to_string())?;
    Ok((config, path))
}

fn resolve_config_path(
    default_config: &Option<PathBuf>,
    requested_path: Option<&str>,
) -> AppResult<PathBuf> {
    if let Some(path) = requested_path {
        return expand_string_path(path);
    }
    if let Some(path) = default_config {
        return Ok(path.clone());
    }
    Config::get_path()
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "No LibraryLoader.toml found".to_owned())
}

fn format_summaries(config: &Config) -> Vec<Value> {
    let mut formats = config
        .formats
        .iter()
        .map(|(name, format)| {
            let output_path = expand_string_path(&format.output_path).ok();
            let model_output_path = format
                .model_output_path
                .as_deref()
                .and_then(|path| expand_string_path(path).ok());
            json!({
                "name": name,
                "format": format.format.to_string(),
                "output_path": format.output_path,
                "expanded_output_path": output_path.as_ref().map(|path| display_path(path)),
                "output_path_exists": output_path.as_ref().is_some_and(|path| path.exists()),
                "model_output_path": format.model_output_path,
                "expanded_model_output_path": model_output_path.as_ref().map(|path| display_path(path)),
                "model_output_path_exists": model_output_path.as_ref().is_some_and(|path| path.exists()),
                "model_uri": format.model_uri,
                "model_formats": model_extensions(format),
                "create_folder": format.create_folder.unwrap_or(false)
            })
        })
        .collect::<Vec<_>>();
    formats.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .cmp(&b.get("name").and_then(Value::as_str))
    });
    formats
}

fn model_extensions(format: &ll_core::Format) -> Vec<String> {
    let mut extensions = if format.model_formats.is_empty() {
        DEFAULT_MODEL_EXTENSIONS
            .iter()
            .map(|ext| (*ext).to_owned())
            .collect()
    } else {
        format
            .model_formats
            .iter()
            .filter_map(|ext| {
                let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
                (!ext.is_empty()).then_some(ext)
            })
            .collect::<Vec<_>>()
    };
    extensions.sort();
    extensions.dedup();
    extensions
}

fn value_contains_quartzpulse(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|path| path.contains("Documents/QuartzPulse") || path.contains("QuartzPulse"))
}

fn expand_string_path(path: &str) -> AppResult<PathBuf> {
    shellexpand::full(path)
        .map(|path| PathBuf::from(path.as_ref()))
        .map_err(|err| err.to_string())
}

fn expand_path_buf(path: &Path) -> AppResult<PathBuf> {
    expand_string_path(&path.to_string_lossy())
}

fn display_paths(paths: Vec<PathBuf>) -> Vec<String> {
    paths.into_iter().map(|path| display_path(&path)).collect()
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn probe_to_json(probe: ll_core::PartProbe) -> Value {
    json!({
        "part_id": probe.part_id,
        "available": probe.available,
        "status": probe.status,
        "content_type": probe.content_type,
        "filename": probe.filename,
        "content_length": probe.content_length,
        "downloaded_body": probe.downloaded_body
    })
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for path in paths {
        let key = display_path(&path);
        if seen.insert(key) {
            deduped.push(path);
        }
    }
    deduped
}

fn tool_success(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ],
        "structuredContent": value
    })
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": message
            }
        ],
        "isError": true
    })
}

fn rpc_success(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn rpc_error(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({
        "code": code,
        "message": message
    });
    if let Some(data) = data {
        if let Some(obj) = error.as_object_mut() {
            obj.insert("data".to_owned(), data);
        }
    }

    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error
    })
}

struct RpcToolError {
    code: i64,
    message: String,
}

impl RpcToolError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    fn method_not_found(message: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: message.into(),
        }
    }

    fn into_rpc(self, id: Value) -> Value {
        rpc_error(id, self.code, &self.message, None)
    }
}

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "validate_config",
            "description": "Validate LibraryLoader.toml and summarize configured outputs without revealing credentials.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "config_path": { "type": "string", "description": "Optional LibraryLoader.toml path. Defaults to the server --config path." }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "list_formats",
            "description": "List configured ECAD export formats, output paths, and 3D model settings. Does not contact ComponentSearchEngine.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "config_path": { "type": "string" }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "exists",
            "description": "Alias for exists_locally. Offline shorthand for checking whether a local part already has required CAD assets. Defaults to symbol, footprint, and 3D.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mpn": { "type": "string", "description": "Manufacturer part number to check locally." },
                    "requirements": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["symbol", "footprint", "3d", "legacy", "zip"] }
                    },
                    "config_path": { "type": "string" }
                },
                "required": ["mpn"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "exists_locally",
            "description": "Offline check for whether a local MPN already has required CAD assets. Defaults to symbol, footprint, and 3D.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mpn": { "type": "string", "description": "Manufacturer part number to check locally." },
                    "requirements": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["symbol", "footprint", "3d", "legacy", "zip"] }
                    },
                    "config_path": { "type": "string" }
                },
                "required": ["mpn"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "exists_on_server",
            "description": "Check whether ComponentSearchEngine has a part archive for a part ID using HEAD only. Does not download the ZIP body.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "part_id": { "type": "integer", "minimum": 1 },
                    "config_path": { "type": "string" }
                },
                "required": ["part_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "format_exists_server",
            "description": "Probe server archive availability for a part ID and explicitly report that symbol/footprint/3D server-specific existence is not exposed without downloading and inspecting the ZIP.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "part_id": { "type": "integer", "minimum": 1 },
                    "requirements": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["symbol", "footprint", "3d", "legacy", "zip"] }
                    },
                    "config_path": { "type": "string" }
                },
                "required": ["part_id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "has_required_formats",
            "description": "Offline check for selected local asset classes for an MPN. Use before downloading.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mpn": { "type": "string" },
                    "requirements": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["symbol", "footprint", "3d", "legacy", "zip"] },
                        "minItems": 1
                    },
                    "config_path": { "type": "string" }
                },
                "required": ["mpn", "requirements"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "vet_part",
            "description": "Offline part-shopping helper that reports whether local CAD assets satisfy requirements and what is missing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mpn": { "type": "string" },
                    "manufacturer": { "type": "string" },
                    "requirements": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["symbol", "footprint", "3d", "legacy", "zip"] }
                    },
                    "config_path": { "type": "string" }
                },
                "required": ["mpn"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "download_epw",
            "description": "Download and install a ComponentSearchEngine EPW or EPW-containing ZIP. This contacts ComponentSearchEngine.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to an .epw file or zip containing an EPW." },
                    "config_path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "download_part_id",
            "description": "Download and install CAD assets by ComponentSearchEngine part ID. This contacts ComponentSearchEngine.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "part_id": { "type": "integer", "minimum": 1 },
                    "mpn": { "type": "string", "description": "Optional MPN for a post-download local asset check." },
                    "config_path": { "type": "string" }
                },
                "required": ["part_id"],
                "additionalProperties": false
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn matches_normalized_part_names() {
        assert!(matches_part("SS20PH102_M3_I.step", "SS20PH102-M3/I"));
        assert!(matches_part("ATMEGA328P-AU.kicad_mod", "ATMEGA328P-AU"));
        assert!(!matches_part("ATMEGA328P-AU.kicad_mod", "STM32G030"));
    }

    #[test]
    fn offline_asset_scan_finds_symbol_footprint_and_model() {
        let root = temp_dir("asset-scan");
        fs::create_dir_all(root.join("QuartzPulseLib.pretty")).expect("pretty dir");
        fs::write(
            root.join("QuartzPulseLib.kicad_sym"),
            "(symbol \"SS20PH102-M3/I\")",
        )
        .expect("symbol");
        fs::write(
            root.join("QuartzPulseLib.pretty/SS20PH102-M3_I.kicad_mod"),
            "(footprint \"SS20PH102-M3/I\")",
        )
        .expect("footprint");
        fs::create_dir_all(root.join("models")).expect("models dir");
        fs::write(root.join("models/SS20PH102-M3_I.step"), b"model").expect("model");

        let config = Config::read(Some(write_config(&root))).expect("config");
        let result = analyze_part_assets(
            &config,
            Path::new("LibraryLoader.toml"),
            "SS20PH102-M3/I",
            DEFAULT_REQUIREMENTS
                .iter()
                .map(|req| (*req).to_owned())
                .collect(),
        )
        .expect("analysis");

        assert_eq!(result["all_required_present"], true);
    }

    fn write_config(root: &Path) -> PathBuf {
        let path = root.join("LibraryLoader.toml");
        let escaped = root.to_string_lossy();
        fs::write(
            &path,
            format!(
                r#"
[settings]
watch_path = "{escaped}"
recursive = false

[profile]
username = ""
password = ""

[formats.QuartzPulseLib]
format = "kicad"
output_path = "{escaped}"
model_output_path = "models"
model_uri = "${{KICAD_USER_3DMODELDIR}}"
model_formats = ["step", "stp", "wrl"]
"#
            ),
        )
        .expect("write config");
        path
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "library-loader-mcp-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("temp dir");
        path
    }
}
