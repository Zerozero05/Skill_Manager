//! Local runtime folders are registered in place; this module never imports a
//! second copy. All tests use synthetic folders under the repository's work/.
use crate::model::{now, Manager};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

const MAX_TEXT_BYTES: u64 = 1024 * 1024;
const MAX_SCAN_DEPTH: usize = 32;
pub(crate) const RUNTIME_ANALYZER_VERSION: u32 = 3;

fn err_path(context: &str, path: &Path, e: impl std::fmt::Display) -> String {
    format!("{context} ({}): {e}", path.display())
}

/// Resolve an explicitly chosen root, including a root junction. Links below
/// that root are rejected separately before their contents are read.
pub fn canonical(path: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|e| err_path("Cannot resolve path", path, e))
}

fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn readable_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return rest.to_owned();
        }
    }
    s.into_owned()
}

fn path_key(path: &Path) -> String {
    #[cfg(windows)]
    {
        readable_path(path).replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}

fn valid_part(part: &str) -> bool {
    if part.is_empty() || part == "." || part == ".." {
        return false;
    }
    if part
        .chars()
        .any(|c| c.is_control() || "<>:\"|?*\\/".contains(c))
        || part.ends_with(['.', ' '])
    {
        return false;
    }
    let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) && !matches!(
        stem.strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT")),
        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
    )
}

fn relative_parts(relative: &str) -> Result<Vec<&str>, String> {
    let parts: Vec<_> = relative.split('/').collect();
    if parts.is_empty() || parts.iter().any(|p| !valid_part(p)) {
        return Err(format!("Unsafe relative file path: {relative}"));
    }
    Ok(parts)
}

/// Validate each existing component before joining. This supports new files,
/// but never traverses an interior link or a Windows reparse point.
pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let parts = relative_parts(relative)?;
    let resolved_root = canonical(root)?;
    if !resolved_root.is_dir() {
        return Err("Skill root must be a directory".into());
    }
    let mut joined = resolved_root;
    for part in parts {
        joined.push(part);
        match fs::symlink_metadata(&joined) {
            Ok(meta) if is_link(&meta) => {
                return Err(format!(
                    "Interior links/reparse points are not allowed: {}",
                    joined.display()
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err_path("Cannot inspect path", &joined, e)),
        }
    }
    Ok(joined)
}

fn is_artifact(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".skill-manager" | ".skill-manager-state.json"
    )
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut input = fs::File::open(path).map_err(|e| err_path("Cannot read file", path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let n = input
            .read(&mut buffer)
            .map_err(|e| err_path("Cannot hash file", path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn files(root: &Path) -> Result<BTreeMap<String, String>, String> {
    fn walk(
        root: &Path,
        dir: &Path,
        out: &mut BTreeMap<String, String>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_SCAN_DEPTH {
            return Err("Skill folder nesting exceeds the safe scan limit".into());
        }
        for entry in fs::read_dir(dir).map_err(|e| err_path("Cannot enumerate folder", dir, e))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "File name is not valid Unicode")?;
            if is_artifact(&name) {
                continue;
            }
            if !valid_part(&name) {
                return Err(format!("Unsupported or unsafe file name: {name}"));
            }
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)
                .map_err(|e| err_path("Cannot inspect file", &path, e))?;
            if is_link(&meta) {
                return Err(format!(
                    "Interior links/reparse points are not allowed: {}",
                    path.display()
                ));
            }
            if meta.is_dir() {
                walk(root, &path, out, depth + 1)?;
            } else if meta.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(relative, hash_file(&path)?);
            } else {
                return Err(format!("Unsupported file type: {}", path.display()));
            }
        }
        Ok(())
    }
    let root = canonical(root)?;
    if !root.is_dir() {
        return Err("Skill root must be a directory".into());
    }
    let mut out = BTreeMap::new();
    walk(&root, &root, &mut out, 0)?;
    Ok(out)
}

pub(crate) fn read_text(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|e| err_path("Cannot inspect text file", path, e))?;
    if !meta.is_file() || meta.len() > MAX_TEXT_BYTES {
        return Err("Only text files up to 1 MiB can be previewed".into());
    }
    // Bound the read too, in case an editor grows the file after metadata().
    let file = fs::File::open(path).map_err(|e| err_path("Cannot open text file", path, e))?;
    let mut bytes = Vec::new();
    file.take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_TEXT_BYTES || bytes.contains(&0) {
        return Err("File is binary or exceeds the 1 MiB preview limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "File is not UTF-8 text".into())
}

fn frontmatter(content: &str) -> Result<Value, String> {
    let content = content.trim_start_matches('\u{feff}');
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Ok(json!({}));
    }
    let mut yaml = String::new();
    let mut ended = false;
    for line in lines {
        if matches!(line.trim(), "---" | "...") {
            ended = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !ended {
        return Err("SKILL.md has an unterminated YAML frontmatter".into());
    }
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(&yaml).map_err(|e| format!("Invalid SKILL.md frontmatter: {e}"))?;
    serde_json::to_value(parsed).map_err(|e| format!("Unsupported SKILL.md frontmatter: {e}"))
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub(crate) fn managed_path(root: &Path) -> bool {
    let parts: Vec<_> = root
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    parts.iter().any(|p| {
        p == ".system" || p == "node_modules" || p == "program files" || p == "program files (x86)"
    }) || parts
        .windows(2)
        .any(|p| p[0] == "plugins" && matches!(p[1].as_str(), "cache" | "marketplaces"))
}

pub(crate) fn readonly_reason(root: &Path) -> Option<String> {
    if managed_path(root) {
        return Some(
            "System, application, or plugin-managed skill; inspect without changing its files"
                .into(),
        );
    }
    if fs::metadata(root)
        .map(|m| m.permissions().readonly())
        .unwrap_or(false)
        || fs::metadata(root.join("SKILL.md"))
            .map(|m| m.permissions().readonly())
            .unwrap_or(false)
    {
        return Some("Folder or SKILL.md is marked read-only".into());
    }
    None
}

/// A registration draft deliberately contains no invented version or baseline.
pub fn draft(root: &Path) -> Result<Value, String> {
    let original_managed = managed_path(root);
    let original_reason = readonly_reason(root);
    let root = canonical(root)?;
    let skill_file = safe_join(&root, "SKILL.md")?;
    if !skill_file.is_file() {
        return Err(format!("No SKILL.md found in {}", root.display()));
    }
    let meta = frontmatter(&read_text(&skill_file)?)?;
    let hashes = files(&root)?;
    let fallback = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let name = scalar(&meta["name"]).unwrap_or(fallback);
    let interface_path = safe_join(&root, "agents/openai.yaml")?;
    let interface: Value = if interface_path.is_file() {
        serde_yaml::from_str(&read_text(&interface_path)?).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let display = scalar(&meta["displayName"])
        .or_else(|| scalar(&meta["metadata"]["displayName"]))
        .or_else(|| scalar(&meta["metadata"]["display_name"]))
        .or_else(|| scalar(&interface["interface"]["display_name"]))
        .unwrap_or_else(|| name.clone());
    let version = scalar(&meta["version"]).or_else(|| scalar(&meta["metadata"]["version"]));
    let reason = original_reason.or_else(|| readonly_reason(&root));
    let runtime = runtime_file_selection(&root, &hashes, None)?;
    Ok(json!({
        "name":name,"displayName":display,"description":scalar(&meta["description"]).unwrap_or_default(),
        "version":version,"localPath":readable_path(&root),"readOnly":reason.is_some(),"readOnlyReason":reason,
        "dependencies":meta.get("compatibility").cloned().unwrap_or(Value::Null),
        "managed":original_managed || managed_path(&root),
        "sourceId":null,"remotePath":null,"repo":null,"branch":null,"commit":null,"baselineCommit":null,
        "baselineHashes":{},"localHashes":hashes,"runtimeFiles":runtime.suggested_files,"runtimeConfirmed":false,
        "runtimeWarnings":runtime.warnings,
        "runtimeIssues":runtime.issues,
        "status":"local_only","lastChecked":now(),"lastLocalCheckedAt":now(),"error":null,"private":false
    }))
}

fn scan(root: &Path) -> Result<Value, String> {
    fn visit(
        root: &Path,
        depth: usize,
        result: &mut Vec<Value>,
        warnings: &mut Vec<String>,
        seen: &mut BTreeSet<String>,
    ) {
        if depth > MAX_SCAN_DEPTH {
            warnings.push(format!("Skipped deeply nested folder: {}", root.display()));
            return;
        }
        if root.join("SKILL.md").is_file() {
            match draft(root) {
                Ok(value) => {
                    let key = path_key(Path::new(value["localPath"].as_str().unwrap_or_default()));
                    if seen.insert(key) {
                        result.push(value);
                    }
                }
                Err(e) => warnings.push(e),
            }
            return;
        }
        let entries = match fs::read_dir(root) {
            Ok(e) => e,
            Err(e) => {
                warnings.push(err_path("Cannot scan folder", root, e));
                return;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(e.to_string());
                    continue;
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_artifact(&name) {
                continue;
            }
            let path = entry.path();
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    warnings.push(err_path("Cannot inspect path", &path, e));
                    continue;
                }
            };
            if is_link(&meta) {
                warnings.push(format!(
                    "Skipped nested link; select it as a root to inspect explicitly: {}",
                    path.display()
                ));
                continue;
            }
            if meta.is_dir() {
                visit(&path, depth + 1, result, warnings, seen);
            }
        }
    }
    let root = canonical(root)?;
    if !root.is_dir() {
        return Err("Choose a folder to scan".into());
    }
    let mut values = vec![];
    let mut warnings = vec![];
    visit(&root, 0, &mut values, &mut warnings, &mut BTreeSet::new());
    values.sort_by_key(|v| v["localPath"].as_str().unwrap_or_default().to_lowercase());
    Ok(json!({"skills":values,"warnings":warnings}))
}

fn required_string<'a>(req: &'a Value, field: &str) -> Result<&'a str, String> {
    req[field]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("Missing {field}"))
}

fn string_list(req: &Value, field: &str) -> Result<Vec<String>, String> {
    req[field]
        .as_array()
        .ok_or_else(|| format!("{field} must be a list"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("Every {field} entry must be text"))
        })
        .collect()
}

fn local_root(skill: &Value) -> Result<PathBuf, String> {
    let path = skill["localPath"]
        .as_str()
        .ok_or("Skill has no local runtime folder")?;
    let root = canonical(Path::new(path))?;
    if path_key(&root) != path_key(Path::new(path)) {
        return Err("Registered runtime folder target changed; scan and register its actual location explicitly".into());
    }
    Ok(root)
}

pub(crate) fn license_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or("");
    matches!(
        stem,
        "license" | "licence" | "copying" | "copyright" | "notice"
    ) || name.starts_with("license-")
        || name.starts_with("licence-")
}

fn management_document(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    matches!(
        name.split('.').next().unwrap_or(""),
        "readme"
            | "changelog"
            | "changes"
            | "history"
            | "contributing"
            | "code_of_conduct"
            | "security"
    )
}

pub(crate) fn base_required_reasons(all: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    all.keys()
        .filter_map(|path| {
            let reason = if path == "SKILL.md" {
                "Skill 入口与执行规则"
            } else if license_path(path) {
                "许可证与来源告知"
            } else if path.starts_with("agents/") && !management_document(path) {
                "客户端调用配置及资源"
            } else {
                return None;
            };
            Some((path.clone(), reason.to_owned()))
        })
        .collect()
}

fn suggested_runtime_resource(path: &str) -> bool {
    if management_document(path) {
        return false;
    }
    ["scripts/", "references/", "assets/", "agents/"]
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || (!path.contains('/')
            && (script_file(path)
                || matches!(
                    path.to_ascii_lowercase().as_str(),
                    "requirements.txt"
                        | "pyproject.toml"
                        | "uv.lock"
                        | "poetry.lock"
                        | "pipfile"
                        | "pipfile.lock"
                        | "package.json"
                        | "package-lock.json"
                        | "pnpm-lock.yaml"
                        | "yarn.lock"
                        | "bun.lock"
                        | "cargo.toml"
                        | "cargo.lock"
                )))
}

fn without_fenced_examples(text: &str) -> String {
    let mut visible = String::new();
    let mut fence: Option<(char, usize, bool)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next();
        let length = marker
            .map(|c| trimmed.chars().take_while(|x| *x == c).count())
            .unwrap_or(0);
        if let Some((character, opened_length, commands)) = fence {
            if marker == Some(character) && length >= opened_length {
                fence = None;
            } else if commands {
                visible.push_str(line);
            }
            visible.push('\n');
            continue;
        }
        if matches!(marker, Some('`' | '~')) && length >= 3 {
            let language = trimmed[length..]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let commands = [
                "bash",
                "sh",
                "shell",
                "powershell",
                "ps1",
                "python",
                "py",
                "javascript",
                "js",
                "typescript",
                "ts",
            ]
            .contains(&language.as_str());
            fence = Some((marker.unwrap(), length, commands));
            visible.push('\n');
            continue;
        }
        visible.push_str(line);
        visible.push('\n');
    }
    visible
}

/// Static package evidence is separate from environment and invocation checks.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuntimeIssue {
    pub category: String,
    pub severity: String,
    pub source: String,
    pub target: String,
    pub reason: String,
}

impl RuntimeIssue {
    fn new(category: &str, severity: &str, source: &str, target: &str, reason: &str) -> Self {
        Self {
            category: category.into(),
            severity: severity.into(),
            source: source.into(),
            target: target.into(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DependencyToken {
    target: String,
    relative: bool,
    historical: bool,
    example: bool,
    conditional: bool,
    generated: bool,
}

fn absolute_reference(target: &str) -> bool {
    let bytes = target.as_bytes();
    (target.starts_with('/') && !target.starts_with("//") && target.trim_matches('/').len() > 1)
        || (target.starts_with("\\\\")
            && target[2..]
                .split('\\')
                .filter(|part| !part.is_empty())
                .count()
                >= 2
            && !target.contains(['+', '[', ']']))
        || (target.starts_with("~/") && target.len() > 2)
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

fn client_command_reference(target: &str, context: &str) -> bool {
    let Some(name) = target.strip_prefix('/') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        && ["客户端", "菜单", "命令", "斜杠", "slash", "command"]
            .iter()
            .any(|word| context.to_lowercase().contains(word))
}

fn source_context_for_command(text: &str, target: &str) -> String {
    text.lines()
        .filter(|line| line.contains(target))
        .collect::<Vec<_>>()
        .join("\n")
}

// Quoted destinations are unambiguous and retain spaces/Chinese characters.
// Bare prose needs a narrower boundary: Chinese conjunctions may immediately
// follow a completed filename, but must not split Chinese filenames themselves.
fn prose_path_segments<'a>(value: &'a str, all: &BTreeMap<String, String>) -> Vec<&'a str> {
    let mut result = Vec::new();
    let mut rest = value;
    while !rest.is_empty() {
        let known = all
            .keys()
            .filter(|path| rest.starts_with(path.as_str()))
            .max_by_key(|path| path.len())
            .filter(|path| {
                rest[path.len()..].chars().next().is_none_or(|next| {
                    !next.is_ascii_alphanumeric() && !matches!(next, '/' | '\\' | '.' | '_' | '-')
                })
            });
        let boundary = known.map(|path| path.len()).or_else(|| {
            rest.char_indices().find_map(|(position, character)| {
                if character != '.' {
                    return None;
                }
                [
                    "md", "mdx", "tex", "txt", "rst", "yaml", "yml", "json", "toml", "py", "ps1",
                    "sh", "js", "ts", "tsx", "jsx", "html", "css", "svg", "png", "jpg", "jpeg",
                    "pdf", "csv", "xml", "patch", "ini", "cfg",
                ]
                .iter()
                .filter_map(|extension| {
                    let end = position + 1 + extension.len();
                    if !rest[position + 1..].starts_with(extension) || !rest.is_char_boundary(end) {
                        return None;
                    }
                    let tail = &rest[end..];
                    [
                        "和", "与", "及", "或", "以", "为", "是", "用于", "作为", "需要",
                    ]
                    .iter()
                    .any(|word| tail.starts_with(word))
                    .then_some(end)
                })
                .max()
            })
        });
        let Some(end) = boundary else {
            result.push(rest);
            break;
        };
        result.push(&rest[..end]);
        rest = &rest[end..];
        // Continue only at another explicit package prefix. Remaining prose
        // such as “以@@...标记” is not a second path.
        let next = ["scripts/", "references/", "assets/", "agents/"]
            .iter()
            .filter_map(|prefix| rest.find(prefix))
            .min();
        match next {
            Some(position) => rest = &rest[position..],
            None => break,
        }
    }
    result
}

fn dependency_tokens(
    text: &str,
    markdown: bool,
    all: &BTreeMap<String, String>,
) -> Vec<DependencyToken> {
    let visible = without_fenced_examples(text);
    let mut found = BTreeSet::new();
    let mut historical_section: Option<usize> = None;
    let mut spaced_paths: Vec<_> = all
        .keys()
        .filter(|path| {
            path.chars()
                .any(|c| c.is_whitespace() || "()[]".contains(c))
        })
        .collect();
    spaced_paths.sort_by_key(|path| std::cmp::Reverse(path.len()));
    for line in visible.lines() {
        let trimmed = line.trim_start();
        let heading = trimmed.chars().take_while(|c| *c == '#').count();
        if heading > 0 {
            if historical_section.is_some_and(|level| heading <= level) {
                historical_section = None;
            }
            if [
                "历史",
                "来源",
                "借鉴",
                "审查清单",
                "本版读取的本地依据",
                "history",
                "sources",
                "provenance",
            ]
            .iter()
            .any(|word| trimmed.to_lowercase().contains(word))
            {
                historical_section = Some(heading);
            }
        }
        let explicit_requirement = [
            "必须读取",
            "执行前读取",
            "运行时读取",
            "先读取",
            "当前规则",
            "现行规则",
            "全局维护规则",
            "required at runtime",
            "must read",
        ]
        .iter()
        .any(|word| line.to_lowercase().contains(word));
        let historical = !explicit_requirement
            && (historical_section.is_some()
                || [
                    "历史来源",
                    "历史索引",
                    "证据索引",
                    "不要求",
                    "不依赖此",
                    "historical",
                    "provenance",
                ]
                .iter()
                .any(|word| line.to_lowercase().contains(word)));
        let example = ["示例", "例如", "实例", "example", "illustration"]
            .iter()
            .any(|word| line.to_lowercase().contains(word));
        let conditional = ["按需", "需要时", "若", "如果", "仅在", "when needed", "if "]
            .iter()
            .any(|word| line.to_lowercase().contains(word));
        let generated = [
            "creates",
            "create ",
            "generated",
            "生成",
            "创建",
            "写入",
            "输出文件",
            "运行目录",
            "run folder",
        ]
        .iter()
        .any(|word| line.to_lowercase().contains(word));
        let mut add = |target: String, relative: bool| {
            found.insert(DependencyToken {
                target,
                relative,
                historical,
                example,
                conditional,
                generated,
            });
        };
        // Markdown destinations (including relative documents outside the usual
        // scripts/references/assets folders). URLs and same-document anchors stay
        // remote documentation rather than runtime dependencies.
        let mut masked = line.to_owned();
        let mut remaining = if markdown { line } else { "" };
        while let Some(i) = remaining.find("](") {
            remaining = &remaining[i + 2..];
            let Some(end) = markdown_destination_end(remaining) else {
                break;
            };
            let destination = remaining[..end].trim();
            let destination = if destination.starts_with('<') {
                destination
                    .trim_start_matches('<')
                    .split('>')
                    .next()
                    .unwrap_or("")
            } else {
                destination.split(" \"").next().unwrap_or("")
            };
            let destination = destination.split(['#', '?']).next().unwrap_or("");
            if !destination.is_empty()
                && !destination.contains("://")
                && !destination.starts_with('#')
                && !destination.starts_with("mailto:")
                && !destination.starts_with("data:")
            {
                add(
                    destination
                        .strip_prefix("./")
                        .unwrap_or(destination)
                        .to_owned(),
                    true,
                );
                masked = masked.replace(&remaining[..end], " ");
            }
            remaining = &remaining[end + 1..];
        }
        // Preserve explicit code/quoted filenames as one token, then mask them
        // so the fallback cannot emit misleading fragments of a spaced path.
        for quote in ['`', '"', '\''] {
            let mut offset = 0;
            while let Some(start) = masked[offset..].find(quote).map(|i| offset + i) {
                let body = start + quote.len_utf8();
                let Some(end) = masked[body..].find(quote).map(|i| body + i) else {
                    break;
                };
                let value = masked[body..end].trim().trim_start_matches("./");
                if absolute_reference(value)
                    || ["scripts/", "references/", "assets/", "agents/"]
                        .iter()
                        .any(|prefix| value.starts_with(prefix))
                {
                    add(value.to_owned(), false);
                    masked.replace_range(start..end + quote.len_utf8(), " ");
                    offset = start + 1;
                } else {
                    offset = end + quote.len_utf8();
                }
            }
        }
        // Existing bare filenames with spaces/punctuation are still exact
        // evidence. Match the longest actual package path before fallback
        // tokenisation; never guess a missing filename from whitespace.
        for path in &spaced_paths {
            let positions: Vec<_> = masked
                .match_indices(path.as_str())
                .filter_map(|(index, _)| {
                    let before = masked[..index].chars().next_back();
                    let after = masked[index + path.len()..].chars().next();
                    (!before.is_some_and(|c| c.is_ascii_alphanumeric() || "/\\_-".contains(c))
                        && !after
                            .is_some_and(|c| c.is_ascii_alphanumeric() || "/\\_.-".contains(c)))
                    .then_some(index)
                })
                .collect();
            for index in positions.into_iter().rev() {
                add((*path).clone(), false);
                masked.replace_range(index..index + path.len(), " ");
            }
        }
        let normalized_line = masked
            .replace("${SKILL_DIR}/", "")
            .replace("${SKILL_ROOT}/", "")
            .replace("{baseDir}/", "");
        let command = [
            "python ",
            "python3 ",
            "node ",
            "pwsh ",
            "powershell ",
            "bash ",
            "sh ",
        ]
        .iter()
        .any(|command| normalized_line.trim_start().starts_with(command));
        for token in normalized_line
            .split(|c: char| c.is_whitespace() || "`\"'<>[](){}=,;，；。！？、：".contains(c))
        {
            let token = token.trim_end_matches(['.', ':', '。', '，', '；']);
            let token = ["$SKILL_DIR/", "$SKILL_ROOT/"]
                .iter()
                .find_map(|prefix| token.strip_prefix(prefix))
                .unwrap_or(token);
            let token = token.strip_prefix("./").unwrap_or(token);
            if (["scripts/", "references/", "assets/", "agents/"]
                .iter()
                .any(|p| token.starts_with(p) && token.len() > p.len())
                || absolute_reference(token)
                || (command && token.contains('.') && script_file(token)))
                && !token.contains(['*', '$'])
            {
                for path in prose_path_segments(token, all) {
                    add(path.to_owned(), false);
                }
            }
        }
    }
    found.into_iter().collect()
}

fn markdown_destination_end(value: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut angle = false;
    for (index, character) in value.char_indices() {
        match character {
            '<' if index == 0 => angle = true,
            '>' if angle => angle = false,
            '(' if !angle => depth += 1,
            ')' if !angle && depth == 0 => return Some(index),
            ')' if !angle => depth -= 1,
            _ => {}
        }
    }
    None
}

fn reference_path(parent: &str, target: &str) -> Option<String> {
    if target.starts_with('/') || target.contains(['\\', ':']) {
        return None;
    }
    let mut parts: Vec<_> = parent
        .split('/')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ if valid_part(part) => parts.push(part.to_owned()),
            _ => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn script_file(path: &str) -> bool {
    matches!(
        path.rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "py" | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "mjs"
            | "cjs"
            | "ps1"
            | "sh"
            | "bash"
            | "rb"
            | "pl"
            | "r"
            | "rs"
    )
}

fn dependency_text_file(path: &str) -> bool {
    script_file(path)
        || matches!(
            path.rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "md" | "mdx"
                | "txt"
                | "rst"
                | "json"
                | "yaml"
                | "yml"
                | "toml"
                | "html"
                | "css"
                | "svg"
        )
}

fn resource_path_literal(value: &str) -> bool {
    if value.is_empty()
        || value.trim_matches(['/', '\\', '.']).is_empty()
        || value.contains(['\n', '\r'])
        || value.starts_with("s/")
        || value.starts_with(". ")
        || (value.starts_with('.') && !value.contains('/') && value.matches('.').count() == 1)
    {
        return false;
    }
    value.contains('/')
        || (value.contains('.')
            && matches!(
                value
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
                "py" | "js"
                    | "jsx"
                    | "ts"
                    | "tsx"
                    | "mjs"
                    | "cjs"
                    | "json"
                    | "yaml"
                    | "yml"
                    | "toml"
                    | "csv"
                    | "tsv"
                    | "xml"
                    | "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "svg"
                    | "pdf"
                    | "txt"
                    | "md"
                    | "bin"
                    | "dat"
                    | "ttf"
                    | "otf"
                    | "woff"
                    | "html"
                    | "css"
                    | "npy"
                    | "pkl"
                    | "pickle"
                    | "xlsx"
                    | "docx"
                    | "sh"
                    | "ps1"
                    | "so"
                    | "dll"
                    | "exe"
                    | "wasm"
            ))
}

/// Static code paths are document-relative, unlike prose's package-relative
/// instruction tokens. Dynamic paths and external modules remain unverified.
fn code_path_tokens(text: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    for line in text.lines() {
        let mut quote = None;
        let mut value = String::new();
        for character in line.chars() {
            if let Some(opened) = quote {
                if character == opened {
                    if (resource_path_literal(&value)
                        || ["scripts", "assets", "references", "agents"].contains(&value.as_str()))
                        && (absolute_reference(&value) || !value.contains([':', '\\']))
                        && !value.contains(['{', '}', '$', '*', '\0'])
                        && !value.chars().any(char::is_whitespace)
                        && !value.starts_with('#')
                    {
                        found.insert(value.clone());
                    }
                    value.clear();
                    quote = None;
                } else {
                    value.push(character);
                }
            } else if matches!(character, '\'' | '"' | '`') {
                quote = Some(character);
            }
        }
    }
    found.into_iter().collect()
}

fn has_dynamic_runtime_read(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        if line.starts_with('#') || line.starts_with("//") {
            return false;
        }
        if line.contains("importlib.import_module(") || line.contains("__import__(") {
            return true;
        }
        [
            "open(",
            "readFile(",
            "readFileSync(",
            "read_file(",
            "read_text(",
            "read_bytes(",
        ]
        .iter()
        .any(|call| {
            let Some((_, arguments)) = line.split_once(call) else {
                return false;
            };
            let arguments = arguments.trim_start();
            // An explicit write is not an input dependency.
            if output_operation(line) {
                return false;
            }
            if let Some(quote) = arguments.chars().next().filter(|c| matches!(c, '\'' | '"')) {
                if let Some(end) = arguments[1..].find(quote) {
                    return !arguments[end + 2..].trim_start().starts_with([',', ')']);
                }
            }
            // A preceding literal Path(...).read_text() is already inspected;
            // a quoted suffix inside a variable expression is not a full path.
            let prefix = line
                .split_once(call)
                .map(|(prefix, _)| prefix)
                .unwrap_or("");
            !code_path_tokens(prefix)
                .iter()
                .any(|path| resource_path_literal(path))
        })
    })
}

fn output_operation(line: &str) -> bool {
    [
        ".write_text(",
        ".write_bytes(",
        "writeFile",
        ".save(",
        "Set-Content",
        "Out-File",
        "json.dump(",
    ]
    .iter()
    .any(|part| line.contains(part))
        || ["'w", "\"w", "'x", "\"x", "'a", "\"a"].iter().any(|mode| {
            line.contains(&format!("open({mode}"))
                || line.contains(&format!(", {mode}"))
                || line.contains(&format!(",{mode}"))
        })
        || line.contains("os.O_WRONLY")
        || line.contains("os.O_CREAT")
}

fn code_read_operation(line: &str) -> bool {
    [
        "open(",
        "read_text(",
        "read_bytes(",
        "readFile",
        "read_file(",
        "ReadAll",
        "Get-Content",
        "Import-Module",
        "include_str!(",
        "include_bytes!(",
        "source ",
    ]
    .iter()
    .any(|part| line.contains(part))
}

fn external_script_inputs(text: &str) -> bool {
    [
        "argparse",
        "sys.argv",
        "process.argv",
        "os.getcwd(",
        "Path.cwd(",
        "Get-Location",
        "param(",
        "param (",
        "$args",
        "$PWD",
        "$(pwd)",
        "${1",
        "$1",
        "sys.stdin",
        "os.environ",
        "tempfile.TemporaryDirectory(",
        "tempfile.mkdtemp(",
        "tmp_path",
    ]
    .iter()
    .any(|part| text.contains(part))
}

fn mentions_variable(line: &str, variable: &str) -> bool {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|word| word == variable)
}

/// Only explicit reads rooted at the script (or explicit ../ package paths)
/// are package references. Arbitrary string literals, suffixes, regexes and
/// CLI/workspace paths are not evidence of a missing packaged file.
fn script_reference_tokens(text: &str) -> (Vec<DependencyToken>, bool) {
    let mut anchors = BTreeSet::from(["PSScriptRoot".to_owned()]);
    for _ in 0..3 {
        for line in text.lines() {
            if let Some((name, expression)) = line.split_once('=') {
                let name = name.trim().trim_start_matches('$');
                if !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && (expression.contains("__file__")
                        || expression.contains("BASH_SOURCE")
                        || (expression.contains("dirname") && expression.contains("$0"))
                        || (!expression.contains("$(")
                            && [" / ", " + ", "/", "Join-Path", "os.path.join", "Path("]
                                .iter()
                                .any(|part| expression.contains(part))
                            && anchors
                                .iter()
                                .any(|anchor| mentions_variable(expression, anchor))))
                {
                    anchors.insert(name.to_owned());
                }
            }
        }
    }
    let cli = external_script_inputs(text);
    let mut found = BTreeSet::new();
    let mut dynamic_package = false;
    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with('#') || line.starts_with("//") || output_operation(line) {
            continue;
        }
        let anchored = line.contains("__file__")
            || anchors.iter().any(|anchor| mentions_variable(line, anchor));
        let read = code_read_operation(line);
        if anchored && read && has_dynamic_runtime_read(line) && code_path_tokens(line).is_empty() {
            dynamic_package = true;
        }
        if !anchored && !read {
            continue;
        }
        let mut expanded = line.to_owned();
        for anchor in &anchors {
            expanded = expanded
                .replace(&format!("${{{anchor}}}/"), "")
                .replace(&format!("${anchor}/"), "");
        }
        for mut target in code_path_tokens(&expanded) {
            if anchored && (line.contains(" + ") || line.contains("+\"") || line.contains("+'")) {
                target = target.strip_prefix('/').unwrap_or(&target).to_owned();
            }
            let package = anchored
                || (!cli
                    && (target.starts_with("../")
                        || target.starts_with("./")
                        || ["scripts/", "assets/", "references/", "agents/"]
                            .iter()
                            .any(|prefix| target.starts_with(prefix))));
            found.insert(DependencyToken {
                target,
                relative: true,
                historical: false,
                example: false,
                conditional: !package,
                generated: false,
            });
        }
    }
    (found.into_iter().collect(), dynamic_package)
}

fn configuration_reference_tokens(text: &str) -> Vec<DependencyToken> {
    let mut targets: BTreeSet<String> = code_path_tokens(text).into_iter().collect();
    for token in text.split(|c: char| c.is_whitespace() || "`\"'<>[](){},;".contains(c)) {
        let token = token.strip_prefix("./").unwrap_or(token);
        if ["scripts/", "assets/", "references/", "agents/"]
            .iter()
            .any(|prefix| token.starts_with(prefix))
            && !token.contains(['$', '*', '{', '}'])
        {
            targets.insert(token.to_owned());
        }
    }
    targets
        .into_iter()
        .filter(|target| {
            ["scripts/", "assets/", "references/", "agents/", "../", "./"]
                .iter()
                .any(|prefix| target.starts_with(prefix))
        })
        .map(|target| DependencyToken {
            target,
            relative: true,
            historical: false,
            example: false,
            conditional: false,
            generated: false,
        })
        .collect()
}

fn python_local_imports(
    path: &str,
    text: &str,
    all: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    let mut found = BTreeSet::new();
    let mut modules = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        if let Some(imports) = line.strip_prefix("import ") {
            modules.extend(
                imports
                    .split(',')
                    .filter_map(|module| module.split_whitespace().next())
                    .map(str::to_owned),
            );
        } else if let Some(import) = line.strip_prefix("from ") {
            if let Some((module, members)) = import.split_once(" import ") {
                let module = module.trim();
                modules.push(module.to_owned());
                for member in members.trim_matches(['(', ')']).split(',') {
                    if let Some(member) =
                        member.split_whitespace().next().filter(|name| *name != "*")
                    {
                        modules.push(format!(
                            "{module}{}{member}",
                            if module.ends_with('.') { "" } else { "." }
                        ));
                    }
                }
            }
        }
    }
    for module in modules {
        let relative_depth = module.chars().take_while(|c| *c == '.').count();
        let tail = module.trim_start_matches('.');
        if !tail
            .split('.')
            .all(|part| part.chars().all(|c| c.is_alphanumeric() || c == '_'))
        {
            continue;
        }
        let prefix = "../".repeat(relative_depth.saturating_sub(1));
        let target = format!("{prefix}{}", tail.replace('.', "/"));
        let roots = if relative_depth > 0 {
            vec![reference_path(parent, &target)]
        } else {
            vec![reference_path(parent, &target), reference_path("", &target)]
        };
        for target in roots.into_iter().flatten() {
            let candidates = [format!("{target}.py"), format!("{target}/__init__.py")];
            if let Some(candidate) = candidates
                .iter()
                .find(|candidate| all.contains_key(*candidate))
            {
                found.insert(candidate.clone());
                let mut package = candidate.rsplit_once('/').map(|(parent, _)| parent);
                while let Some(parent) = package {
                    let initializer = format!("{parent}/__init__.py");
                    if all.contains_key(&initializer) {
                        found.insert(initializer);
                    }
                    package = parent.rsplit_once('/').map(|(parent, _)| parent);
                }
                break;
            }
        }
    }
    found
}

// Split an explicit script invocation only at a script filename followed by
// quoted arguments or an option. Exact filenames (including spaces) win first.
fn script_command_entry(target: &str, all: &BTreeMap<String, String>) -> Option<String> {
    if all.contains_key(target) {
        return None;
    }
    let arguments = |rest: &str| {
        let trimmed = rest.trim_start();
        trimmed.len() < rest.len()
            && (matches!(trimmed.chars().next(), Some('\'' | '"')) || trimmed.starts_with('-'))
    };
    let known = all
        .keys()
        .filter(|path| script_file(path))
        .filter(|path| target.strip_prefix(path.as_str()).is_some_and(arguments))
        .max_by_key(|path| path.len());
    if let Some(path) = known {
        return Some(path.clone());
    }
    let boundary = target.find(char::is_whitespace)?;
    let entry = &target[..boundary];
    (script_file(entry)
        && ["scripts/", "./scripts/"].iter().any(|prefix| entry.starts_with(prefix))
        && arguments(&target[boundary..]))
    .then(|| entry.to_owned())
}

fn dependency_closure_with_reader(
    all: &BTreeMap<String, String>,
    selected: &BTreeSet<String>,
    mut read: impl FnMut(&str) -> Result<String, String>,
) -> Result<(BTreeMap<String, String>, Vec<RuntimeIssue>), String> {
    let mut required = BTreeMap::new();
    let mut issues = BTreeSet::new();
    let mut queue = selected.clone();
    let mut inspected = BTreeSet::new();
    while let Some(path) = queue.pop_first() {
        if !all.contains_key(&path) || !inspected.insert(path.clone()) {
            continue;
        }
        if inspected.len() > 2000 {
            return Err("Runtime dependency inspection exceeds the 2000-file limit".into());
        }
        if !dependency_text_file(&path) {
            continue;
        }
        let text = match read(&path).and_then(|text| {
            if text.len() as u64 > MAX_TEXT_BYTES || text.contains('\0') {
                Err(
                    "Dependency inspection requires UTF-8 text up to 1 MiB without NUL bytes"
                        .into(),
                )
            } else {
                Ok(text)
            }
        }) {
            Ok(text) => text,
            Err(error) if path == "SKILL.md" => {
                return Err(format!(
                    "Could not inspect the Skill entry SKILL.md: {error}"
                ));
            }
            Err(e) => {
                issues.insert(RuntimeIssue::new(
                    "uncertain",
                    "warning",
                    &path,
                    &path,
                    &format!("无法检查此文件的引用：{e}"),
                ));
                continue;
            }
        };
        let parent = path
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("");
        let prose = matches!(
            path.rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "md" | "mdx" | "rst" | "txt"
        );
        let markdown = path.ends_with(".md") || path.ends_with(".mdx");
        let (tokens, dynamic_package) = if prose {
            (dependency_tokens(&text, markdown, all), false)
        } else if script_file(&path) {
            script_reference_tokens(&text)
        } else {
            (configuration_reference_tokens(&text), false)
        };
        if script_file(&path) && has_dynamic_runtime_read(&text) {
            if external_script_inputs(&text)
                && !dynamic_package
                && !text.contains("importlib.import_module(")
                && !text.contains("__import__(")
            {
                issues.insert(RuntimeIssue::new("external", "info", &path, "运行时输入", "脚本从命令行、工作目录或运行环境读取输入；不属于缺失的包内文件，实际调用尚未验证"));
            } else {
                issues.insert(RuntimeIssue::new("uncertain", "warning", &path, "动态读取", "脚本包含无法静态确定目标的读取或加载；需确认输入来自用户、外部环境还是包内资源"));
            }
        }
        let generated: Vec<_> = tokens
            .iter()
            .filter(|token| token.generated)
            .map(|token| token.target.trim_end_matches('/').to_owned())
            .collect();
        for token in tokens {
            let target = if let Some(entry) = script_command_entry(&token.target, all) {
                issues.insert(RuntimeIssue::new(
                    "command",
                    "info",
                    &path,
                    &token.target,
                    "脚本调用的工作目录或参数相对路径已与入口分开核对；参数不是缺失的静态资源",
                ));
                entry
            } else {
                token.target
            };
            let markdown = token.relative || !prose;
            if client_command_reference(&target, &source_context_for_command(&text, &target)) {
                issues.insert(RuntimeIssue::new(
                    "command",
                    "info",
                    &path,
                    &target,
                    "客户端斜杠命令，不是文件路径，无需绑定或打包",
                ));
                continue;
            }
            if absolute_reference(&target) {
                let (category, severity, reason) = if token.historical || token.example {
                    (
                        "history",
                        "info",
                        "来源、历史或示例中的外部路径；不计为包内缺失文件，也不随包复制",
                    )
                } else if token.conditional {
                    (
                        "external",
                        "info",
                        "按条件使用的外部文件或配置；是否可用需在对应环境核验，不随包复制",
                    )
                } else {
                    (
                        "external",
                        "warning",
                        "当前规则引用的外部文件或配置；不属于包内文件，尚未验证运行时是否可用",
                    )
                };
                issues.insert(RuntimeIssue::new(
                    category, severity, &path, &target, reason,
                ));
                continue;
            }
            if !prose && token.conditional {
                issues.insert(RuntimeIssue::new(
                    "external",
                    "info",
                    &path,
                    &target,
                    "脚本读取的工作目录或参数相对路径；不视为缺失的包内文件，运行时按输入核验",
                ));
                continue;
            }
            let document_relative = reference_path(parent, &target);
            let root_relative = reference_path("", &target);
            let candidates = if markdown {
                [document_relative, root_relative]
            } else {
                [root_relative, document_relative]
            };
            let exists = |candidate: &str| {
                all.contains_key(candidate)
                    || all.keys().any(|p| p.starts_with(&format!("{candidate}/")))
            };
            let direct = candidates.iter().flatten().find(|p| exists(p));
            // A self-anchored expression may join a separately declared scripts
            // directory. Preserve its uniquely identifiable package file.
            let suffix_matches: Vec<_> = if direct.is_none() && !prose && !token.conditional {
                all.keys()
                    .filter(|path| path.ends_with(&format!("/{target}")))
                    .collect()
            } else {
                vec![]
            };
            let resolved =
                direct.or_else(|| (suffix_matches.len() == 1).then(|| suffix_matches[0]));
            if let Some(resolved) = resolved {
                let dependencies = if all.contains_key(resolved) {
                    vec![resolved.to_string()]
                } else {
                    let prefix = format!("{resolved}/");
                    all.keys()
                        .filter(|p| p.starts_with(&prefix))
                        .cloned()
                        .collect()
                };
                for dependency in dependencies {
                    required
                        .entry(dependency.clone())
                        .or_insert_with(|| format!("被 {path} 引用"));
                    if !inspected.contains(&dependency) {
                        queue.insert(dependency);
                    }
                }
            } else {
                let package_path = ["scripts/", "references/", "assets/", "agents/"]
                    .iter()
                    .any(|prefix| target.starts_with(prefix))
                    || (target.contains('.') && script_file(&target))
                    || (!prose && resource_path_literal(&target));
                let generated_target = token.generated
                    || generated.iter().any(|prefix| {
                        target == *prefix || target.starts_with(&format!("{prefix}/"))
                    });
                let outside_package =
                    target.starts_with("../") && reference_path(parent, &target).is_none();
                let (category, severity, reason) = if token.historical || token.example {
                    (
                        "history",
                        "info",
                        "历史、来源或示例引用未解析；未视为已存在的运行依赖",
                    )
                } else if generated_target {
                    (
                        "external",
                        "info",
                        "文档说明在任务运行目录生成或创建的文件；不属于随 Skill 安装的静态资源",
                    )
                } else if outside_package {
                    (
                        "external",
                        "info",
                        "引用位于 Skill 包之外；可能是关联技能或工作目录输入，需在对应环境核验",
                    )
                } else if package_path && !token.conditional {
                    (
                        "package_required",
                        "error",
                        "明确引用的包内运行文件缺失，需补齐或核对引用",
                    )
                } else {
                    (
                        "uncertain",
                        "warning",
                        "此引用未能解析；需判断是否为实际运行依赖或条件输入",
                    )
                };
                issues.insert(RuntimeIssue::new(
                    category, severity, &path, &target, reason,
                ));
            }
        }
        if path.to_ascii_lowercase().ends_with(".py") {
            for dependency in python_local_imports(&path, &text, all) {
                required
                    .entry(dependency.clone())
                    .or_insert_with(|| format!("{path} 的包内脚本依赖"));
                if !inspected.contains(&dependency) {
                    queue.insert(dependency);
                }
            }
        }
    }
    Ok((required, issues.into_iter().collect()))
}

#[cfg(test)]
fn dependency_notes(
    root: &Path,
    selected: &BTreeSet<String>,
    all: &BTreeMap<String, String>,
) -> Result<(BTreeSet<String>, Vec<String>), String> {
    let (required, issues) =
        dependency_closure_with_reader(all, selected, |path| read_text(&safe_join(root, path)?))?;
    Ok((
        required.into_keys().collect(),
        issues.iter().map(issue_warning).collect(),
    ))
}

fn issue_warning(issue: &RuntimeIssue) -> String {
    format!("{}：{} → {}", issue.reason, issue.source, issue.target)
}

#[derive(Debug)]
pub struct RuntimeFileSelection {
    pub required_files: Vec<String>,
    pub optional_files: Vec<String>,
    pub suggested_files: Vec<String>,
    pub required_reasons: BTreeMap<String, String>,
    pub warnings: Vec<String>,
    pub issues: Vec<RuntimeIssue>,
}

/// Classify only actual package files. None chooses a minimal initial scope;
/// Some retains the user's confirmed scope, adding no optional files to it.
pub fn runtime_file_selection_with_reader(
    all: &BTreeMap<String, String>,
    selected: Option<&BTreeSet<String>>,
    read: impl FnMut(&str) -> Result<String, String>,
) -> Result<RuntimeFileSelection, String> {
    if !all.contains_key("SKILL.md") {
        return Err("Runtime files must include SKILL.md".into());
    }
    let mut required_reasons = base_required_reasons(all);
    let mut suggested: BTreeSet<_> = selected.cloned().unwrap_or_else(|| {
        all.keys()
            .filter(|path| suggested_runtime_resource(path))
            .cloned()
            .collect()
    });
    suggested.extend(required_reasons.keys().cloned());
    let (dependencies, mut issues) = dependency_closure_with_reader(all, &suggested, read)?;
    for (path, reason) in dependencies {
        required_reasons.entry(path).or_insert(reason);
    }
    for path in suggested.iter().filter(|path| !all.contains_key(*path)) {
        issues.push(RuntimeIssue::new(
            "package_required",
            "error",
            path,
            path,
            "已选运行文件缺失",
        ));
    }
    suggested.retain(|path| all.contains_key(path));
    suggested.extend(required_reasons.keys().cloned());
    let mut warnings: Vec<_> = issues.iter().map(issue_warning).collect();
    if suggested.iter().any(|path| script_file(path)) {
        warnings.push("已检查包内显式路径与可识别的本地 Python 导入；动态加载、生成文件和外部依赖仍需实际运行验证。".into());
    }
    Ok(RuntimeFileSelection {
        required_files: required_reasons.keys().cloned().collect(),
        optional_files: all
            .keys()
            .filter(|path| !required_reasons.contains_key(*path))
            .cloned()
            .collect(),
        suggested_files: suggested.into_iter().collect(),
        required_reasons,
        warnings,
        issues,
    })
}

/// A per-operation minimal package, independent of historical optional selections.
/// Uncertain references prevent destructive trimming; external configuration and
/// provenance remain visible but are never copied into the package.
pub fn minimal_runtime_files(root: &Path) -> Result<Vec<String>, String> {
    let all = files(root)?;
    minimal_runtime_files_with_inventory(root, &all)
}

/// Reuse only the hashes read within this operation; writers still scan afresh.
fn minimal_runtime_files_with_inventory(
    root: &Path,
    all: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let roots = base_required_reasons(all).into_keys().collect();
    let classification = runtime_file_selection(root, all, Some(&roots))?;
    if let Some(issue) = classification
        .issues
        .iter()
        .find(|issue| issue.severity == "error" || issue.category == "uncertain")
    {
        return Err(issue_warning(issue));
    }
    Ok(classification.required_files)
}

/// The manager's explicit replacement bindings are part of runtime evidence.
/// Generic format inspection above remains useful without a registered Skill.
pub(crate) fn minimal_runtime_for_skill(
    skill: &Value,
    root: &Path,
    all: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let roots = base_required_reasons(all).into_keys().collect();
    let mut classification = runtime_file_selection(root, all, Some(&roots))?;
    let external = crate::external::inspect_issues(skill, root, all, &classification.issues);
    crate::external::apply_resolutions(&mut classification, &external);
    if let Some(issue) = classification
        .issues
        .iter()
        .find(|issue| issue.severity == "error" || issue.category == "uncertain")
    {
        return Err(issue_warning(issue));
    }
    Ok(classification.required_files)
}

/// Old composite prose tokens are no longer active references. Keep their
/// original binding as history rather than silently discarding user evidence.
pub(crate) fn retire_obsolete_reference_bindings(
    skill: &mut Value,
    all: &BTreeMap<String, String>,
) {
    let Ok(root) = local_root(skill) else {
        return;
    };
    let Some(bindings) = skill["externalBindings"].as_array() else {
        return;
    };
    let mut active = Vec::new();
    let mut retired = Vec::new();
    for binding in bindings {
        let target = binding["target"].as_str().unwrap_or("");
        let source = binding["source"].as_str().unwrap_or("");
        let segments = prose_path_segments(target, all);
        let obsolete = !all.contains_key(target)
            && segments.len() > 1
            && segments.iter().all(|path| all.contains_key(*path))
            && safe_join(&root, source)
                .and_then(|path| read_text(&path))
                .is_ok_and(|text| text.contains(target));
        if obsolete {
            let mut entry = binding.clone();
            entry["retiredAt"] = json!(now());
            entry["reason"] = json!("旧分析器将连续中文说明误识别为一个路径；已按实际文件重新解析");
            retired.push(entry);
        } else {
            active.push(binding.clone());
        }
    }
    if !retired.is_empty() {
        let mut history = skill["externalBindingHistory"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        history.extend(retired);
        skill["externalBindings"] = json!(active);
        skill["externalBindingHistory"] = json!(history);
    }
}

/// Inspect a registered folder without changing its files or shrinking the
/// existing managed scope. The hashes bind this evidence to the bytes checked.
pub fn runtime_audit(skill: &Value) -> Result<Value, String> {
    let root = local_root(skill)?;
    let all = files(&root)?;
    runtime_audit_with_inventory(skill, &all)
}

pub fn runtime_audit_with_inventory(
    skill: &Value,
    all: &BTreeMap<String, String>,
) -> Result<Value, String> {
    let root = local_root(skill)?;
    let preserved: BTreeSet<String> = skill["runtimeFiles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|path| path.as_str().map(str::to_owned))
        .collect();
    let mut classification =
        runtime_file_selection(&root, all, (!preserved.is_empty()).then_some(&preserved))?;
    if files(&root)? != *all {
        return Err("核对期间文件内容发生变化，请重新检查；未保存过期的运行范围证据".into());
    }
    let raw_issues = classification.issues.clone();
    let external = crate::external::inspect_issues(skill, &root, all, &raw_issues);
    crate::external::apply_resolutions(&mut classification, &external);
    let complete = !classification
        .issues
        .iter()
        .any(|issue| issue.severity == "error" || issue.category == "uncertain");
    let scope: BTreeSet<_> = classification.suggested_files.iter().cloned().collect();
    let hashes: BTreeMap<_, _> = all
        .iter()
        .filter(|(path, _)| scope.contains(*path))
        .map(|(path, hash)| (path.clone(), hash.clone()))
        .collect();
    let optional_in_scope: Vec<_> = classification
        .suggested_files
        .iter()
        .filter(|path| !classification.required_files.contains(path))
        .cloned()
        .collect();
    let external_blocked = external
        .iter()
        .any(|item| item.required && item.status != "available");
    Ok(
        json!({"status":if complete && !external_blocked {"passed"} else {"needs_review"},"checkedAt":now(),"method":"static_package",
        "analyzerVersion":RUNTIME_ANALYZER_VERSION,
        "requiredFiles":classification.required_files,"preservedScope":preserved,"proposedScope":classification.suggested_files,
        "optionalInScope":optional_in_scope,"scopeHashes":hashes,"inventoryHashes":all,"issues":classification.issues,"rawIssues":raw_issues,"warnings":classification.warnings,
        "canAutoConfirm":complete,"externalStatus":if external_blocked {"missing_required"} else {"checked"},
        "externalDependencies":external,"invocationStatus":"not_verified"}),
    )
}

/// Refresh callers must invalidate old automatic evidence when its selected
/// bytes or scope change. Manual choices are retained as choices, not proof.
pub fn invalidate_runtime_audit(skill: &mut Value, current: &BTreeMap<String, String>) -> bool {
    if skill["runtimeAudit"]["status"] != "passed" {
        return false;
    }
    let scope: BTreeSet<_> = skill["runtimeFiles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|path| path.as_str())
        .collect();
    let audited: BTreeSet<_> = skill["runtimeAudit"]["proposedScope"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|path| path.as_str())
        .collect();
    let changed = skill["runtimeAudit"]["analyzerVersion"] != RUNTIME_ANALYZER_VERSION
        || scope != audited
        || scope.iter().any(|path| {
            current.get(*path).map(String::as_str)
                != skill["runtimeAudit"]["scopeHashes"][*path].as_str()
        });
    if changed {
        skill["runtimeAudit"]["status"] = json!("needs_review");
        skill["runtimeAudit"]["canAutoConfirm"] = json!(false);
        skill["runtimeAudit"]["invalidatedAt"] = json!(now());
        skill["runtimeAudit"]["reason"] =
            json!("运行文件或范围已变化，之前的静态核对证据已失效，请重新核对");
        if skill["runtimeConfirmationKind"] == "automatic" {
            skill["runtimeConfirmed"] = json!(false);
        }
    }
    changed
}

pub fn runtime_file_selection(
    root: &Path,
    all: &BTreeMap<String, String>,
    selected: Option<&BTreeSet<String>>,
) -> Result<RuntimeFileSelection, String> {
    runtime_file_selection_with_reader(all, selected, |path| read_text(&safe_join(root, path)?))
}

/// Confirm only an explicit runtime set. No unselected file is removed.
pub fn validate_runtime_files(root: &Path, selected: &[String]) -> Result<Vec<String>, String> {
    let all = files(root)?;
    let selected: BTreeSet<_> = selected.iter().cloned().collect();
    for path in &selected {
        safe_join(root, path)?;
        if !all.contains_key(path) {
            return Err(format!("Selected runtime file is missing: {path}"));
        }
    }
    let classification = runtime_file_selection(root, &all, Some(&selected))?;
    for path in classification.required_files {
        if !selected.contains(&path) {
            return Err(format!(
                "Required runtime or license file is not selected: {path}"
            ));
        }
    }
    if !selected.contains("SKILL.md") {
        return Err("Runtime files must include SKILL.md".into());
    }
    Ok(selected.into_iter().collect())
}

fn reject_ancestor_links(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        // A drive prefix (not yet followed by RootDir) is not a filesystem
        // path. In particular \\?\D: cannot be queried as a directory.
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) if is_link(&meta) => {
                return Err(format!(
                    "Destination ancestor is a link/reparse point: {}",
                    current.display()
                ))
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(format!(
                    "Destination ancestor is not a directory: {}",
                    current.display()
                ))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(err_path("Cannot inspect destination ancestor", &current, e)),
        }
    }
    Ok(())
}

// Resolve only a caller-selected destination's existing ancestor. The returned
// absolute path has no unresolved aliases; future writes recheck its parents.
fn destination_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("Choose a full absolute destination path".into());
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("Destination cannot contain traversal components".into());
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| valid_part(s))
        .ok_or("Invalid destination file/folder name")?;
    let mut ancestor = path
        .parent()
        .ok_or("Destination has no parent folder")?
        .to_path_buf();
    let mut missing = vec![];
    while !ancestor.exists() {
        let part = ancestor
            .file_name()
            .ok_or("Destination has no existing ancestor")?
            .to_owned();
        missing.push(part);
        if !ancestor.pop() {
            return Err("Destination has no existing ancestor".into());
        }
    }
    let mut resolved = canonical(&ancestor)?;
    for part in missing.iter().rev() {
        if !valid_part(&part.to_string_lossy()) {
            return Err("Unsafe destination folder name".into());
        }
        resolved.push(part);
    }
    resolved.push(name);
    Ok(resolved)
}

fn within(path: &Path, root: &Path) -> bool {
    let p = path_key(path);
    let r = path_key(root);
    if p == r {
        return true;
    }
    #[cfg(windows)]
    let separator = "\\";
    #[cfg(not(windows))]
    let separator = "/";
    p.starts_with(&format!("{}{}", r.trim_end_matches(['/', '\\']), separator))
}

fn create_link(source: &Path, target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        junction::create(source, target)
            .map_err(|e| err_path("Cannot create directory junction", target, e))
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)
            .map_err(|e| err_path("Cannot create directory link", target, e))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (source, target);
        Err("Directory links are not supported on this platform".into())
    }
}

fn verify_link(path: &Path, expected: &Path) -> Result<(), String> {
    reject_ancestor_links(path.parent().ok_or("Connection path has no parent")?)?;
    let meta =
        fs::symlink_metadata(path).map_err(|e| err_path("Cannot inspect connection", path, e))?;
    if !is_link(&meta) {
        return Err("Connection path is no longer a link; refusing to touch it".into());
    }
    let actual =
        fs::read_link(path).map_err(|e| err_path("Cannot read connection target", path, e))?;
    let actual = if actual.is_absolute() {
        actual
    } else {
        path.parent().unwrap().join(actual)
    };
    if path_key(&actual) != path_key(expected) {
        return Err("Connection target changed; refusing to disconnect or overwrite it".into());
    }
    Ok(())
}

fn remove_link(path: &Path, expected: &Path) -> Result<(), String> {
    verify_link(path, expected)?;
    #[cfg(windows)]
    {
        // RemoveDirectoryW detaches the verified junction object only. The
        // junction crate's delete() merely strips its reparse data and leaves
        // an empty ordinary directory, which would block a later reconnect.
        // Never use remove_dir_all here: the runtime target must survive.
        fs::remove_dir(path).map_err(|e| err_path("Cannot detach directory junction", path, e))
    }
    #[cfg(unix)]
    {
        fs::remove_file(path).map_err(|e| err_path("Cannot detach directory link", path, e))
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err("Directory links are not supported on this platform".into())
    }
}

fn connection_path(skill: &Value, req: &Value) -> Result<PathBuf, String> {
    if let Some(path) = req["targetPath"].as_str().filter(|s| !s.is_empty()) {
        return destination_path(Path::new(path));
    }
    let client = required_string(req, "client")?;
    let name = skill["name"].as_str().filter(|s| valid_part(s)).ok_or(
        "Skill name cannot be used as a client folder name; choose a destination explicitly",
    )?;
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok_or("User home folder is unavailable; choose a destination")?;
    let folder = match client {
        "codex" => ".agents",
        "claude" => ".claude",
        "cursor" => ".cursor",
        _ => return Err("Unknown client".into()),
    };
    destination_path(&PathBuf::from(home).join(folder).join("skills").join(name))
}

fn connect(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(required_string(req, "skillId")?)?;
    if skill["readOnly"].as_bool() == Some(true) {
        return Err(
            "Managed/read-only skills can be inspected but cannot be deployed by this manager"
                .into(),
        );
    }
    let client = required_string(req, "client")?;
    if !matches!(client, "codex" | "claude" | "cursor") {
        return Err("Unknown client".into());
    }
    let root = local_root(&skill)?;
    // Check the runtime tree before exposing it to another client.
    files(&root)?;
    let target = connection_path(&skill, req)?;
    if within(&target, &root) || within(&root, &target) {
        return Err("Client connection cannot overlap the canonical runtime folder".into());
    }
    for connection in &m.state.connections {
        if connection["path"].as_str().map(Path::new).map(path_key) == Some(path_key(&target)) {
            if connection["skillId"] == skill["id"] && connection["client"] == client {
                verify_link(&target, &root)?;
                return Ok(m.snapshot());
            }
            return Err("Destination is already registered as another connection".into());
        }
    }
    if fs::symlink_metadata(&target).is_ok() {
        return Err("Destination already exists; no folder or link will be overwritten".into());
    }
    let parent = target.parent().ok_or("Destination has no parent")?;
    reject_ancestor_links(parent)?;
    fs::create_dir_all(parent)
        .map_err(|e| err_path("Cannot create client skills folder", parent, e))?;
    reject_ancestor_links(parent)?;
    create_link(&root, &target)?;
    if let Err(e) = verify_link(&target, &root) {
        return Err(format!(
            "Link could not be verified; connection was not registered: {e}"
        ));
    }
    let previous = m.state.clone();
    m.state.connections.push(json!({"id":m.id(),"skillId":skill["id"],"client":client,
        "path":readable_path(&target),"target":readable_path(&root),"discovered":true,"linked":true,"verified":false}));
    if let Err(e) = m.save() {
        m.state = previous;
        let rollback = remove_link(&target, &root);
        return Err(format!(
            "Could not save connection: {e}; link rollback: {}",
            rollback.err().unwrap_or_else(|| "complete".into())
        ));
    }
    Ok(m.snapshot())
}

fn disconnect(m: &mut Manager, req: &Value) -> Result<Value, String> {
    let id = required_string(req, "connectionId")?;
    let connection = m
        .state
        .connections
        .iter()
        .find(|c| c["id"] == id)
        .cloned()
        .ok_or("Connection no longer exists")?;
    let path = PathBuf::from(
        connection["path"]
            .as_str()
            .ok_or("Connection path missing")?,
    );
    let target = PathBuf::from(
        connection["target"]
            .as_str()
            .ok_or("Connection target missing")?,
    );
    remove_link(&path, &target)?;
    let previous = m.state.clone();
    m.state.connections.retain(|c| c["id"] != id);
    if let Err(e) = m.save() {
        m.state = previous;
        let rollback = create_link(&target, &path);
        return Err(format!(
            "Could not save disconnection: {e}; link rollback: {}",
            rollback.err().unwrap_or_else(|| "complete".into())
        ));
    }
    Ok(m.snapshot())
}

pub(crate) fn export_destination(root: &Path, destination: &str) -> Result<PathBuf, String> {
    let destination = pending_export_destination(root, destination)?;
    if !destination
        .parent()
        .ok_or("Export destination has no parent")?
        .is_dir()
    {
        return Err("Choose an existing destination folder".into());
    }
    Ok(destination)
}

/// Resolve and validate without writing; only an explicit default export task
/// may create its configured parent after its content proof has passed.
pub(crate) fn pending_export_destination(
    root: &Path,
    destination: &str,
) -> Result<PathBuf, String> {
    let destination = destination_path(Path::new(destination))?;
    if within(&destination, root) {
        return Err("Export destination cannot be inside the runtime source folder".into());
    }
    let parent = destination
        .parent()
        .ok_or("Export destination has no parent")?;
    reject_ancestor_links(parent)?;
    Ok(destination)
}

/// Write an already validated mixed local/remote package without retaining a
/// second Skill folder. Callers bind local bytes and provenance to this snapshot.
pub(crate) fn export_package(
    root: &Path,
    destination: &str,
    expected: &BTreeMap<String, String>,
    local_paths: &[String],
    data: &BTreeMap<String, Vec<u8>>,
    provenance: &Value,
) -> Result<Value, String> {
    crate::transaction::verify_root(root)?;
    let destination = export_destination(root, destination)?;
    let provenance_name = "skill-manager-provenance.json";
    if data.contains_key(provenance_name) || local_paths.iter().any(|path| path == provenance_name)
    {
        return Err("Skill already contains the reserved export provenance filename".into());
    }
    for path in data.keys().chain(local_paths.iter()) {
        let actual = safe_join(root, path)?;
        if crate::external::private_resource(Path::new(path))
            || crate::external::private_resource(&actual)
        {
            return Err(format!(
                "凭据或机器私有配置不能加入 ZIP：{path}；请改为运行环境配置"
            ));
        }
    }
    if local_paths
        .iter()
        .any(|path| !expected.contains_key(path) || data.contains_key(path))
    {
        return Err("Local export selection is missing or conflicts with a remote file".into());
    }
    if files(root)? != *expected {
        return Err(
            "Local files changed while preparing export; retry after editing has finished".into(),
        );
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|e| err_path("Cannot create a new export", &destination, e))?;
    let result = (|| -> Result<(), String> {
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        for path in local_paths {
            zip.start_file(path, options).map_err(|e| e.to_string())?;
            let mut source = fs::File::open(safe_join(root, path)?)
                .map_err(|e| err_path("Cannot export file", &root.join(path), e))?;
            std::io::copy(&mut source, &mut zip).map_err(|e| e.to_string())?;
        }
        for (path, bytes) in data {
            zip.start_file(path, options).map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())?;
        }
        zip.start_file(provenance_name, options)
            .map_err(|e| e.to_string())?;
        zip.write_all(&serde_json::to_vec_pretty(provenance).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        zip.finish().map_err(|e| e.to_string())?;
        crate::transaction::verify_root(root)?;
        if files(root)? != *expected {
            return Err(
                "Runtime files changed during export; retry after editing has finished".into(),
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        return Err(format!(
            "Export failed: {error}{}",
            fs::remove_file(&destination)
                .err()
                .map(|e| format!("; incomplete export could not be removed: {e}"))
                .unwrap_or_default()
        ));
    }
    Ok(json!({"path":readable_path(&destination)}))
}

fn export(m: &Manager, req: &Value) -> Result<Value, String> {
    let skill = m.skill(required_string(req, "skillId")?)?;
    let root = local_root(&skill)?;
    if skill["runtimeConfirmed"].as_bool() != Some(true) {
        return Err("Confirm the runtime files on the Files page before exporting".into());
    }
    let selected = string_list(&skill, "runtimeFiles")?;
    let selected = validate_runtime_files(&root, &selected)?;
    let destination = export_destination(&root, required_string(req, "destination")?)?;
    let snapshot = files(&root)?;
    let provenance_name = "skill-manager-provenance.json";
    if selected.iter().any(|s| s == provenance_name) {
        return Err("Skill already contains the reserved export provenance filename".into());
    }
    let selection: BTreeSet<_> = selected.iter().cloned().collect();
    let classification = runtime_file_selection(&root, &snapshot, Some(&selection))?;
    let warnings =
        crate::external::sanitized_warnings(&classification.warnings, &classification.issues);
    let external_ids = req
        .get("externalIds")
        .map(|_| string_list(req, "externalIds"))
        .transpose()?
        .unwrap_or_default();
    let external = crate::external::export_resources(&skill, &selected, &external_ids)?;
    if external.data.iter().any(|(path, bytes)| {
        snapshot
            .get(path)
            .is_some_and(|expected| expected != &format!("{:x}", Sha256::digest(bytes)))
    }) {
        return Err("外部资源的包内路径与 Skill 文件冲突".into());
    }
    let mut data = external.data;
    data.extend(external.replacements.clone());
    let local_paths: Vec<_> = selected
        .iter()
        .filter(|path| !data.contains_key(*path))
        .cloned()
        .collect();
    let exported_hashes: BTreeMap<String, String> = local_paths
        .iter()
        .filter_map(|path| snapshot.get(path).map(|hash| (path.clone(), hash.clone())))
        .chain(
            data.iter()
                .map(|(path, bytes)| (path.clone(), format!("{:x}", Sha256::digest(bytes)))),
        )
        .collect();
    let provenance = json!({"formatVersion":1,"name":skill["name"],"version":skill["version"],
        "repo":skill["repo"],"branch":skill["branch"],"commit":skill["commit"],
        "baselineCommit":skill["baselineCommit"],"remotePath":skill["remotePath"],"private":skill["private"],
        "exportedAt":now(),"runtimeFiles":selected,"runtimeConfirmed":true,
        "hashes":exported_hashes,
        "licenseFiles":selected.iter().filter(|p| license_path(p)).collect::<Vec<_>>(),
        "licenseStatus":if selected.iter().any(|p| license_path(p)) {"included"} else {"unknown; no license file found; redistribution rights are not established"},"warnings":warnings,
        "externalResources":external.provenance
    });
    export_package(
        &root,
        destination.to_str().ok_or("导出路径无效")?,
        &snapshot,
        &local_paths,
        &data,
        &provenance,
    )
}

pub fn handle(m: &mut Manager, req: &Value) -> Result<Option<Value>, String> {
    match req["action"].as_str().unwrap_or("") {
        "scan_local" => Ok(Some(scan(Path::new(required_string(req, "path")?))?)),
        "add_local" => {
            let paths = string_list(req, "paths")?;
            if paths.is_empty() {
                return Err("Select at least one local Skill folder".into());
            }
            let drafts: Vec<_> = paths
                .iter()
                .map(|p| draft(Path::new(p)))
                .collect::<Result<_, _>>()?;
            let previous = m.state.clone();
            for mut value in drafts {
                let key = path_key(Path::new(value["localPath"].as_str().unwrap_or_default()));
                let ignored_before = m.state.ignored_local_paths.len();
                m.state
                    .ignored_local_paths
                    .retain(|path| path_key(Path::new(path)) != key);
                if ignored_before != m.state.ignored_local_paths.len() {
                    m.state.check_generation += 1;
                }
                let exists = m.state.skills.iter().any(|s| {
                    s["localPath"]
                        .as_str()
                        .map(Path::new)
                        .and_then(|p| canonical(p).ok())
                        .map(|p| path_key(&p))
                        == Some(key.clone())
                });
                if !exists {
                    value["id"] = json!(m.id());
                    crate::audit::refresh_local_audit(&mut value, None);
                    m.upsert_skill(value);
                }
            }
            if let Err(e) = m.save() {
                m.state = previous;
                return Err(e);
            }
            Ok(Some(m.snapshot()))
        }
        "get_files" => {
            let mut skill = m.skill(required_string(req, "skillId")?)?;
            if skill["localPath"].as_str().is_none() {
                return Ok(None);
            }
            let inspected =
                local_root(&skill).and_then(|root| files(&root).map(|hashes| (root, hashes)));
            let (root, hashes) = match inspected {
                Ok(current) => current,
                Err(error) => {
                    let confirmed = skill["runtimeConfirmed"].clone();
                    let previous = m.state.clone();
                    if skill["runtimeConfirmationKind"] == "automatic" {
                        skill["runtimeConfirmed"] = json!(false);
                    }
                    if skill["runtimeAudit"]["status"] == "failed"
                        && skill["runtimeAudit"]["reason"] == error
                        && skill["runtimeAudit"]["preservedScope"] == skill["runtimeFiles"]
                        && skill["runtimeConfirmed"] == confirmed
                    {
                        return Err(error);
                    }
                    skill["runtimeAudit"] = json!({"status":"failed","checkedAt":now(),"invalidatedAt":now(),"method":"static_package","reason":error,"canAutoConfirm":false,"preservedScope":skill["runtimeFiles"],"invocationStatus":"not_verified"});
                    m.upsert_skill(skill);
                    if let Err(save_error) = m.save() {
                        m.state = previous;
                        return Err(save_error);
                    }
                    return Err(error);
                }
            };
            if crate::audit::refresh_local_audit(&mut skill, Some(&hashes)) {
                let previous = m.state.clone();
                m.upsert_skill(skill.clone());
                if let Err(error) = m.save() {
                    m.state = previous;
                    return Err(error);
                }
            }
            let confirmed = skill["runtimeConfirmed"].as_bool() == Some(true);
            let existing: BTreeSet<_> = string_list(&skill, "runtimeFiles")
                .unwrap_or_default()
                .into_iter()
                .collect();
            let mut classification =
                runtime_file_selection(&root, &hashes, confirmed.then_some(&existing))?;
            let external =
                crate::external::inspect_issues(&skill, &root, &hashes, &classification.issues);
            crate::external::apply_resolutions(&mut classification, &external);
            let (export_required, export_error) =
                match minimal_runtime_for_skill(&skill, &root, &hashes) {
                    Ok(paths) => (paths, None),
                    Err(error) => (classification.required_files.clone(), Some(error)),
                };
            let export_error = export_error.or_else(|| {
                external
                    .iter()
                    .find(|item| item.required && item.status != "available")
                    .map(|item| format!("必要外部文件不可用：{}；{}", item.target, item.reason))
            });
            let external_summary =
                crate::external::report(&external, &skill["id"])["summary"].clone();
            let selected: BTreeSet<_> = classification.suggested_files.iter().cloned().collect();
            let entries: Vec<_> = hashes.iter().map(|(path,hash)| {
                let full = safe_join(&root,path)?;
                let size = fs::metadata(full).map_err(|e|e.to_string())?.len();
                Ok(json!({"path":path,"hash":hash,"size":size,"runtime":selected.contains(path)}))
            }).collect::<Result<_,String>>()?;
            let mut issues =
                serde_json::to_value(&classification.issues).map_err(|error| error.to_string())?;
            for issue in skill["runtimeAudit"]["issues"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if !issues
                    .as_array()
                    .expect("classified issue list")
                    .contains(issue)
                {
                    issues
                        .as_array_mut()
                        .expect("classified issue list")
                        .push(issue.clone());
                }
            }
            Ok(Some(
                json!({"files":entries,"runtimeConfirmed":confirmed,"requiredFiles":classification.required_files,
                    "runtimeFiles":skill["runtimeFiles"],
                    "runtimeAudit":skill["runtimeAudit"],"runtimeConfirmationKind":skill["runtimeConfirmationKind"],
                    "exportRequiredFiles":export_required,"exportError":export_error,
                    "optionalFiles":classification.optional_files,"requiredReasons":classification.required_reasons,
                    "suggestedFiles":classification.suggested_files,"warnings":classification.warnings,"issues":issues,
                    "externalDependencies":external,"externalSummary":external_summary}),
            ))
        }
        "read_file" => {
            let skill = m.skill(required_string(req, "skillId")?)?;
            if skill["localPath"].as_str().is_none() {
                return Ok(None);
            }
            let path = safe_join(&local_root(&skill)?, required_string(req, "path")?)?;
            Ok(Some(json!({"content":read_text(&path)?})))
        }
        "set_runtime_files" => {
            let mut skill = m.skill(required_string(req, "skillId")?)?;
            let selected =
                validate_runtime_files(&local_root(&skill)?, &string_list(req, "files")?)?;
            let root = local_root(&skill)?;
            let scope = selected.iter().cloned().collect();
            let classification = runtime_file_selection(&root, &files(&root)?, Some(&scope))?;
            let previous = m.state.clone();
            let prior_scope: BTreeSet<_> = string_list(&skill, "runtimeFiles")
                .unwrap_or_default()
                .into_iter()
                .collect();
            if prior_scope != scope {
                for field in [
                    "comparisonLocalHashes",
                    "comparisonRemoteHashes",
                    "comparisonObservedAt",
                    "comparisonRuntimeFiles",
                    "comparisonRuntimeSelection",
                    "comparisonCommit",
                    "comparisonScope",
                    "comparisonDocHash",
                ] {
                    skill[field] = Value::Null;
                }
                skill["changeDetails"] = json!([]);
                skill["remoteHashes"] = Value::Null;
                if skill["sourceId"].as_str().is_some() {
                    skill["remoteCheckRequired"] = json!(true);
                    skill["associationNote"] = json!("运行清单已变化，需重新检查双方内容");
                    skill["status"] = json!("unknown_baseline");
                } else {
                    skill["status"] = json!("local_only");
                }
            }
            skill["runtimeFiles"] = json!(selected);
            skill["runtimeConfirmed"] = json!(true);
            skill["runtimeWarnings"] = json!(classification.warnings);
            skill["runtimeIssues"] = json!(classification.issues);
            skill["runtimeConfirmationKind"] = json!("manual");
            skill["runtimeAudit"] = Value::Null;
            m.upsert_skill(skill);
            if let Err(e) = m.save() {
                m.state = previous;
                return Err(e);
            }
            Ok(Some(m.snapshot()))
        }
        "connect" => Ok(Some(connect(m, req)?)),
        "disconnect" => Ok(Some(disconnect(m, req)?)),
        "export" => Ok(Some(export(m, req)?)),
        "forget" => {
            let id = required_string(req, "skillId")?;
            let skill = m.skill(id)?;
            if m.state.connections.iter().any(|c| c["skillId"] == id) {
                return Err(
                    "Disconnect this Skill from its clients before forgetting its record".into(),
                );
            }
            let previous = m.state.clone();
            if let Some(path) = skill["localPath"].as_str() {
                let path = canonical(Path::new(path)).unwrap_or_else(|_| PathBuf::from(path));
                let path = readable_path(&path);
                if !m
                    .state
                    .ignored_local_paths
                    .iter()
                    .any(|old| path_key(Path::new(old)) == path_key(Path::new(&path)))
                {
                    m.state.ignored_local_paths.push(path);
                    m.state.check_generation += 1;
                }
            }
            if let (Some(source_id), Some(remote_path)) =
                (skill["sourceId"].as_str(), skill["remotePath"].as_str())
            {
                if let Some(source) = m.state.sources.iter_mut().find(|s| s["id"] == source_id) {
                    let mut ignored = source["ignoredPaths"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let path = json!(remote_path);
                    if !ignored.contains(&path) {
                        ignored.push(path);
                    }
                    source["ignoredPaths"] = json!(ignored);
                }
            }
            m.state.skills.retain(|s| s["id"] != id);
            if let Err(e) = m.save() {
                m.state = previous;
                return Err(e);
            }
            Ok(Some(m.snapshot()))
        }
        "refresh" => {
            let selected = m.refresh_selection(req)?;
            let candidates: Vec<_> = m
                .state
                .skills
                .iter()
                .filter(|s| {
                    s["sourceId"].as_str().is_none()
                        && s["localPath"].as_str().is_some()
                        && selected
                            .as_ref()
                            .is_none_or(|ids| s["id"].as_str().is_some_and(|id| ids.contains(id)))
                })
                .cloned()
                .collect();
            let previous = m.state.clone();
            for mut skill in candidates {
                match local_root(&skill).and_then(|p| draft(&p)) {
                    Ok(value) => {
                        let current = value["localHashes"]
                            .as_object()
                            .into_iter()
                            .flatten()
                            .filter_map(|(path, hash)| {
                                hash.as_str().map(|hash| (path.clone(), hash.to_owned()))
                            })
                            .collect();
                        invalidate_runtime_audit(&mut skill, &current);
                        for field in [
                            "description",
                            "displayName",
                            "version",
                            "readOnly",
                            "readOnlyReason",
                            "localHashes",
                        ] {
                            skill[field] = value[field].clone();
                        }
                        if skill["managed"].as_bool() == Some(true) {
                            skill["readOnly"] = json!(true);
                            skill["readOnlyReason"] = json!("System, application, or plugin-managed skill; inspect without changing its files");
                        }
                        crate::audit::refresh_local_audit(&mut skill, Some(&current));
                        skill["status"] = json!("local_only");
                        skill["lastLocalCheckedAt"] = json!(now());
                        skill["error"] = Value::Null;
                    }
                    Err(e) => {
                        invalidate_runtime_audit(&mut skill, &BTreeMap::new());
                        crate::audit::refresh_local_audit(&mut skill, None);
                        skill["error"] = json!(e);
                    }
                }
                skill["lastChecked"] = json!(now());
                m.upsert_skill(skill);
            }
            if let Err(e) = m.save() {
                m.state = previous;
                return Err(e);
            }
            let needs_remote = match &selected {
                None => !m.state.sources.is_empty(),
                Some(ids) => m.state.skills.iter().any(|s| {
                    s["sourceId"].as_str().is_some()
                        && s["id"].as_str().is_some_and(|id| ids.contains(id))
                }),
            };
            if needs_remote {
                Ok(None)
            } else {
                Ok(Some(m.snapshot()))
            }
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod issue_tests {
    use super::*;

    #[test]
    fn continuous_chinese_template_sentence_resolves_each_real_file_without_fragments() {
        let contents = BTreeMap::from([
            ("SKILL.md", "Read references/tex-output.md。"),
            (
                "references/tex-output.md",
                "assets/model-extract.tex和assets/model-report.tex以@@UPPER_CASE@@标记待填项。",
            ),
            ("assets/model-extract.tex", "template one"),
            ("assets/model-report.tex", "template two"),
        ]);
        let all = contents
            .keys()
            .map(|path| ((*path).to_owned(), "hash".to_owned()))
            .collect();
        let selection =
            runtime_file_selection_with_reader(&all, None, |path| Ok(contents[path].into()))
                .unwrap();
        assert!(selection.issues.is_empty(), "{:?}", selection.issues);
        for path in ["assets/model-extract.tex", "assets/model-report.tex"] {
            assert!(selection.required_files.contains(&path.to_owned()));
        }
        assert!(!selection
            .required_files
            .iter()
            .any(|path| path.contains("标记") || path.contains("tex和")));
    }

    #[test]
    fn chinese_filename_and_explicit_spaced_parenthesized_destinations_are_not_split() {
        let contents = BTreeMap::from([
            ("SKILL.md", "Read `assets/模型.tex和说明.md`、[模板](<assets/中文 模板 (最终).tex>) 和 [第二份](assets/另一个 (修订).tex)。\n另见 assets/中文 模板 (最终).tex。"),
            ("assets/模型.tex和说明.md", "A real Chinese filename."),
            ("assets/中文 模板 (最终).tex", "template one"),
            ("assets/另一个 (修订).tex", "template two"),
        ]);
        let all = contents
            .keys()
            .map(|path| ((*path).to_owned(), "hash".to_owned()))
            .collect();
        let selection =
            runtime_file_selection_with_reader(&all, None, |path| Ok(contents[path].into()))
                .unwrap();
        assert!(selection.issues.is_empty(), "{:?}", selection.issues);
        assert_eq!(selection.required_files.len(), 4);
        let bare = dependency_tokens("assets/模型.tex和说明.md", false, &all);
        assert_eq!(bare.len(), 1);
        assert_eq!(bare[0].target, "assets/模型.tex和说明.md");
    }

    #[test]
    fn missing_continuous_paths_still_report_two_specific_missing_resources() {
        let all = BTreeMap::from([("SKILL.md".to_owned(), "hash".to_owned())]);
        let selection = runtime_file_selection_with_reader(&all, None, |_| {
            Ok("assets/one.tex和assets/two.tex以占位符填写。".into())
        })
        .unwrap();
        assert_eq!(selection.issues.len(), 2);
        for target in ["assets/one.tex", "assets/two.tex"] {
            assert!(selection.issues.iter().any(|issue| issue.target == target
                && issue.category == "package_required"
                && issue.severity == "error"));
        }
    }

    #[test]
    fn structured_issues_distinguish_missing_package_external_condition_and_history() {
        let all = BTreeMap::from([("SKILL.md".to_owned(), "hash".to_owned())]);
        let text = "# Current\nRun `scripts/missing.py`。\n必须读取 [当前规则](D:/current/AGENTS.md)。\n按需读取 `D:\\current\\optional.yaml`。\n# Sources\n[旧交接](D:/old/checkpoint.md) 是历史来源。\n必须读取 [仍有效的外部规则](D:/current/rules.md)。\n```md\nRun scripts/example.py\n```\n";
        let result = runtime_file_selection_with_reader(&all, None, |_| Ok(text.into())).unwrap();
        for (target, category, severity) in [
            ("scripts/missing.py", "package_required", "error"),
            ("D:/current/AGENTS.md", "external", "warning"),
            ("D:\\current\\optional.yaml", "external", "info"),
            ("D:/old/checkpoint.md", "history", "info"),
            ("D:/current/rules.md", "external", "warning"),
        ] {
            assert!(
                result.issues.iter().any(|issue| issue.target == target
                    && issue.category == category
                    && issue.severity == severity),
                "{target}: {:?}",
                result.issues
            );
        }
        assert!(!result
            .issues
            .iter()
            .any(|issue| issue.target.contains("example.py")));
        assert!(result.issues.iter().all(|issue| issue.source == "SKILL.md"));
    }

    #[test]
    fn chinese_punctuation_does_not_swallow_prose_or_turn_existing_paths_into_missing_files() {
        let contents = BTreeMap::from([
            ("SKILL.md", "Read `references/workflow.md`。"),
            (
                "references/workflow.md",
                "执行版为 references/workflow.md，独立MD可直接阅读；记录来源。",
            ),
        ]);
        let all = contents
            .keys()
            .map(|path| ((*path).to_owned(), "hash".to_owned()))
            .collect();
        let result =
            runtime_file_selection_with_reader(&all, None, |path| Ok(contents[path].into()))
                .unwrap();
        assert!(result.issues.is_empty(), "{:?}", result.issues);
        assert!(result
            .required_files
            .contains(&"references/workflow.md".to_owned()));
    }

    #[test]
    fn minimum_export_keeps_required_linked_readme_and_license_but_excludes_unselected_notes() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/runtime-issue-tests");
        fs::create_dir_all(&parent).unwrap();
        let fixture = tempfile::tempdir_in(parent).unwrap();
        fs::write(
            fixture.path().join("SKILL.md"),
            "Read [required guide](README.md).",
        )
        .unwrap();
        fs::write(fixture.path().join("README.md"), "Runtime instructions.").unwrap();
        fs::write(fixture.path().join("LICENSE"), "Fixture license.").unwrap();
        fs::write(fixture.path().join("CHANGELOG.md"), "Optional history.").unwrap();
        assert_eq!(
            minimal_runtime_files(fixture.path()).unwrap(),
            vec!["LICENSE", "README.md", "SKILL.md"]
        );
        fs::write(
            fixture.path().join("README.md"),
            "Run `scripts/missing.py`.",
        )
        .unwrap();
        assert!(minimal_runtime_files(fixture.path())
            .unwrap_err()
            .contains("scripts/missing.py"));
    }

    #[test]
    fn dynamic_reads_remain_uncertain_while_literal_resources_and_explicit_outputs_do_not() {
        assert!(has_dynamic_runtime_read("data = open(input_path).read()"));
        assert!(has_dynamic_runtime_read(
            "fs.readFileSync(prefix + '.json')"
        ));
        assert!(has_dynamic_runtime_read("open('data/' + name).read()"));
        assert!(!has_dynamic_runtime_read("open('data.json').read()"));
        assert!(!has_dynamic_runtime_read(
            "open(output_path, 'w').write('generated')"
        ));
        let contents = BTreeMap::from([
            ("SKILL.md", "Run `scripts/run.py`."),
            ("scripts/run.py", "data = open(input_path).read()"),
        ]);
        let all = contents
            .keys()
            .map(|path| ((*path).to_owned(), "hash".to_owned()))
            .collect();
        let result =
            runtime_file_selection_with_reader(&all, None, |path| Ok(contents[path].into()))
                .unwrap();
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.category == "uncertain" && issue.source == "scripts/run.py"));
    }

    #[test]
    fn cli_inputs_generated_outputs_suffixes_and_regexes_are_not_missing_package_resources() {
        let contents = BTreeMap::from([
            ("SKILL.md", "Run `scripts/extract.py`."),
            ("scripts/extract.py", "import argparse\nfrom pathlib import Path\nparser = argparse.ArgumentParser()\nargs = parser.parse_args()\nsource = Path(args.source)\nwith source.open('rb') as stream:\n    data = stream.read()\nwith (destination / 'extraction.json').open('x') as out:\n    out.write('{}')\n(destination / 'source.raw.md').write_text(data)\nSUFFIXES = ('.png', '.jpg', '.jpeg')\npattern = r'\\d+'\noperator = '/'\nprint('](Get-Content)')\n"),
        ]);
        let all = contents
            .keys()
            .map(|path| ((*path).to_owned(), "hash".to_owned()))
            .collect();
        let result =
            runtime_file_selection_with_reader(&all, None, |path| Ok(contents[path].into()))
                .unwrap();
        assert!(
            !result
                .issues
                .iter()
                .any(|issue| issue.category == "package_required" || issue.category == "uncertain"),
            "{:?}",
            result.issues
        );
        assert!(!result
            .issues
            .iter()
            .any(|issue| ["/", ".png", ".jpg", ".jpeg", "Get-Content"]
                .contains(&issue.target.as_str())));
        assert!(!absolute_reference("/"));
        assert!(!absolute_reference("//"));
        assert!(!absolute_reference(r"\n"));
        assert!(!absolute_reference(r"\d+"));
        assert!(absolute_reference("/actual/file.json"));
        assert!(absolute_reference(r"D:\actual\rules.md"));
    }

    #[test]
    fn workspace_powershell_names_and_shell_code_are_separate_from_script_anchored_dependencies() {
        let contents=BTreeMap::from([
            ("SKILL.md","Run `scripts/run.ps1`."),
            ("scripts/run.ps1","param($Workspace)\nif (@('AGENTS.md', 'config.toml') -contains $Name) { throw 'protected' }\nGet-Content (Join-Path $Workspace 'AGENTS.md')\n. (Join-Path $PSScriptRoot 'helper.ps1')\n"),
            ("scripts/helper.ps1","Write-Output 'fixture'\n"),
        ]);
        let all = contents
            .keys()
            .map(|path| ((*path).to_owned(), "hash".to_owned()))
            .collect();
        let result =
            runtime_file_selection_with_reader(&all, None, |path| Ok(contents[path].into()))
                .unwrap();
        assert!(result
            .required_files
            .contains(&"scripts/helper.ps1".to_owned()));
        assert!(
            !result
                .issues
                .iter()
                .any(|issue| issue.category == "package_required" || issue.category == "uncertain"),
            "{:?}",
            result.issues
        );
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.target == "AGENTS.md" && issue.category == "external"));
        let missing = BTreeMap::from([
            ("SKILL.md".into(), "hash".into()),
            ("scripts/run.ps1".into(), "hash".into()),
        ]);
        let result =
            runtime_file_selection_with_reader(&missing, None, |path| Ok(contents[path].into()))
                .unwrap();
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.target == "helper.ps1"
                && issue.category == "package_required"
                && issue.severity == "error"));
    }

    #[test]
    fn cli_presence_does_not_hide_self_anchored_missing_resources_or_opaque_internal_reads() {
        let contents=BTreeMap::from([
            ("SKILL.md","Run `scripts/run.py`."),
            ("scripts/run.py","import argparse\nfrom pathlib import Path\nROOT = Path(__file__).resolve().parent.parent\nresource = ROOT / 'assets/schema.json'\nopen(resource).read()\n"),
        ]);
        let all = contents
            .keys()
            .map(|path| ((*path).to_owned(), "hash".to_owned()))
            .collect();
        let result =
            runtime_file_selection_with_reader(&all, None, |path| Ok(contents[path].into()))
                .unwrap();
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.target == "assets/schema.json" && issue.severity == "error"));
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.category == "uncertain"));
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/runtime-issue-tests");
        fs::create_dir_all(&directory).unwrap();
        let fixture = tempfile::tempdir_in(directory).unwrap();
        fs::create_dir(fixture.path().join("scripts")).unwrap();
        for (path, content) in &contents {
            fs::write(fixture.path().join(path), content).unwrap();
        }
        assert!(minimal_runtime_files(fixture.path()).is_err());
    }

    #[test]
    fn generated_document_paths_and_other_skill_links_remain_external_but_current_missing_paths_fail(
    ) {
        let contents=BTreeMap::from([
            ("SKILL.md","`prepare.py` creates guides under `references/generated/`.\nAttach `references/generated/row.png` after generation.\nCreate `references/canonical.png` from the chosen output.\nRead [other skill](../../other/SKILL.md).\nRead [current required](references/current.md).\nRead [source record](references/sources.md).\n"),
            ("references/sources.md","# Borrowed sources\nRead upstream `scripts/not-copied.py` only as a historical source.\n"),
        ]);
        let all = contents
            .keys()
            .map(|path| ((*path).to_owned(), "hash".to_owned()))
            .collect();
        let result =
            runtime_file_selection_with_reader(&all, None, |path| Ok(contents[path].into()))
                .unwrap();
        for target in [
            "references/generated/",
            "references/generated/row.png",
            "references/canonical.png",
            "../../other/SKILL.md",
        ] {
            assert!(
                result
                    .issues
                    .iter()
                    .any(|issue| issue.target == target && issue.category == "external"),
                "{target}: {:?}",
                result.issues
            );
        }
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.target == "references/current.md"
                && issue.category == "package_required"));
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.target == "scripts/not-copied.py" && issue.category == "history"));
    }

    #[test]
    fn operational_fences_keep_scripts_and_legal_resource_names_in_the_minimum_package() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/runtime-issue-tests");
        fs::create_dir_all(&parent).unwrap();
        let fixture = tempfile::tempdir_in(parent).unwrap();
        fs::create_dir_all(fixture.path().join("scripts")).unwrap();
        fs::create_dir_all(fixture.path().join("assets")).unwrap();
        fs::write(fixture.path().join("SKILL.md"),"```bash\npython \"$SKILL_DIR/scripts/run.py\"\npython root.py\nsh \"${SKILL_ROOT}/scripts/helper.sh\"\n```\n```md\nExample scripts/not-a-runtime.py\n```\n").unwrap();
        fs::write(fixture.path().join("scripts/run.py"),"open('../assets/data(1).json').read()\nopen('../assets/frame[2].png').read()\nopen('../assets/data+more.json').read()\n").unwrap();
        fs::write(fixture.path().join("root.py"), "print('root entry')\n").unwrap();
        fs::write(fixture.path().join("scripts/helper.sh"), "printf fixture\n").unwrap();
        assert!(minimal_runtime_files(fixture.path())
            .unwrap_err()
            .contains("data(1).json"));
        for path in [
            "assets/data(1).json",
            "assets/frame[2].png",
            "assets/data+more.json",
        ] {
            fs::write(fixture.path().join(path), b"fixture").unwrap();
        }
        assert_eq!(
            minimal_runtime_files(fixture.path()).unwrap(),
            vec![
                "SKILL.md",
                "assets/data(1).json",
                "assets/data+more.json",
                "assets/frame[2].png",
                "root.py",
                "scripts/helper.sh",
                "scripts/run.py"
            ]
        );
    }
}
