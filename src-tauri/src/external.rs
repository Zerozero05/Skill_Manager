//! External resources stay at their original location. Packaging reads them
//! into the user-selected ZIP only; it never installs or trims an original.
use crate::{
    local,
    model::{now, Manager},
    remote::github::{Entry, Snapshot},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_RESOURCE: usize = 4 * 1024 * 1024;
const MAX_RESOURCES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub id: String,
    pub source: String,
    pub target: String,
    pub source_hash: String,
    pub reference_fingerprint: String,
    pub role: String,
    pub status: String,
    pub reason: String,
    pub canonical_path: Option<String>,
    pub kind: Option<String>,
    pub hash: Option<String>,
    pub package_path: Option<String>,
    pub required: bool,
    pub default_included: bool,
    pub portable: bool,
    pub exists: bool,
    pub readable: bool,
    pub checked_at: String,
    pub binding_stale: bool,
    pub reference_kind: String,
    pub local: Value,
    pub remote: Value,
    pub effective_source: Option<String>,
    pub candidates: Vec<Value>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn display(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\UNC\")
        .map(|tail| format!(r"\\{tail}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| text.into_owned())
}
fn key(path: &Path) -> String {
    let value = display(path).replace('\\', "/");
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}
fn normalized(text: &str) -> String {
    text.replace('\\', "/")
}
fn absolute_target(text: &str) -> bool {
    let bytes = text.as_bytes();
    text.starts_with('/')
        || text.starts_with("\\\\")
        || text.starts_with("~/")
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}
fn target_path(root: &Path, source: &str, target: &str) -> Option<PathBuf> {
    if let Some(tail) = target.strip_prefix("~/") {
        return std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(|home| PathBuf::from(home).join(tail));
    }
    if absolute_target(target) {
        return Some(PathBuf::from(target));
    }
    if target.starts_with("../") || target.starts_with("..\\") {
        return Some(root.join(source).parent()?.join(target));
    }
    None
}
fn source_text(root: &Path, source: &str) -> String {
    let Ok(path) = local::safe_join(root, source) else {
        return String::new();
    };
    let Ok(metadata) = fs::metadata(&path) else {
        return String::new();
    };
    if metadata.len() > 1024 * 1024 {
        return String::new();
    }
    fs::read_to_string(path).unwrap_or_default()
}
fn source_context(text: &str, target: &str) -> String {
    let needle = normalized(target);
    text.lines()
        .filter(|line| {
            let line = normalized(line);
            line.match_indices(&needle).any(|(position, _)| {
                let tail = &line[position + needle.len()..];
                let mut chars = tail.chars();
                match chars.next() {
                    Some('/' | '\\' | '_' | '-') => false,
                    Some(character) if character.is_ascii_alphanumeric() => false,
                    Some('.') => !chars
                        .next()
                        .is_some_and(|character| character.is_ascii_alphanumeric()),
                    _ => true,
                }
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub(crate) fn private_resource(path: &Path) -> bool {
    let normalized = key(path);
    let filename = normalized.rsplit('/').next().unwrap_or("");
    normalized
        .split('/')
        .any(|part| matches!(part, ".ssh" | ".aws" | ".azure" | ".kube" | ".gnupg"))
        || filename == ".env"
        || filename.starts_with(".env.")
        || [
            "auth.json",
            "credentials",
            "credentials.json",
            ".git-credentials",
            ".npmrc",
            "cookies",
            "login data",
            "id_rsa",
            "id_ed25519",
        ]
        .contains(&filename)
        || ["password", "credential", "secret", "token"]
            .iter()
            .any(|word| filename.contains(word))
}

fn other_skill_file(path: &Path, root: &Path) -> bool {
    path.parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .any(|parent| key(parent) != key(root) && parent.join("SKILL.md").is_file())
}
fn history_target(target: &str, context: &str) -> bool {
    let path = normalized(target).to_lowercase();
    path.split('/').any(|part| part == "_records")
        || [
            "维护索引",
            "历史备份",
            "历史来源",
            "证据索引",
            "记录：",
            "记录:",
        ]
        .iter()
        .any(|word| context.contains(word))
}
fn inferred_role(issue: &local::RuntimeIssue, context: &str) -> &'static str {
    if issue.category == "package_required" {
        return "required";
    }
    if issue.category == "command" {
        return "environment";
    }
    // Preserve the package scanner's established distinction between concrete
    // file resources and CLI/workspace/generated inputs. A script reference is
    // not automatically a distributable file merely because its source is code.
    if issue.category == "external"
        && ((issue.severity == "info"
            && !absolute_target(&issue.target)
            && ![
                "必须读取",
                "执行前读取",
                "运行时读取",
                "must read",
                "must load",
            ]
            .iter()
            .any(|word| context.to_lowercase().contains(word))
            && !(issue.target.starts_with("../")
                && ["按需", "需要时", "如果", "when needed", "if "]
                    .iter()
                    .any(|word| context.to_lowercase().contains(word))))
            || issue.target == "运行时输入"
            || issue.reason.contains("工作目录或参数相对路径")
            || issue.reason.contains("文档说明在任务运行目录生成"))
    {
        return "environment";
    }
    if issue.category == "history" || history_target(&issue.target, context) {
        return "history";
    }
    if private_resource(Path::new(&issue.target)) {
        return "environment";
    }
    let text = context.to_lowercase();
    let filename = normalized(&issue.target)
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_lowercase();
    if matches!(filename.as_str(), "agents.md" | "config.toml")
        || text.contains("codex_home")
        || text.contains("源码：")
        || text.contains("客户端入口")
    {
        return "environment";
    }
    let conditional = [
        "按需",
        "需要时",
        "若",
        "如果",
        "仅在",
        "可访问",
        "when needed",
        "if ",
    ]
    .iter()
    .any(|word| text.contains(word));
    let required = [
        "必须读取",
        "执行前读取",
        "运行时读取",
        "先读取",
        "必须加载",
        "运行必需",
        "必要资源",
        "required at runtime",
        "must read",
        "must load",
    ]
    .iter()
    .any(|word| text.contains(word));
    let script = matches!(
        Path::new(&issue.source)
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("py" | "js" | "ts" | "mjs" | "cjs" | "sh" | "ps1" | "rb")
    );
    if (required || script) && !conditional {
        "required"
    } else if conditional
        && (absolute_target(&issue.target) || issue.target.starts_with("../"))
        && !filename.is_empty()
        && filename.contains('.')
    {
        "optional"
    } else {
        "environment"
    }
}
fn reference_fingerprint(issue: &local::RuntimeIssue, context: &str) -> String {
    // The document digest is display evidence. Only the meaning of this
    // reference, rather than an unrelated edit elsewhere, invalidates a choice.
    let text = context.to_lowercase();
    let usage = [
        "生成",
        "创建",
        "写入",
        "输出文件",
        "creates",
        "generated",
        "write ",
    ]
    .iter()
    .any(|word| text.contains(word));
    digest(
        format!(
            "{}\0{}\0{}\0{}\0{}",
            issue.source,
            issue.target,
            issue.category,
            inferred_role(issue, context),
            usage
        )
        .as_bytes(),
    )
}
fn resource_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| format!("无法读取外部文件：{error}"))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("只能纳入明确选定的普通文件，不能打包整个目录".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_RESOURCE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_RESOURCE {
        return Err("外部文件超过 4 MiB，未加入运行包".into());
    }
    Ok(bytes)
}

pub fn inspect_issues(
    skill: &Value,
    root: &Path,
    hashes: &BTreeMap<String, String>,
    issues: &[local::RuntimeIssue],
) -> Vec<Dependency> {
    inspect_issues_with_reader(skill, Some(root), hashes, issues, |source| {
        source_text(root, source)
    })
}

fn inspect_issues_with_reader(
    skill: &Value,
    root: Option<&Path>,
    hashes: &BTreeMap<String, String>,
    issues: &[local::RuntimeIssue],
    mut read: impl FnMut(&str) -> String,
) -> Vec<Dependency> {
    let mut seen = BTreeSet::new();
    let mut source_cache = BTreeMap::new();
    let root_key = root.map(key);
    issues.iter().filter(|issue| matches!(issue.category.as_str(), "external" | "history" | "command" | "package_required")
        || (issue.category=="uncertain" && (issue.target.contains('/')||issue.target.contains('.')))).filter_map(|issue| {
        let id = digest(format!("{}\0{}", issue.source, issue.target).as_bytes());
        if !seen.insert(id.clone()) { return None; }
        let source_text = source_cache.entry(issue.source.clone()).or_insert_with(|| read(&issue.source));
        let context = source_context(source_text, &issue.target);
        let source_hash = hashes.get(&issue.source).cloned().unwrap_or_default();
        let reference_fingerprint = reference_fingerprint(issue, &context);
        let binding = skill["externalBindings"].as_array().into_iter().flatten().find(|value| value["id"] == id);
        let binding_stale = binding.is_some_and(|value| value["referenceFingerprint"] != reference_fingerprint);
        let active = binding;
        let role = active.and_then(|value| value["role"].as_str()).unwrap_or_else(|| inferred_role(issue, &context)).to_owned();
        let target = active.filter(|value| value["sourceKind"]!="github").and_then(|value| value["path"].as_str()).unwrap_or(&issue.target);
        let path = root.and_then(|root| target_path(root, &issue.source, target)
            .or_else(|| if !absolute_target(target) {local::safe_join(root,target).ok()} else {None}));
        let mut item = Dependency { id, source:issue.source.clone(),target:issue.target.clone(),source_hash,reference_fingerprint,
            role, status:"environment".into(),reason:"运行输入或环境约定，无需随 Skill 复制；实际调用另行验证".into(),
            canonical_path:None,kind:None,hash:None,package_path:None,required:false,default_included:false,portable:false,
            exists:false,readable:false,checked_at:now(),binding_stale,
            reference_kind:if issue.category=="command" {"command"} else {"file"}.into(),
            local:Value::Null,remote:json!({"status":if skill["sourceId"].is_string(){"not_checked"}else{"not_linked"}}),
            effective_source:None,candidates:vec![] };
        item.required = item.role == "required";
        if item.role == "history" { item.status="reference".into(); item.reason="维护、历史或来源信息，不属于缺失的运行文件".into(); }
        if item.reference_kind=="command" {
            item.role="environment".into();item.required=false;item.status="environment".into();
            item.reason=if issue.reason.starts_with("脚本调用") {
                "脚本入口已单独核对；调用参数不是静态文件，无需绑定或加入 ZIP"
            } else {
                "客户端斜杠命令，不是文件路径；无需绑定文件或加入 ZIP"
            }.into();
            item.local=json!({"status":"not_applicable"});return Some(item);
        }
        let Some(path) = path else {
            item.local=json!({"status":if root.is_some(){"not_applicable"}else{"not_installed"}});return Some(item);
        };
        if (target.starts_with("\\\\")||target.starts_with("//"))&&active.is_none() {
            item.role="environment".into();item.required=false;
            item.reason="网络共享路径作为环境要求，不在后台自动读取；可绑定到本机具体文件".into();
            return Some(item);
        }
        match fs::canonicalize(&path) {
            Err(error) => {
                if item.role != "history" { item.status=if error.kind()==std::io::ErrorKind::NotFound {"missing"} else {"inaccessible"}.into(); item.reason=if error.kind()==std::io::ErrorKind::NotFound {"引用路径当前不存在".into()} else {format!("引用路径暂不可访问：{error}")}; }
            },
            Ok(resolved) => {
                item.exists=true; item.canonical_path=Some(display(&resolved));
                let metadata=fs::metadata(&resolved);
                if let Ok(metadata)=metadata {
                    item.kind=Some(if metadata.is_dir(){"directory"}else if metadata.is_file(){"file"}else{"other"}.into());
                    let resolved_key=key(&resolved);
                    let source_binding = issue.category=="package_required" && active.is_some()
                        && root.and_then(|root|local::safe_join(root,&issue.source).ok())
                            .and_then(|path|fs::canonicalize(path).ok())
                            .is_some_and(|source|key(&source)==resolved_key);
                    if source_binding {
                        item.status="needs_selection".into();item.required=true;item.role="required".into();
                        item.reason="绑定的是引用说明本身，不能代替缺失的目标文件；请绑定实际资源文件".into();
                    } else if root_key.as_ref().is_some_and(|root|resolved_key==*root || resolved_key.starts_with(&format!("{root}/"))) {
                        item.reference_kind=if metadata.is_file(){"package_file"}else{"skill_alias"}.into();
                        item.status="available".into(); item.readable=true;item.effective_source=Some("local".into());
                        if metadata.is_file() {
                            if let Some(root)=root {item.package_path=resolved.strip_prefix(root).ok().map(|path|display(path).replace('\\',"/"));}
                            item.hash=item.package_path.as_ref().and_then(|path|hashes.get(path).cloned());
                            item.default_included=item.required;
                            item.reason="引用指向当前 Skill 包内文件，按用途随包，使用原包内路径且不重复打包".into();
                        } else {
                            item.required=false;item.role="environment".into();
                            item.reason="链接指向当前 Skill 原目录，属于同一份来源，不重复打包目录".into();
                        }
                    } else if item.role=="history" { /* metadata only; never enumerate a history directory */ }
                    else if active.is_none() && metadata.is_file() && root.is_some_and(|root|other_skill_file(&resolved,root)) {
                        item.role="environment".into();item.required=false;item.status="environment".into();
                        item.reason="此文件属于另一项 Skill，作为环境依赖显示；需要单独随包时请明确绑定具体文件与用途".into();
                    }
                    else if private_resource(&resolved) {
                        item.role="environment".into(); item.required=false; item.status="environment".into();
                        item.reason="凭据或机器私有配置仅作为环境要求，不读取内容或加入分享包".into();
                    } else if metadata.is_dir() {
                        item.reference_kind="directory".into();
                        item.readable=fs::read_dir(&resolved).is_ok();
                        item.status=if item.required {"needs_selection"}else if item.readable{"available"}else{"inaccessible"}.into();
                        item.reason=if item.required {"目录不能整体纳入，请绑定到实际必需的单个文件"}else{"目录位置已核实；这是环境或维护入口，不递归扫描或打包目录"}.into();
                    } else if metadata.is_file() {
                        item.readable=fs::File::open(&resolved).is_ok();
                        item.status=if item.readable{"available"}else{"inaccessible"}.into();
                        item.reason=if item.readable{"本机文件可读；是否随包取决于依赖用途，实际调用另行验证"}else{"文件存在，但当前无法读取"}.into();
                        if item.readable && matches!(item.role.as_str(),"required"|"optional") {
                            match resource_bytes(&resolved) {
                                Ok(bytes)=> {
                                    item.hash=Some(digest(&bytes));
                                    let name=resolved.file_name().and_then(|name|name.to_str()).unwrap_or("resource");
                                    let resource_id=digest(resolved_key.as_bytes());
                                    let package_path=format!("external/{}/{}",&resource_id[..12],name);
                                    if root.is_some_and(|root|local::safe_join(root,&package_path).is_ok()) {
                                        let (rewritten,changed)=rewrite_prose(source_text,&item.target,&relative_reference(&item.source,&package_path));
                                        item.portable=matches!(Path::new(&item.source).extension().and_then(|ext|ext.to_str()),Some("md"|"mdx"|"txt"|"rst"))
                                            &&changed&&!normalized(&rewritten).contains(&normalized(&item.target));
                                        item.package_path=Some(package_path);item.default_included=item.required;
                                        item.reason=if item.required{"必要外部文件将直接加入 ZIP，原件保持在现有位置"}else{"可选外部文件默认不导出，勾选后直接加入 ZIP"}.into();
                                    } else { item.status="unsupported".into(); item.reason="外部文件名无法安全用于分享包".into(); }
                                },
                                Err(error)=>{item.status="unsupported".into();item.reason=error;}
                            }
                        }
                    }
                } else {item.status="inaccessible".into();item.reason="无法读取引用路径的文件属性".into();}
            }
        }
        if item.binding_stale { item.reason=format!("此引用的用途已变化，请重新确认此前选择的依赖用途；{}",item.reason); }
        let target_changed = active.and_then(|value| value["canonicalPath"].as_str())
            .is_some_and(|path| item.canonical_path.as_deref().is_some_and(|current| key(Path::new(path)) != key(Path::new(current))));
        if target_changed { item.binding_stale=true;item.reason="绑定的链接目标已变化，请重新确认依赖用途；原文件未复制或移动".into(); }
        if item.binding_stale && matches!(item.role.as_str(),"required"|"optional") {item.status="needs_review".into();item.default_included=false;}
        item.local=json!({"status":item.status,"path":item.canonical_path,"exists":item.exists,"readable":item.readable,
            "kind":item.kind,"hash":item.hash,"checkedAt":item.checked_at});
        if item.status=="available" && item.effective_source.is_none() {item.effective_source=Some("local".into());}
        Some(item)
    }).collect()
}

/// Package validity, row warnings and export eligibility use the same resolved
/// reference. An available explicit replacement satisfies a missing literal;
/// the original scanner result remains evidence in the audit's rawIssues.
pub(crate) fn apply_resolutions(selection: &mut local::RuntimeFileSelection, items: &[Dependency]) {
    for issue in &mut selection.issues {
        if !matches!(issue.category.as_str(), "package_required" | "uncertain") {
            continue;
        }
        let Some(item) = items.iter().find(|item| {
            item.source == issue.source
                && item.target == issue.target
                && item.status == "available"
                && item.required
                && !item.binding_stale
                && item.readable
                && item.kind.as_deref() == Some("file")
                && item.package_path.is_some()
        }) else {
            continue;
        };
        let original = format!("{}：{} → {}", issue.reason, issue.source, issue.target);
        issue.category = "resolved".into();
        issue.severity = "info".into();
        issue.reason = format!(
            "引用已由核验后的{}文件满足",
            if item.effective_source.as_deref() == Some("github") {
                " GitHub "
            } else {
                "绑定"
            }
        );
        let resolved = format!("{}：{} → {}", issue.reason, issue.source, issue.target);
        for warning in &mut selection.warnings {
            if *warning == original {
                *warning = resolved.clone();
            }
        }
        if item.reference_kind == "package_file"
            && item.effective_source.as_deref() == Some("local")
        {
            if let Some(path) = &item.package_path {
                if !selection.required_files.contains(path) {
                    selection.required_files.push(path.clone());
                }
                if !selection.suggested_files.contains(path) {
                    selection.suggested_files.push(path.clone());
                }
                selection.optional_files.retain(|optional| optional != path);
                selection
                    .required_reasons
                    .entry(path.clone())
                    .or_insert_with(|| format!("满足 {} 的已绑定引用", issue.source));
            }
        }
    }
    selection.required_files.sort();
    selection.suggested_files.sort();
    selection.issues.sort();
    selection.issues.dedup();
}

fn binding<'a>(skill: &'a Value, id: &str) -> Option<&'a Value> {
    skill["externalBindings"]
        .as_array()?
        .iter()
        .find(|value| value["id"] == id)
}

// Resolution context only: never persist client paths as Skill configuration.
pub(crate) fn resolution_skill(m: &Manager, skill: &Value) -> Value {
    let mut result = skill.clone();
    result["_externalAliasRoots"] = json!(m
        .state
        .connections
        .iter()
        .filter(|entry| entry["skillId"] == skill["id"] && entry["linked"] == true)
        .filter_map(|entry| entry["path"].as_str())
        .collect::<Vec<_>>());
    result
}

fn explicit_mapping(value: &Value) -> bool {
    value["automatic"] != true && (value["path"].is_string() || value["repositoryPath"].is_string())
}

fn same_repo(left: &str, right: &str) -> bool {
    // GitHub owner/repository names are case insensitive; Git paths are not.
    left.eq_ignore_ascii_case(right)
}

/// Even a missing leaf has an exact repository identity below a verified root.
/// Interior reparse points and lexical traversal are rejected by safe_join.
fn absolute_package_reference(skill: &Value, target: &str) -> Option<String> {
    if !absolute_target(target) || target.starts_with("//") || target.starts_with("\\\\") {
        return None;
    }
    let registered = Path::new(skill["localPath"].as_str()?);
    crate::transaction::verify_root(registered).ok()?;
    let root = local::canonical(registered).ok()?;
    let mut anchors = vec![display(&root), display(registered)];
    anchors.extend(
        skill["_externalAliasRoots"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|alias| {
                local::canonical(Path::new(alias))
                    .is_ok_and(|resolved| key(&resolved) == key(&root))
            })
            .map(str::to_owned),
    );
    let original = normalized(target);
    for anchor in anchors {
        let anchor = normalized(&anchor).trim_end_matches('/').to_owned();
        if key(Path::new(&original)).starts_with(&format!("{}/", key(Path::new(&anchor)))) {
            let suffix = original.get(anchor.len() + 1..)?;
            local::safe_join(&root, suffix).ok()?;
            return Some(suffix.to_owned());
        }
    }
    None
}

fn repo_reference(base: &str, target: &str) -> Option<String> {
    if absolute_target(target) || target.contains(':') {
        return None;
    }
    let mut parts = base
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let target = normalized(target);
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part.to_owned()),
        }
    }
    let path = parts.join("/");
    crate::remote::github::validate_relative(&path).ok()?;
    Some(path)
}

fn safe_remote_resource(path: &str, entry: &Entry) -> bool {
    crate::remote::github::validate_relative(path).is_ok()
        && matches!(entry.mode.as_str(), "100644" | "100755")
        && entry.size <= MAX_RESOURCE as u64
        && !private_resource(Path::new(path))
        && !private_resource(Path::new(&entry.source))
        && !path.split('/').any(|part| {
            matches!(
                part,
                ".git" | ".skill-manager" | ".skill-manager-state.json"
            )
        })
}

fn other_remote_skill(path: &str, prefix: &str, snap: &Snapshot) -> bool {
    Path::new(path)
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .any(|parent| {
            let root = display(parent).replace('\\', "/");
            root != prefix
                && snap.tree.contains_key(&if root.is_empty() {
                    "SKILL.md".into()
                } else {
                    format!("{root}/SKILL.md")
                })
        })
}

/// Resolve proven package roots, relative paths and unique current content in
/// one immutable repository snapshot; basename matches remain suggestions.
fn enrich_remote(skill: &Value, items: &mut [Dependency], snap: &Snapshot) {
    let prefix = skill["remotePath"].as_str().unwrap_or("");
    let mut read_objects = BTreeSet::new();
    let mut read_total = 0usize;
    let mut local_proofs = BTreeMap::<String, Option<(String, String, u64)>>::new();
    let mut proof_total = 0usize;
    for item in items {
        if matches!(item.reference_kind.as_str(), "command" | "skill_alias") {
            item.remote = json!({"status":"not_applicable"});
            continue;
        }
        let saved = binding(skill, &item.id);
        let explicit = saved.and_then(|value| value["repositoryPath"].as_str());
        if item.required
            && explicit
                .is_some_and(|path| repo_reference(prefix, &item.source).as_deref() == Some(path))
        {
            item.status = "needs_selection".into();
            item.default_included = false;
            item.remote = json!({"status":"needs_selection","commit":snap.commit});
            item.reason = "GitHub 绑定指向引用说明本身，不能代替缺失的目标文件".into();
            continue;
        }
        let mapped_repo = saved.and_then(|value| value["repo"].as_str());
        if mapped_repo.is_some_and(|repo| {
            !skill["repo"]
                .as_str()
                .is_some_and(|current| same_repo(repo, current))
        }) {
            item.binding_stale = true;
            item.status = "needs_review".into();
            item.reason = "关联仓库已变化，请重新确认此文件映射".into();
            item.remote = json!({"status":"needs_selection","commit":snap.commit});
            continue;
        }
        if let Some(pinned) = saved
            .filter(|value| explicit_mapping(value) && value["sourceKind"] == "github")
            .and_then(|value| value["remoteCommit"].as_str())
            .filter(|pinned| !pinned.eq_ignore_ascii_case(&snap.commit))
        {
            // A current snapshot cannot prove bytes from a user's older fixed
            // commit. Keep that selection and ask before adopting current bytes.
            item.remote = json!({"status":"needs_selection","repo":skill["repo"],"commit":snap.commit,
                "pinnedCommit":pinned,"repositoryPath":explicit,"requiresCommitConfirmation":true,
                "readable":false,"checkedAt":now()});
            if let Some((path, entry)) = explicit.and_then(|path| {
                snap.tree
                    .get(path)
                    .filter(|entry| safe_remote_resource(path, entry))
                    .map(|entry| (path, entry))
            }) {
                item.candidates = vec![
                    json!({"source":"github","repo":skill["repo"],"repositoryPath":path,
                    "commit":snap.commit,"gitBlobSha":entry.sha,"size":entry.size,"exact":false,
                    "requiresCommitConfirmation":true}),
                ];
            }
            item.status = "needs_selection".into();
            item.default_included = false;
            item.effective_source = None;
            item.reason =
                "保留人工固定提交；当前核验提交不同，请确认使用的提交与文件后再导出".into();
            continue;
        }
        let mut exact = BTreeSet::new();
        let mut match_kind = "relative_path";
        if let Some(path) = explicit {
            exact.insert(path.to_owned());
            match_kind = if saved.is_some_and(|value| value["automatic"] == true) {
                saved
                    .and_then(|value| value["matchKind"].as_str())
                    .unwrap_or("confirmed_mapping")
            } else {
                "manual"
            };
        } else if item.reference_kind == "package_file" {
            match_kind = "package_path";
            if let Some(path) = item.package_path.as_deref() {
                if let Some(path) = repo_reference(prefix, path) {
                    exact.insert(path);
                }
            }
        } else if let Some(path) = absolute_package_reference(skill, &item.target) {
            match_kind = "package_path";
            if let Some(path) = repo_reference(prefix, &path) {
                exact.insert(path);
            }
        } else if !absolute_target(&item.target) {
            let source = format!(
                "{}{}{}",
                prefix,
                if prefix.is_empty() { "" } else { "/" },
                item.source
            );
            let parent = source
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .unwrap_or("");
            for base in [parent, prefix] {
                if let Some(path) = repo_reference(base, &item.target) {
                    exact.insert(path);
                }
            }
        }
        let mut matches = exact
            .iter()
            .filter_map(|path| {
                snap.tree
                    .get(path)
                    .filter(|entry| safe_remote_resource(path, entry))
                    .map(|entry| (path.clone(), entry))
            })
            .collect::<Vec<_>>();
        // Git tree hashes let us identify a unique identical object without
        // downloading every candidate. Never use a cached hash of a missing file.
        if matches.len() != 1 || match_kind == "content" {
            let current = item
                .canonical_path
                .as_deref()
                .filter(|path| {
                    item.local["readable"] == true
                        && item.kind.as_deref() == Some("file")
                        && !private_resource(Path::new(path))
                        && item.role != "history"
                        && skill["localPath"]
                            .as_str()
                            .is_some_and(|root| !other_skill_file(Path::new(path), Path::new(root)))
                })
                .and_then(|path| {
                    local_proofs
                        .entry(path.to_owned())
                        .or_insert_with(|| {
                            let bytes = resource_bytes(Path::new(path)).ok()?;
                            proof_total = proof_total
                                .checked_add(bytes.len())
                                .filter(|size| *size <= MAX_RESOURCES)?;
                            Some((
                                digest(&bytes),
                                crate::remote::github::blob_hash(&bytes),
                                bytes.len() as u64,
                            ))
                        })
                        .clone()
                });
            if let Some((hash, blob, size)) = current {
                item.local["hash"] = json!(hash);
                let identical = snap
                    .tree
                    .iter()
                    .filter(|(_, entry)| entry.sha == blob && entry.size == size)
                    .collect::<Vec<_>>();
                if matches.len() != 1
                    && identical.len() == 1
                    && (explicit.is_none() || saved.is_some_and(|value| value["automatic"] == true))
                {
                    let (path, entry) = identical[0];
                    if safe_remote_resource(path, entry)
                        && !other_remote_skill(path, prefix, snap)
                        && !private_resource(Path::new(&item.target))
                    {
                        exact.clear();
                        exact.insert(path.clone());
                        matches = vec![(path.clone(), entry)];
                        match_kind = "content";
                    }
                }
            }
        }
        let selected = if matches.len() == 1 {
            Some(matches[0].clone())
        } else {
            None
        };
        let leaf = normalized(&item.target)
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_owned();
        let mut candidates = snap
            .tree
            .iter()
            .filter(|(path, entry)| {
                safe_remote_resource(path, entry)
                    && (exact.contains(*path)
                        || (!leaf.is_empty() && path.rsplit('/').next() == Some(leaf.as_str())))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(path, _)| (!exact.contains(*path), *path));
        let candidate_count = candidates.len();
        item.candidates=candidates.into_iter().take(40).map(|(path,entry)|json!({"source":"github","repo":skill["repo"],"repositoryPath":path,"commit":snap.commit,"gitBlobSha":entry.sha,"size":entry.size,"exact":exact.contains(path)})).collect();
        let Some((path, entry)) = selected else {
            let restricted = exact.iter().any(|path| snap.tree.contains_key(path));
            item.remote = json!({"status":if restricted&&matches.is_empty(){"unsupported"}else if matches.len()>1||exact.is_empty(){"needs_selection"}else{"missing"},
                "repo":skill["repo"],"commit":snap.commit,"checkedAt":now(),"checkedPaths":exact,
                "candidateCount":candidate_count,"candidatesTruncated":candidate_count>40});
            if item.effective_source.is_none()
                && !item.candidates.is_empty()
                && matches!(item.role.as_str(), "required" | "optional")
            {
                item.status = "needs_selection".into();
                item.reason =
                    "关联仓库存在候选文件，请确认完整仓库路径；不会按文件名自动绑定".into();
            }
            continue;
        };
        if !saved.is_some_and(explicit_mapping) && other_remote_skill(&path, prefix, snap) {
            item.status = "environment".into();
            item.remote = json!({"status":"available","repo":skill["repo"],"repositoryPath":path,"commit":snap.commit,"checkedAt":now()});
            item.reason =
                "关联仓库中的此文件属于另一项 Skill，不自动打包；需要时请明确绑定普通文件与用途"
                    .into();
            continue;
        }
        item.remote = json!({"status":"available","repo":skill["repo"],"repositoryPath":path,"commit":snap.commit,
            "gitBlobSha":entry.sha,"size":entry.size,"checkedAt":now(),"readable":false,"matchKind":match_kind,
            "candidateCount":candidate_count,"candidatesTruncated":candidate_count>40});
        if !matches!(item.role.as_str(), "required" | "optional") {
            continue;
        }
        let bytes = (|| {
            if read_objects.insert(entry.sha.clone()) {
                read_total = read_total
                    .checked_add(entry.size as usize)
                    .filter(|total| *total <= MAX_RESOURCES)
                    .ok_or("外部资源核验超过 32 MiB，请减少本次范围")?;
            }
            snap.read_entry(entry)
        })();
        match bytes {
            Ok(bytes) => {
                item.remote["readable"] = json!(true);
                item.remote["hash"] = json!(digest(&bytes));
                let prefer_remote = saved.is_some_and(|value| {
                    value["sourceKind"] == "github" && value["automatic"] != true
                });
                if prefer_remote || item.status != "available" || item.effective_source.is_none() {
                    item.effective_source = Some("github".into());
                    item.exists = true;
                    item.readable = true;
                    item.kind = Some("file".into());
                    item.hash = Some(digest(&bytes));
                    item.status = "available".into();
                    let package = path
                        .strip_prefix(&format!("{prefix}/"))
                        .filter(|_| !prefix.is_empty())
                        .unwrap_or_else(|| if prefix.is_empty() { &path } else { "" });
                    item.package_path = Some(if !package.is_empty() {
                        package.to_owned()
                    } else {
                        format!(
                            "external/{}/{}",
                            &digest(format!("{}:{path}", skill["repo"]).as_bytes())[..12],
                            path.rsplit('/').next().unwrap_or("resource")
                        )
                    });
                    item.default_included = item.required;
                    item.portable = matches!(
                        Path::new(&item.source)
                            .extension()
                            .and_then(|ext| ext.to_str()),
                        Some("md" | "mdx" | "txt" | "rst")
                    );
                    item.reason = if item.required {
                        "必要文件由关联 GitHub 固定提交提供，默认直接进入 ZIP，不生成本地副本"
                    } else {
                        "可选文件由关联 GitHub 固定提交提供，勾选后直接进入 ZIP"
                    }
                    .into();
                }
            }
            Err(error) => {
                item.remote["status"] = json!("error");
                item.remote["error"] = json!(error);
                if item.effective_source.is_none() {
                    item.reason = format!("本地不可用，GitHub 文件读取失败：{error}");
                }
            }
        }
        if item.binding_stale {
            item.status = "needs_review".into();
            item.default_included = false;
        }
    }
}

pub(crate) fn merged_classification(
    root: Option<&Path>,
    local_hashes: &BTreeMap<String, String>,
    snap: &Snapshot,
    remote_path: &str,
    selected: Option<&BTreeSet<String>>,
) -> Result<local::RuntimeFileSelection, String> {
    let entries = snap.skill_files(remote_path)?;
    let mut all = entries
        .iter()
        .filter(|(path, entry)| safe_remote_resource(path, entry))
        .map(|(path, entry)| (path.clone(), entry.sha.clone()))
        .collect::<BTreeMap<_, _>>();
    all.extend(local_hashes.clone());
    let mut total = 0usize;
    local::runtime_file_selection_with_reader(&all, selected, |path| {
        if local_hashes.contains_key(path) {
            return local::read_text(&local::safe_join(root.ok_or("本地路径不可用")?, path)?);
        }
        let entry = entries
            .get(path)
            .ok_or_else(|| format!("引用文件缺失：{path}"))?;
        total = total
            .checked_add(entry.size as usize)
            .filter(|total| *total <= MAX_RESOURCES)
            .ok_or("远端依赖核验超过 32 MiB")?;
        String::from_utf8(snap.read_entry(entry)?)
            .map_err(|_| format!("引用文件不是 UTF-8 文本：{path}"))
    })
}

pub(crate) fn inspect_snapshot(
    skill: &Value,
    root: Option<&Path>,
    hashes: &BTreeMap<String, String>,
    issues: &[local::RuntimeIssue],
    snap: &Snapshot,
) -> Vec<Dependency> {
    let prefix = skill["remotePath"].as_str().unwrap_or("");
    let mut items = inspect_issues_with_reader(skill, root, hashes, issues, |source| {
        if hashes.contains_key(source) {
            return root
                .map(|root| source_text(root, source))
                .unwrap_or_default();
        }
        repo_reference(prefix, source)
            .and_then(|path| snap.read(&path).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default()
    });
    enrich_remote(skill, &mut items, snap);
    items
}

fn inspect_linked(
    m: &Manager,
    skill: &Value,
    commit: Option<&str>,
    include_remote: bool,
) -> Result<Vec<Dependency>, String> {
    let context = resolution_skill(m, skill);
    let skill = &context;
    let root = skill["localPath"]
        .as_str()
        .map(Path::new)
        .map(|path| {
            crate::transaction::verify_root(path)?;
            local::canonical(path)
        })
        .transpose()?;
    let hashes = root
        .as_ref()
        .map(|root| local::files(root))
        .transpose()?
        .unwrap_or_default();
    let local_selection = root
        .as_ref()
        .map(|root| local::runtime_file_selection(root, &hashes, None))
        .transpose()?;
    let mut issues = local_selection
        .as_ref()
        .map(|selection| selection.issues.clone())
        .unwrap_or_default();
    if include_remote && skill["sourceId"].is_string() {
        match crate::remote::dependency_snapshot(m, skill, commit) {
            Ok(snap) => {
                let selected = local_selection
                    .as_ref()
                    .map(|selection| selection.suggested_files.iter().cloned().collect());
                if let Ok(merged) = merged_classification(
                    root.as_deref(),
                    &hashes,
                    &snap,
                    skill["remotePath"].as_str().unwrap_or(""),
                    selected.as_ref(),
                ) {
                    issues.extend(merged.issues);
                    issues.sort();
                    issues.dedup();
                }
                return Ok(inspect_snapshot(
                    skill,
                    root.as_deref(),
                    &hashes,
                    &issues,
                    &snap,
                ));
            }
            Err(error) => {
                let mut items = inspect_issues_with_reader(
                    skill,
                    root.as_deref(),
                    &hashes,
                    &issues,
                    |source| {
                        root.as_ref()
                            .map(|root| source_text(root, source))
                            .unwrap_or_default()
                    },
                );
                for item in &mut items {
                    item.remote = json!({"status":"error","error":error,"checkedAt":now()});
                }
                return Ok(items);
            }
        }
    }
    Ok(inspect_issues_with_reader(
        skill,
        root.as_deref(),
        &hashes,
        &issues,
        |source| {
            root.as_ref()
                .map(|root| source_text(root, source))
                .unwrap_or_default()
        },
    ))
}

pub fn report(items: &[Dependency], skill_id: &Value) -> Value {
    json!({"skillId":skill_id,"items":items,"summary":{
        "total":items.len(),"available":items.iter().filter(|item|item.status=="available").count(),
        "required":items.iter().filter(|item|item.required).count(),
        "blocked":items.iter().filter(|item|item.required&&item.status!="available").count(),
        "invocationStatus":"not_verified"}})
}
#[cfg(test)]
fn inspect(skill: &Value) -> Result<Vec<Dependency>, String> {
    let registered = Path::new(
        skill["localPath"]
            .as_str()
            .ok_or("此 Skill 尚无本地运行目录")?,
    );
    crate::transaction::verify_root(registered)?;
    let root = local::canonical(registered)?;
    let hashes = local::files(&root)?;
    let classification = local::runtime_file_selection(&root, &hashes, None)?;
    Ok(inspect_issues(
        skill,
        &root,
        &hashes,
        &classification.issues,
    ))
}

fn auto_match(
    m: &mut Manager,
    req: &Value,
    mut skill: Value,
    items: Vec<Dependency>,
) -> Result<Value, String> {
    if skill["readOnly"] == true {
        return Err("托管的只读 Skill 不能保存自动映射".into());
    }
    let updates = match req.get("items") {
        Some(value) => value.as_array().cloned().ok_or("请选择外部引用")?,
        None => items
            .iter()
            .map(|item| json!({"id":item.id,"expectedFingerprint":item.reference_fingerprint}))
            .collect(),
    };
    if updates.len() > 1000 {
        return Err("一次最多匹配 1000 项外部引用".into());
    }
    let original = skill["externalBindings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut bindings = original.clone();
    let mut results = vec![];
    let mut changed_ids = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let commit = req["remoteCommit"]
        .as_str()
        .or_else(|| items.iter().find_map(|item| item.remote["commit"].as_str()));
    for update in updates {
        let id = update["id"].as_str().unwrap_or("");
        let outcome = (|| -> Result<Value, String> {
            if !seen.insert(id.to_owned()) {
                return Err("同一引用在批次中重复".into());
            }
            let item = items
                .iter()
                .find(|item| item.id == id)
                .ok_or("引用已移除，请刷新后重试")?;
            if update["expectedFingerprint"].as_str() != Some(item.reference_fingerprint.as_str()) {
                return Err("引用用途已变化，本项未修改".into());
            }
            let result =
                |status: &str, reason: &str| json!({"id":id,"status":status,"reason":reason});
            if matches!(item.reference_kind.as_str(), "command" | "skill_alias") {
                return Ok(result(
                    "matched",
                    "这是命令或当前 Skill 目录入口，无需匹配文件",
                ));
            }
            if item.binding_stale {
                return Ok(result(
                    "needs_selection",
                    "引用含义或目标已变化，请确认已有映射",
                ));
            }
            if item.remote["requiresCommitConfirmation"] == true {
                return Ok(result("needs_selection", &item.reason));
            }
            if original
                .iter()
                .find(|value| value["id"] == id)
                .is_some_and(explicit_mapping)
            {
                return Ok(result(
                    if item.status == "available" || item.remote["status"] == "available" {
                        "matched"
                    } else {
                        "needs_selection"
                    },
                    "保留手动映射，不自动替换",
                ));
            }
            if item.remote["status"] == "error" {
                return Ok(result(
                    "error",
                    item.remote["error"]
                        .as_str()
                        .unwrap_or("GitHub 读取失败，本地文件保留"),
                ));
            }
            if item.remote["status"] != "available" {
                return Ok(result(
                    if item.remote["status"] == "missing" {
                        "missing"
                    } else if item.remote["status"] == "unsupported" {
                        "error"
                    } else {
                        "needs_selection"
                    },
                    if item.remote["status"] == "missing" {
                        "该固定提交没有对应文件"
                    } else if item.remote["status"] == "unsupported" {
                        "对应路径不是可安全采用的普通资源文件"
                    } else {
                        "尚无可证明的唯一对应关系；仅同名不会自动采用"
                    },
                ));
            }
            if item.kind.as_deref() == Some("directory")
                || private_resource(Path::new(&item.target))
            {
                return Ok(result(
                    "needs_selection",
                    "目录或私有配置不能自动作为普通资源绑定",
                ));
            }
            let path = item.remote["repositoryPath"]
                .as_str()
                .ok_or("缺少已核验的仓库路径")?;
            let remote_commit = item.remote["commit"].as_str().ok_or("缺少固定提交")?;
            if Some(remote_commit) != commit || item.remote["repo"] != skill["repo"] {
                return Err("关联仓库或固定提交已变化，请重新匹配".into());
            }
            let match_kind = item.remote["matchKind"]
                .as_str()
                .ok_or("此项只有候选，尚未确认对应关系")?;
            let mut value = json!({"id":id,"source":item.source,"target":item.target,"sourceHash":item.source_hash,
                "referenceFingerprint":item.reference_fingerprint,"role":item.role,"automatic":true,"matchKind":match_kind,
                "sourceKind":"github","repo":skill["repo"],"repositoryPath":path,"remoteCommit":remote_commit,
                "canonicalPath":item.canonical_path,"hash":item.local["hash"],"boundAt":now()});
            if let Some(old) = bindings.iter().find(|old| old["id"] == id) {
                if [
                    "source",
                    "target",
                    "sourceHash",
                    "referenceFingerprint",
                    "role",
                    "automatic",
                    "matchKind",
                    "sourceKind",
                    "repo",
                    "repositoryPath",
                    "remoteCommit",
                    "canonicalPath",
                    "hash",
                ]
                .iter()
                .all(|key| old[*key] == value[*key])
                {
                    return Ok(result("matched", "已确认对应关系，保留现有自动映射"));
                }
                value["boundAt"] = old["boundAt"].clone();
            }
            bindings.retain(|old| old["id"] != id);
            bindings.push(value);
            changed_ids.insert(id.to_owned());
            Ok(
                json!({"id":id,"status":"matched","reason":"已确认对应关系；用途不变，本地优先","repositoryPath":path,"matchKind":match_kind}),
            )
        })();
        results.push(outcome.unwrap_or_else(
            |error| json!({"id":id,"status":"error","reason":error,"error":error}),
        ));
    }
    skill["externalBindings"] = json!(bindings);
    let mut checked = inspect_linked(m, &skill, commit, req["includeRemote"] != false)?;
    let mut rolled_back = false;
    for result in &mut results {
        let id = result["id"].as_str().unwrap_or("").to_owned();
        if !changed_ids.contains(&id) {
            continue;
        }
        let before = items.iter().find(|item| item.id == id);
        let after = checked.iter().find(|item| item.id == id);
        let valid = before.zip(after).is_some_and(|(before, after)| {
            before.reference_fingerprint == after.reference_fingerprint
                && before.source_hash == after.source_hash
                && !after.binding_stale
                && after.remote["repo"] == before.remote["repo"]
                && after.remote["commit"] == before.remote["commit"]
                && after.remote["repositoryPath"] == before.remote["repositoryPath"]
                && after.remote["gitBlobSha"] == before.remote["gitBlobSha"]
                && (before.remote["matchKind"] != "content"
                    || before.local["hash"] == after.local["hash"])
        });
        if !valid {
            result["status"] = json!("error");
            result["reason"] = json!("核验期间引用或文件内容已变化，本项未保存");
            let values = skill["externalBindings"]
                .as_array_mut()
                .expect("bindings array");
            values.retain(|value| value["id"] != id);
            if let Some(old) = original.iter().find(|value| value["id"] == id) {
                values.push(old.clone());
            }
            rolled_back = true;
        }
    }
    if rolled_back {
        checked = inspect_linked(m, &skill, commit, req["includeRemote"] != false)?;
    }
    if skill["externalBindings"] != json!(original) {
        let previous = m.state.clone();
        crate::audit::refresh_local_audit(&mut skill, None);
        m.upsert_skill(skill.clone());
        if let Err(error) = m.save() {
            m.state = previous;
            return Err(error);
        }
    }

    // Report only saved record changes. Resolving a remote resource is not a
    // local file repair or a claim that local and remote contents agree.
    for result in &mut results {
        let id = result["id"].as_str().unwrap_or("").to_owned();
        let matched = result["status"] == "matched";
        let record_updated = matched
            && changed_ids.contains(&id)
            && original.iter().find(|old| old["id"] == id)
                != skill["externalBindings"]
                    .as_array()
                    .and_then(|values| values.iter().find(|value| value["id"] == id));
        result["recordUpdated"] = json!(record_updated);
        result["recordRepaired"] =
            json!(record_updated && original.iter().any(|old| old["id"] == id));
        let current = checked.iter().find(|item| item.id == id);
        let remote_ready = current.is_some_and(|item| {
            !item.binding_stale
                && matches!(item.role.as_str(), "required" | "optional")
                && item.remote["status"] == "available"
                && item.remote["readable"] == true
        });
        result["remoteReady"] = json!(remote_ready);
        result["localPreviewRequired"] =
            json!(remote_ready && current.is_some_and(|item| item.local["readable"] != true));
        result["contentDiffers"] = json!(current.is_some_and(|item| {
            item.local["readable"] == true
                && item.remote["readable"] == true
                && item.local["hash"].is_string()
                && item.remote["hash"].is_string()
                && item.local["hash"] != item.remote["hash"]
        }));
        if record_updated {
            result["reason"] = json!(if result["recordRepaired"] == true {
                "自动关联记录已修复并重新核验；本地内容保持原样"
            } else {
                "已保存可靠的自动关联；本地内容保持原样"
            });
        }
    }
    let summary = json!({"matched":results.iter().filter(|row|row["status"]=="matched").count(),
        "needsSelection":results.iter().filter(|row|row["status"]=="needs_selection").count(),
        "missing":results.iter().filter(|row|row["status"]=="missing").count(),
        "error":results.iter().filter(|row|row["status"]=="error").count(),
        "recordsUpdated":results.iter().filter(|row|row["recordUpdated"]==true).count(),
        "recordsRepaired":results.iter().filter(|row|row["recordRepaired"]==true).count(),
        "remoteReady":results.iter().filter(|row|row["remoteReady"]==true).count(),
        "localPreviewRequired":results.iter().filter(|row|row["localPreviewRequired"]==true).count(),
        "contentDiffers":results.iter().filter(|row|row["contentDiffers"]==true).count()});
    let mut result = report(&checked, &skill["id"]);
    result["results"] = json!(results);
    result["autoMatchSummary"] = summary;
    result["stateSnapshot"] = m.snapshot();
    Ok(result)
}

pub fn handle(m: &mut Manager, req: &Value) -> Result<Option<Value>, String> {
    let action = req["action"].as_str().unwrap_or("");
    if !matches!(
        action,
        "get_external_dependencies"
            | "check_external_dependencies"
            | "bind_external_dependency"
            | "batch_external_dependencies"
    ) {
        return Ok(None);
    }
    let id = req["skillId"].as_str().ok_or("缺少 Skill ID")?;
    let mut skill = m.skill(id)?;
    let items = inspect_linked(
        m,
        &skill,
        req["remoteCommit"].as_str(),
        req["includeRemote"] != false,
    )?;
    let operation = if action == "batch_external_dependencies" {
        req["operation"].as_str().ok_or("请选择批量操作")?
    } else {
        "bind"
    };
    if !matches!(
        operation,
        "bind" | "role" | "remap" | "recheck" | "auto_match"
    ) {
        return Err("不支持的外部文件操作".into());
    }
    if !matches!(
        action,
        "bind_external_dependency" | "batch_external_dependencies"
    ) {
        return Ok(Some(report(&items, &skill["id"])));
    }
    if operation == "auto_match" {
        return auto_match(m, req, skill, items).map(Some);
    }
    if skill["readOnly"] == true && operation != "recheck" {
        return Err("托管的只读 Skill 不能修改依赖绑定".into());
    }
    let updates = if action == "batch_external_dependencies" {
        req["items"].as_array().cloned().ok_or("请选择外部引用")?
    } else {
        vec![
            json!({"id":req["dependencyId"],"expectedFingerprint":req["expectedFingerprint"],"role":req["role"],"path":req["path"],"source":req["source"],"repositoryPath":req["repositoryPath"],"remoteCommit":req["remoteCommit"]}),
        ]
    };
    if updates.is_empty() || updates.len() > 1000 {
        return Err("请选择 1 至 1000 项外部引用".into());
    }
    let previous = m.state.clone();
    let mut bindings = skill["externalBindings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut results = vec![];
    let mut seen = BTreeSet::new();
    for update in updates {
        let id = update["id"].as_str().unwrap_or("");
        let applied = (|| -> Result<Option<Value>, String> {
            if !seen.insert(id.to_owned()) {
                return Err("同一引用在批次中重复".into());
            }
            let item = items
                .iter()
                .find(|item| item.id == id)
                .ok_or("外部引用已变化，请刷新后选择")?;
            let expected = update["expectedFingerprint"].as_str();
            if (action == "batch_external_dependencies" && expected.is_none())
                || expected.is_some_and(|value| value != item.reference_fingerprint)
            {
                return Err("引用用途已变化，本项未修改，请重新检查".into());
            }
            if operation == "recheck" {
                return Ok(None);
            }
            if item.reference_kind == "command" {
                return Err("客户端命令无需绑定文件；请查看引用说明".into());
            }
            if operation == "remap" {
                return Ok(Some(
                    json!({"id":id,"source":item.source,"target":item.target,"sourceHash":item.source_hash,
                    "referenceFingerprint":item.reference_fingerprint,"role":item.role,"sourceKind":"local","boundAt":now()}),
                ));
            }
            let role = update["role"].as_str().unwrap_or(&item.role);
            if !matches!(role, "required" | "optional" | "environment" | "history") {
                return Err("请选择有效的依赖用途".into());
            }
            if operation == "role"
                && item.reference_kind == "skill_alias"
                && matches!(role, "required" | "optional")
            {
                return Err("这是当前 Skill 的目录入口，不能重复打包目录；请定位包内文件".into());
            }
            let old = bindings.iter().find(|value| value["id"] == id);
            let source = update["source"]
                .as_str()
                .or_else(|| {
                    if update["path"].is_string() {
                        Some("local")
                    } else {
                        old.and_then(|old| old["sourceKind"].as_str())
                    }
                })
                .unwrap_or("local");
            if !matches!(source, "local" | "github") {
                return Err("请选择本地或关联 GitHub 来源".into());
            }
            let mut path = update["path"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| old.and_then(|old| old["path"].as_str()).map(str::to_owned));
            let mut repository_path = None;
            let mut remote_commit = None;
            if source == "github" {
                path = None;
                let target = update["repositoryPath"]
                    .as_str()
                    .or_else(|| old.and_then(|old| old["repositoryPath"].as_str()))
                    .ok_or("请选择完整仓库文件路径")?;
                let commit = update["remoteCommit"]
                    .as_str()
                    .or_else(|| old.and_then(|old| old["remoteCommit"].as_str()))
                    .or_else(|| req["remoteCommit"].as_str())
                    .or_else(|| item.remote["commit"].as_str())
                    .ok_or("请先读取固定提交的仓库文件清单")?;
                let snap = crate::remote::dependency_snapshot(m, &skill, Some(commit))?;
                let entry = snap
                    .tree
                    .get(target)
                    .filter(|entry| safe_remote_resource(target, entry))
                    .ok_or("该固定提交没有可绑定的普通资源文件")?;
                if matches!(role, "required" | "optional") {
                    snap.read_entry(entry)?;
                }
                if item.required
                    && repo_reference(skill["remotePath"].as_str().unwrap_or(""), &item.source)
                        .as_deref()
                        == Some(target)
                {
                    return Err("引用说明本身不能代替目标资源，请选择实际需要的文件".into());
                }
                repository_path = Some(target.to_owned());
                remote_commit = Some(commit.to_owned());
            } else if let Some(path) = path.as_deref() {
                if !absolute_target(path) {
                    return Err("绑定文件必须使用完整绝对路径".into());
                }
                let canonical = local::canonical(Path::new(path))?;
                if !canonical.is_file() || private_resource(&canonical) {
                    return Err("只能绑定普通资源文件，不能纳入目录、凭据或机器私有配置".into());
                }
                if item.required
                    && skill["localPath"]
                        .as_str()
                        .and_then(|root| local::safe_join(Path::new(root), &item.source).ok())
                        .and_then(|source| fs::canonicalize(source).ok())
                        .is_some_and(|source| key(&source) == key(&canonical))
                {
                    return Err("引用说明本身不能代替目标资源，请绑定实际需要的文件".into());
                }
            }
            if matches!(role, "required" | "optional")
                && private_resource(Path::new(path.as_deref().unwrap_or(&item.target)))
            {
                return Err("凭据和机器私有配置不能加入分享包".into());
            }
            let mut value = json!({"id":id,"source":item.source,"target":item.target,"sourceHash":item.source_hash,
                "referenceFingerprint":item.reference_fingerprint,"role":role,"path":path,"sourceKind":source,
                "repositoryPath":repository_path,"repo":if source=="github"{skill["repo"].clone()}else{Value::Null},
                "remoteCommit":remote_commit,"boundAt":now()});
            if old.is_some_and(|old| old["automatic"] == true)
                && !update["source"].is_string()
                && !update["path"].is_string()
                && !update["repositoryPath"].is_string()
            {
                value["automatic"] = json!(true);
                value["matchKind"] = old.expect("existing automatic mapping")["matchKind"].clone();
            }
            Ok(Some(value))
        })();
        match applied {
            Ok(value) => {
                if operation != "recheck" {
                    bindings.retain(|value| value["id"] != id);
                    if let Some(value) = value {
                        bindings.push(value);
                    }
                }
                results.push(json!({"id":id,"status":"success"}));
            }
            Err(error) => {
                if action != "batch_external_dependencies" {
                    return Err(error);
                }
                results.push(json!({"id":id,"status":"error","error":error}));
            }
        }
    }
    skill["externalBindings"] = json!(bindings);
    let commit = items
        .iter()
        .find_map(|item| item.remote["commit"].as_str())
        .or_else(|| req["remoteCommit"].as_str());
    let mut checked = inspect_linked(m, &skill, commit, req["includeRemote"] != false)?;
    let mut changed_during_check = false;
    if operation != "recheck" {
        for result in results
            .iter_mut()
            .filter(|result| result["status"] == "success")
        {
            let id = result["id"].as_str().unwrap_or("").to_owned();
            let before = items.iter().find(|item| item.id == id);
            let after = checked.iter().find(|item| item.id == id);
            if before.zip(after).is_none_or(|(before, after)| {
                before.reference_fingerprint != after.reference_fingerprint
            }) {
                if action != "batch_external_dependencies" {
                    return Err("核验期间引用发生变化，未保存过期绑定".into());
                }
                result["status"] = json!("error");
                result["error"] = json!("核验期间引用发生变化，本项未修改");
                let prior = previous
                    .skills
                    .iter()
                    .find(|prior| prior["id"] == skill["id"])
                    .and_then(|prior| binding(prior, &id))
                    .cloned();
                let values = skill["externalBindings"]
                    .as_array_mut()
                    .expect("bindings array");
                values.retain(|value| value["id"] != id);
                if let Some(prior) = prior {
                    values.push(prior);
                }
                changed_during_check = true;
            }
        }
    }
    if changed_during_check {
        checked = inspect_linked(m, &skill, commit, req["includeRemote"] != false)?;
    }
    if let Some(values) = skill["externalBindings"].as_array_mut() {
        for value in values {
            if !results
                .iter()
                .any(|result| result["id"] == value["id"] && result["status"] == "success")
            {
                continue;
            }
            if let Some(item) = checked.iter().find(|item| value["id"] == item.id) {
                value["canonicalPath"] = json!(item.canonical_path);
                value["hash"] = json!(item.hash);
            }
        }
    }
    if operation != "recheck" && results.iter().any(|result| result["status"] == "success") {
        crate::audit::refresh_local_audit(&mut skill, None);
        m.upsert_skill(skill.clone());
        if let Err(error) = m.save() {
            m.state = previous;
            return Err(error);
        }
    }
    let mut result = report(&checked, &skill["id"]);
    result["results"] = json!(results);
    result["stateSnapshot"] = m.snapshot();
    Ok(Some(result))
}

#[derive(Debug)]
pub struct ExportResources {
    pub data: BTreeMap<String, Vec<u8>>,
    pub replacements: BTreeMap<String, Vec<u8>>,
    pub provenance: Value,
}
pub fn sanitized_warnings(warnings: &[String], issues: &[local::RuntimeIssue]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| {
            let mut text = warning.clone();
            for issue in issues
                .iter()
                .filter(|issue| matches!(issue.category.as_str(), "external" | "history"))
            {
                if absolute_target(&issue.target) || issue.target.starts_with("../") {
                    text = text.replace(&issue.target, "[external reference]");
                }
            }
            text
        })
        .collect()
}
fn relative_reference(source: &str, target: &str) -> String {
    let depth = source.matches('/').count();
    format!("{}{target}", "../".repeat(depth))
}
/// Rewrites only exact prose references in the ZIP's copy. Code and dynamic
/// expressions remain explicitly dependent on the original environment.
fn rewrite_prose(text: &str, old: &str, new: &str) -> (String, bool) {
    let mut changed = false;
    let mut fence: Option<char> = None;
    let rewritten = text
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                let marker = trimmed.chars().next().unwrap_or('`');
                if fence == Some(marker) {
                    fence = None;
                } else if fence.is_none() {
                    fence = Some(marker);
                }
                return line.to_owned();
            }
            if fence.is_some() {
                return line.to_owned();
            }
            let mut result = line.to_owned();
            let destination = new
                .as_bytes()
                .iter()
                .map(|byte| {
                    if byte.is_ascii_alphanumeric()
                        || matches!(byte, b'/' | b'~' | b'-' | b'_' | b'.')
                    {
                        (*byte as char).to_string()
                    } else {
                        format!("%{byte:02X}")
                    }
                })
                .collect::<String>();
            for (from, to) in [
                (format!("]({old})"), format!("]({destination})")),
                (format!("](<{old}>)"), format!("](<{destination}>)")),
                (format!("`{old}`"), format!("`{new}`")),
            ] {
                if result.contains(&from) {
                    result = result.replace(&from, &to);
                    changed = true;
                }
            }
            result
        })
        .collect::<Vec<_>>()
        .join("\n");
    (
        if text.ends_with('\n') {
            format!("{rewritten}\n")
        } else {
            rewritten
        },
        changed,
    )
}
fn read_bound_resource(
    skill: &Value,
    root: &Path,
    item: &Dependency,
    read: impl FnOnce(&Path) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    let original = skill["externalBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|binding| binding["id"] == item.id)
        .and_then(|binding| binding["path"].as_str())
        .unwrap_or(&item.target);
    let reference =
        target_path(root, &item.source, original).ok_or("外部文件的原始引用已变化，请重新检查")?;
    let expected = Path::new(item.canonical_path.as_deref().ok_or("外部文件路径未核验")?);
    let before = local::canonical(&reference)?;
    if key(&before) != key(expected) {
        return Err("准备导出时外部引用的链接目标已变化，请重新检查".into());
    }
    let bytes = read(&before)?;
    if Some(digest(&bytes)) != item.hash || key(&local::canonical(&reference)?) != key(&before) {
        return Err("准备导出时外部文件或原始引用的链接目标发生变化，请重新检查".into());
    }
    Ok(bytes)
}
pub fn export_resources(
    skill: &Value,
    runtime: &[String],
    optional_ids: &[String],
) -> Result<ExportResources, String> {
    export_resources_snapshot(skill, runtime, optional_ids, None)
}

/// Derive one-time packaging capability without changing a reference's saved purpose.
/// Metadata-only calls never fetch ordinary environment file contents.
pub(crate) fn export_candidate(
    skill: &Value,
    item: &Dependency,
    snap: Option<&Snapshot>,
    materialize: bool,
) -> Result<Dependency, String> {
    if matches!(
        item.reference_kind.as_str(),
        "command" | "skill_alias" | "directory"
    ) || item.kind.as_deref() == Some("directory")
    {
        return Err("这是命令、目录或环境入口，不作为单个文件加入 ZIP".into());
    }
    if item.binding_stale || item.remote["requiresCommitConfirmation"] == true {
        return Err("引用、绑定或人工固定提交已变化，请先明确确认来源".into());
    }
    if private_resource(Path::new(&item.target)) {
        return Err("凭据或机器私有配置不能加入 ZIP".into());
    }
    if matches!(item.role.as_str(), "required" | "optional") {
        if item.status != "available" || item.package_path.is_none() {
            return Err(item.reason.clone());
        }
        return Ok(item.clone());
    }
    let mut selected = item.clone();
    let root = skill["localPath"]
        .as_str()
        .map(Path::new)
        .map(local::canonical)
        .transpose()?;
    let saved = binding(skill, &item.id);
    let manual = saved.is_some_and(explicit_mapping);
    let prefer_remote =
        saved.is_some_and(|value| value["sourceKind"] == "github" && explicit_mapping(value));
    if !prefer_remote {
        if let Some(path) = item
            .canonical_path
            .as_deref()
            .filter(|_| item.kind.as_deref() == Some("file"))
        {
            let canonical = local::canonical(Path::new(path))?;
            let path = canonical.as_path();
            if private_resource(path) {
                return Err("凭据或机器私有配置不能加入 ZIP".into());
            }
            if !manual
                && root
                    .as_ref()
                    .is_some_and(|root| other_skill_file(path, root))
            {
                return Err("资源属于另一项 Skill，请先明确绑定具体文件".into());
            }
            let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
            if !metadata.is_file() || metadata.len() > MAX_RESOURCE as u64 {
                return Err("外部资源不是普通文件或超过 4 MiB 限制".into());
            }
            fs::File::open(path).map_err(|error| format!("文件暂不可读：{error}"))?;
            selected.package_path = Some(
                root.as_ref()
                    .and_then(|root| path.strip_prefix(root).ok())
                    .map(|relative| display(relative).replace('\\', "/"))
                    .unwrap_or_else(|| {
                        format!(
                            "external/{}/{}",
                            &digest(key(path).as_bytes())[..12],
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("resource")
                        )
                    }),
            );
            selected.hash = materialize
                .then(|| resource_bytes(path).map(|bytes| digest(&bytes)))
                .transpose()?;
            selected.status = "available".into();
            selected.effective_source = Some("local".into());
            selected.readable = true;
            selected.portable = matches!(
                Path::new(&item.source)
                    .extension()
                    .and_then(|ext| ext.to_str()),
                Some("md" | "mdx" | "txt" | "rst")
            );
            return Ok(selected);
        }
    }
    if item.remote["status"] != "available" {
        return Err(item.reason.clone());
    }
    let snap = snap.ok_or("请先核对关联 GitHub 的固定提交")?;
    let path = item.remote["repositoryPath"]
        .as_str()
        .ok_or("GitHub 文件路径尚未明确")?;
    let entry = snap
        .tree
        .get(path)
        .filter(|entry| safe_remote_resource(path, entry))
        .ok_or("远端资源不是可打包的普通文件")?;
    let prefix = skill["remotePath"].as_str().unwrap_or("");
    if !manual && other_remote_skill(path, prefix, snap) {
        return Err("资源属于另一项 Skill，请先明确绑定具体文件".into());
    }
    selected.hash = materialize
        .then(|| snap.read_entry(entry).map(|bytes| digest(&bytes)))
        .transpose()?;
    let package = path
        .strip_prefix(&format!("{prefix}/"))
        .filter(|_| !prefix.is_empty())
        .unwrap_or(if prefix.is_empty() { path } else { "" });
    selected.package_path = Some(if package.is_empty() {
        format!(
            "external/{}/{}",
            &digest(format!("{}:{path}", skill["repo"]).as_bytes())[..12],
            path.rsplit('/').next().unwrap_or("resource")
        )
    } else {
        package.to_owned()
    });
    selected.status = "available".into();
    selected.kind = Some("file".into());
    selected.effective_source = Some("github".into());
    selected.readable = true;
    selected.portable = matches!(
        Path::new(&item.source)
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("md" | "mdx" | "txt" | "rst")
    );
    Ok(selected)
}

pub(crate) fn export_resources_snapshot(
    skill: &Value,
    runtime: &[String],
    optional_ids: &[String],
    snap: Option<&Snapshot>,
) -> Result<ExportResources, String> {
    export_resources_for_plan(skill, runtime, optional_ids, snap, false)
}

pub(crate) fn export_resources_for_plan(
    skill: &Value,
    runtime: &[String],
    optional_ids: &[String],
    snap: Option<&Snapshot>,
    allow_environment: bool,
) -> Result<ExportResources, String> {
    let registered = Path::new(
        skill["localPath"]
            .as_str()
            .ok_or("此 Skill 尚无本地运行目录")?,
    );
    crate::transaction::verify_root(registered)?;
    let root = local::canonical(registered)?;
    let hashes = local::files(&root)?;
    let classification = local::runtime_file_selection(&root, &hashes, None)?;
    let mut issues = classification.issues;
    let mut items = if let Some(snap) = snap {
        let scope = runtime.iter().cloned().collect();
        let merged = merged_classification(
            Some(&root),
            &hashes,
            snap,
            skill["remotePath"].as_str().unwrap_or(""),
            Some(&scope),
        )?;
        issues.extend(merged.issues);
        issues.sort();
        issues.dedup();
        inspect_snapshot(skill, Some(&root), &hashes, &issues, snap)
    } else {
        inspect_issues(skill, &root, &hashes, &issues)
    };
    if allow_environment {
        for item in &mut items {
            if optional_ids.contains(&item.id)
                && !matches!(item.role.as_str(), "required" | "optional")
            {
                *item = export_candidate(skill, item, snap, true)?;
            }
        }
    }
    for id in optional_ids {
        if !items.iter().any(|item| {
            &item.id == id
                && (matches!(item.role.as_str(), "required" | "optional")
                    || allow_environment
                        && item.status == "available"
                        && item.package_path.is_some())
        }) {
            return Err("所选外部文件已变化，请重新核对并勾选".into());
        }
    }
    let mut data = BTreeMap::new();
    let mut replacements: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut receipts = Vec::new();
    let mut total = 0usize;
    for item in &items {
        if !item.required && !optional_ids.contains(&item.id) {
            continue;
        }
        if item.status != "available" {
            return Err(format!(
                "必要或选定的外部文件不可用于导出：{}；{}",
                item.target, item.reason
            ));
        }
        let package = item
            .package_path
            .as_deref()
            .ok_or("请先将外部依赖绑定到具体的普通文件")?;
        let bytes = if item.effective_source.as_deref() == Some("github") {
            let snap = snap.ok_or("请先核对关联 GitHub 的固定提交")?;
            let path = item.remote["repositoryPath"]
                .as_str()
                .ok_or("GitHub 文件路径未核验")?;
            let bytes = snap.read(path)?;
            if Some(digest(&bytes)) != item.hash {
                return Err("固定提交的外部文件摘要不一致".into());
            }
            bytes
        } else {
            read_bound_resource(skill, &root, item, resource_bytes)?
        };
        if let Some(prior) = data.get(package) {
            if prior != &bytes {
                return Err("外部文件的包内路径发生冲突".into());
            }
        } else {
            total = total
                .checked_add(bytes.len())
                .filter(|value| *value <= MAX_RESOURCES)
                .ok_or("外部资源总大小超过 32 MiB")?;
            data.insert(package.to_owned(), bytes);
        }
        let mut rewritten = false;
        if matches!(
            Path::new(&item.source)
                .extension()
                .and_then(|ext| ext.to_str()),
            Some("md" | "mdx" | "txt" | "rst")
        ) && runtime.contains(&item.source)
        {
            let original = if let Some(bytes) = replacements.get(&item.source) {
                bytes.clone()
            } else {
                if hashes.contains_key(&item.source) {
                    fs::read(local::safe_join(&root, &item.source)?)
                        .map_err(|error| error.to_string())?
                } else {
                    let snap = snap.ok_or("引用文档不在本地，请重新读取 GitHub 文件")?;
                    let path =
                        repo_reference(skill["remotePath"].as_str().unwrap_or(""), &item.source)
                            .ok_or("引用文档路径无效")?;
                    snap.read(&path)?
                }
            };
            if let Ok(text) = String::from_utf8(original) {
                let (text, changed) = rewrite_prose(
                    &text,
                    &item.target,
                    &relative_reference(&item.source, package),
                );
                if changed {
                    replacements.insert(item.source.clone(), text.into_bytes());
                    rewritten = true;
                }
            }
        }
        receipts.push(json!({"id":item.id,"source":item.source,"packagePath":package,"hash":item.hash,"hashAlgorithm":"sha256","role":item.role,
            "contentSource":item.effective_source,"repo":if item.effective_source.as_deref()==Some("github"){skill["repo"].clone()}else{Value::Null},
            "repositoryPath":item.remote.get("repositoryPath").filter(|_|item.effective_source.as_deref()==Some("github")),
            "commit":item.remote.get("commit").filter(|_|item.effective_source.as_deref()==Some("github")),
            "referenceRewritten":rewritten,"portability":if rewritten&&item.portable{"relative_reference"}else{"requires_environment"}}));
    }
    Ok(ExportResources {
        data,
        replacements,
        provenance: json!({"formatVersion":1,"files":receipts,"environment":items.iter().filter(|item|item.role=="environment").map(|item|json!({"source":item.source,"kind":item.kind,"status":item.status,"requirement":"original_environment_required"})).collect::<Vec<_>>(),"invocationStatus":"not_verified"}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf, Value) {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("work/external-tests");
        fs::create_dir_all(&parent).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("synthetic-")
            .tempdir_in(parent)
            .unwrap();
        let root = temp.path().join("skill");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: synthetic\ndescription: Test\n---\n",
        )
        .unwrap();
        let root = local::canonical(&root).unwrap();
        let skill = json!({"id":"synthetic","localPath":display(&root),"name":"synthetic"});
        (temp, root, skill)
    }
    fn external_file(temp: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
        let path = temp.path().join(name);
        fs::write(&path, content).unwrap();
        local::canonical(&path).unwrap()
    }
    fn issue(source: &str, target: &str, category: &str) -> local::RuntimeIssue {
        local::RuntimeIssue {
            source: source.into(),
            target: target.into(),
            category: category.into(),
            severity: "warning".into(),
            reason: "test".into(),
        }
    }

    #[test]
    fn v021_missing_absolute_identity_rejects_traversal_and_prefix_siblings() {
        let (_temp, root, skill) = fixture();
        let missing = root.join("assets/deep/config.json");
        assert_eq!(
            absolute_package_reference(&skill, &display(&missing)).as_deref(),
            Some("assets/deep/config.json")
        );
        assert!(!missing.exists());
        assert!(
            absolute_package_reference(&skill, &format!("{}/../outside.json", display(&root)))
                .is_none()
        );
        assert!(absolute_package_reference(
            &skill,
            &format!("{}-sibling/file.json", display(&root))
        )
        .is_none());
        assert_eq!(
            repo_reference("skills/current/references", r"..\assets\config.json").as_deref(),
            Some("skills/current/assets/config.json")
        );
    }

    #[cfg(windows)]
    #[test]
    fn v021_missing_absolute_identity_rejects_interior_links_and_changed_alias_roots() {
        let (temp, root, mut skill) = fixture();
        let elsewhere = temp.path().join("outside");
        fs::create_dir(&elsewhere).unwrap();
        let interior = root.join("assets");
        junction::create(&elsewhere, &interior).unwrap();
        assert!(
            absolute_package_reference(&skill, &display(&interior.join("missing.json"))).is_none()
        );
        let alias = temp.path().join("known-alias");
        junction::create(&root, &alias).unwrap();
        skill["_externalAliasRoots"] = json!([display(&alias)]);
        assert_eq!(
            absolute_package_reference(&skill, &display(&alias.join("config.json"))).as_deref(),
            Some("config.json")
        );
        junction::delete(&alias).unwrap();
        fs::remove_dir(&alias).unwrap();
        junction::create(&elsewhere, &alias).unwrap();
        assert!(absolute_package_reference(&skill, &display(&alias.join("config.json"))).is_none());
        junction::delete(&alias).unwrap();
        fs::remove_dir(&alias).unwrap();
        junction::delete(&interior).unwrap();
        fs::remove_dir(&interior).unwrap();
    }
    fn tree_names(root: &Path) -> BTreeSet<PathBuf> {
        fn visit(root: &Path, current: &Path, all: &mut BTreeSet<PathBuf>) {
            for entry in fs::read_dir(current).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                all.insert(path.strip_prefix(root).unwrap().to_owned());
                if entry.file_type().unwrap().is_dir() {
                    visit(root, &path, all);
                }
            }
        }
        let mut all = BTreeSet::new();
        visit(root, root, &mut all);
        all
    }
    #[test]
    fn required_resource_is_in_zip_copy_only_and_static_reference_is_relative() {
        let (temp, root, skill) = fixture();
        let resource = external_file(&temp, "template.json", "{\"template\":true}");
        let source = format!(
            "必须读取 [模板]({})。\n",
            display(&resource).replace('\\', "/")
        );
        fs::write(root.join("SKILL.md"), &source).unwrap();
        let before = local::files(&root).unwrap();
        let items = inspect(&skill).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].required && items[0].default_included && items[0].readable);
        let exported = export_resources(&skill, &["SKILL.md".into()], &[]).unwrap();
        assert_eq!(exported.data.len(), 1);
        assert_eq!(
            exported.data.values().next().unwrap(),
            b"{\"template\":true}"
        );
        let rewritten = std::str::from_utf8(&exported.replacements["SKILL.md"]).unwrap();
        assert!(rewritten.contains("](external/"));
        assert!(!rewritten.contains(&display(&resource).replace('\\', "/")));
        assert_eq!(fs::read_to_string(root.join("SKILL.md")).unwrap(), source);
        assert_eq!(local::files(&root).unwrap(), before);
        assert_eq!(
            fs::read_to_string(&resource).unwrap(),
            "{\"template\":true}"
        );
        assert!(!exported
            .provenance
            .to_string()
            .contains(&display(&resource)));
        assert_eq!(exported.provenance["invocationStatus"], "not_verified");
    }
    #[test]
    fn optional_resource_stays_unselected_until_requested() {
        let (temp, root, skill) = fixture();
        let resource = external_file(&temp, "extra.md", "Optional guide");
        fs::write(
            root.join("SKILL.md"),
            format!("按需读取 `{}`。", display(&resource).replace('\\', "/")),
        )
        .unwrap();
        let items = inspect(&skill).unwrap();
        assert_eq!(items[0].role, "optional");
        assert!(export_resources(&skill, &["SKILL.md".into()], &[])
            .unwrap()
            .data
            .is_empty());
        assert_eq!(
            export_resources(&skill, &["SKILL.md".into()], &[items[0].id.clone()])
                .unwrap()
                .data
                .len(),
            1
        );
        assert_eq!(local::files(&root).unwrap().len(), 1);
    }
    #[test]
    fn required_missing_resource_blocks_export_but_historical_absence_does_not() {
        let (temp, root, skill) = fixture();
        let missing = display(&temp.path().join("missing.json")).replace('\\', "/");
        fs::write(
            root.join("SKILL.md"),
            format!("必须读取 [{missing}]({missing})。\n"),
        )
        .unwrap();
        let items = inspect(&skill).unwrap();
        assert!(items[0].required);
        assert_eq!(items[0].status, "missing");
        assert!(export_resources(&skill, &["SKILL.md".into()], &[])
            .unwrap_err()
            .contains("不可用于导出"));
        fs::write(
            root.join("SKILL.md"),
            format!("# 历史来源\n[历史备份]({missing})。\n"),
        )
        .unwrap();
        let items = inspect(&skill).unwrap();
        assert_eq!(items[0].status, "reference");
        assert!(export_resources(&skill, &["SKILL.md".into()], &[])
            .unwrap()
            .data
            .is_empty());
    }
    #[test]
    fn shared_rules_environment_root_and_records_have_terminal_roles_without_recursion() {
        let (temp, root, skill) = fixture();
        let agents = external_file(&temp, "AGENTS.md", "Shared policy");
        let records = temp.path().join("_records").join("skill");
        fs::create_dir_all(&records).unwrap();
        let env = display(temp.path()).replace('\\', "/");
        fs::write(
            root.join("SKILL.md"),
            format!(
                "本机可访问时读取 [全局规则]({})。\nCODEX_HOME 是 `{env}`。\n维护索引：{}。\n",
                display(&agents).replace('\\', "/"),
                display(&records).replace('\\', "/")
            ),
        )
        .unwrap();
        let hashes = local::files(&root).unwrap();
        let items = inspect_issues(
            &skill,
            &root,
            &hashes,
            &[
                issue("SKILL.md", &display(&agents), "external"),
                issue("SKILL.md", &env, "external"),
                issue("SKILL.md", &display(&records), "external"),
            ],
        );
        assert_eq!(items[0].role, "environment");
        assert_eq!(items[0].status, "available");
        assert!(items[0].hash.is_none());
        assert_eq!(items[1].role, "environment");
        assert_eq!(items[1].status, "available");
        assert_eq!(items[2].role, "history");
        assert_eq!(items[2].status, "reference");
        assert!(items.iter().all(|item| !item.default_included));
    }
    #[test]
    fn secret_file_is_not_read_packaged_or_bound_as_required() {
        let (temp, root, skill) = fixture();
        let secret = external_file(&temp, "auth.json", "Not a real credential");
        fs::write(
            root.join("SKILL.md"),
            format!("必须读取 `{}`。", display(&secret).replace('\\', "/")),
        )
        .unwrap();
        let items = inspect(&skill).unwrap();
        assert_eq!(items[0].role, "environment");
        assert!(items[0].hash.is_none());
        assert!(export_resources(&skill, &["SKILL.md".into()], &[])
            .unwrap()
            .data
            .is_empty());
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.upsert_skill(skill);
        let error=handle(&mut manager,&json!({"action":"bind_external_dependency","skillId":"synthetic","dependencyId":items[0].id,"role":"required","path":display(&secret)})).unwrap_err();
        assert!(error.contains("私有"));
        assert!(manager.skill("synthetic").unwrap()["externalBindings"].is_null());
    }
    #[test]
    fn binding_survives_unrelated_edits_but_reference_meaning_changes_require_review() {
        let (temp, root, skill) = fixture();
        let resource = external_file(&temp, "rules.md", "Selected resource");
        fs::write(
            root.join("SKILL.md"),
            format!("当前配置位置 `{}`。", display(&resource).replace('\\', "/")),
        )
        .unwrap();
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.upsert_skill(skill);
        let id = inspect(&manager.skill("synthetic").unwrap()).unwrap()[0]
            .id
            .clone();
        let response=handle(&mut manager,&json!({"action":"bind_external_dependency","skillId":"synthetic","dependencyId":id,"role":"required","path":display(&resource)})).unwrap().unwrap();
        assert_eq!(response["items"][0]["required"], true);
        assert!(resource.exists());
        assert_eq!(local::files(&root).unwrap().len(), 1);
        let original = fs::read_to_string(root.join("SKILL.md")).unwrap();
        fs::write(
            root.join("SKILL.md"),
            format!("{original}\nNew source revision."),
        )
        .unwrap();
        let items = inspect(&manager.skill("synthetic").unwrap()).unwrap();
        assert!(!items[0].binding_stale);
        assert!(items[0].required);
        assert_ne!(
            items[0].source_hash,
            response["items"][0]["sourceHash"].as_str().unwrap()
        );
        fs::write(&resource, "Updated resource").unwrap();
        let current = inspect(&manager.skill("synthetic").unwrap()).unwrap();
        assert_eq!(current[0].status, "available");
        assert!(!current[0].binding_stale);
        assert_ne!(current[0].hash, items[0].hash);
        assert_eq!(
            export_resources(
                &manager.skill("synthetic").unwrap(),
                &["SKILL.md".into()],
                &[]
            )
            .unwrap()
            .data
            .values()
            .next()
            .unwrap(),
            b"Updated resource"
        );
        fs::write(root.join("SKILL.md"), format!("# 历史来源\n{original}\n")).unwrap();
        let items = inspect(&manager.skill("synthetic").unwrap()).unwrap();
        assert!(items[0].binding_stale);
        assert!(items[0].required);
        assert_eq!(items[0].status, "needs_review");
        assert!(export_resources(
            &manager.skill("synthetic").unwrap(),
            &["SKILL.md".into()],
            &[]
        )
        .is_err());
        handle(&mut manager,&json!({"action":"bind_external_dependency","skillId":"synthetic","dependencyId":id,"role":"required"})).unwrap();
        assert!(!inspect(&manager.skill("synthetic").unwrap()).unwrap()[0].binding_stale);
        fs::write(
            root.join("SKILL.md"),
            format!("按需读取 `{}`。", display(&resource).replace('\\', "/")),
        )
        .unwrap();
        assert!(inspect(&manager.skill("synthetic").unwrap()).unwrap()[0].binding_stale);
    }
    #[cfg(windows)]
    #[test]
    fn bound_external_junction_target_change_requires_review() {
        let (temp, root, skill) = fixture();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(first.join("template.json"), "first").unwrap();
        fs::write(second.join("template.json"), "second").unwrap();
        let link = temp.path().join("resources");
        junction::create(&first, &link).unwrap();
        let target = display(&link.join("template.json")).replace('\\', "/");
        fs::write(
            root.join("SKILL.md"),
            format!("必须读取 [模板]({target})。\n"),
        )
        .unwrap();
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.upsert_skill(skill);
        let id = inspect(&manager.skill("synthetic").unwrap()).unwrap()[0]
            .id
            .clone();
        handle(&mut manager,&json!({"action":"bind_external_dependency","skillId":"synthetic","dependencyId":id,"role":"required","path":target})).unwrap();
        assert!(!inspect(&manager.skill("synthetic").unwrap()).unwrap()[0].binding_stale);
        junction::delete(&link).unwrap();
        fs::remove_dir(&link).unwrap();
        junction::create(&second, &link).unwrap();
        let item = inspect(&manager.skill("synthetic").unwrap())
            .unwrap()
            .remove(0);
        assert!(item.binding_stale);
        assert_eq!(item.status, "needs_review");
        assert!(item.required);
        assert!(export_resources(
            &manager.skill("synthetic").unwrap(),
            &["SKILL.md".into()],
            &[]
        )
        .is_err());
        junction::delete(&link).unwrap();
        fs::remove_dir(&link).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn original_external_alias_swap_during_read_rejects_the_export_snapshot() {
        let (temp, root, skill) = fixture();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(first.join("template.json"), "original").unwrap();
        fs::write(second.join("template.json"), "replacement").unwrap();
        let link = temp.path().join("resources");
        junction::create(&first, &link).unwrap();
        let target = display(&link.join("template.json")).replace('\\', "/");
        fs::write(
            root.join("SKILL.md"),
            format!("必须读取 [模板]({target})。\n"),
        )
        .unwrap();
        let item = inspect(&skill).unwrap().remove(0);
        assert!(item.required);
        let error = read_bound_resource(&skill, &root, &item, |path| {
            let bytes = resource_bytes(path)?;
            junction::delete(&link).map_err(|error| error.to_string())?;
            fs::remove_dir(&link).map_err(|error| error.to_string())?;
            junction::create(&second, &link).map_err(|error| error.to_string())?;
            Ok(bytes)
        })
        .unwrap_err();
        assert!(error.contains("链接目标发生变化"));
        assert_eq!(
            fs::read_to_string(first.join("template.json")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(second.join("template.json")).unwrap(),
            "replacement"
        );
        assert!(!temp.path().join("synthetic.zip").exists());
        junction::delete(&link).unwrap();
        fs::remove_dir(&link).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn client_junction_to_current_skill_is_an_alias_not_a_second_resource() {
        let (temp, root, skill) = fixture();
        let link = temp.path().join("client-entry");
        junction::create(&root, &link).unwrap();
        let item = inspect_issues(
            &skill,
            &root,
            &local::files(&root).unwrap(),
            &[issue("SKILL.md", &display(&link), "external")],
        )
        .remove(0);
        assert_eq!(item.role, "environment");
        assert_eq!(item.reference_kind, "skill_alias");
        assert_eq!(item.status, "available");
        assert_eq!(item.canonical_path, Some(display(&root)));
        assert!(!item.default_included);
        junction::delete(&link).unwrap();
        fs::remove_dir(&link).unwrap();
    }
    #[test]
    fn static_script_resource_is_included_but_not_claimed_portable() {
        let (temp, root, skill) = fixture();
        let resource = external_file(&temp, "template.json", "Script input");
        fs::create_dir(root.join("scripts")).unwrap();
        fs::write(root.join("SKILL.md"), "Run `scripts/main.py`.").unwrap();
        fs::write(
            root.join("scripts/main.py"),
            format!(
                "import json\nfrom pathlib import Path\nvalue = Path('{}').read_text()\n",
                display(&resource).replace('\\', "/")
            ),
        )
        .unwrap();
        let items = inspect(&skill).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].required);
        assert!(!items[0].portable);
        let exported =
            export_resources(&skill, &["SKILL.md".into(), "scripts/main.py".into()], &[]).unwrap();
        assert_eq!(exported.data.len(), 1);
        assert!(exported.replacements.is_empty());
        assert_eq!(
            exported.provenance["files"][0]["portability"],
            "requires_environment"
        );
    }
    #[test]
    fn cli_and_workspace_inputs_are_environment_requirements_and_do_not_block_real_zip() {
        let (temp, root, mut skill) = fixture();
        fs::create_dir(root.join("scripts")).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "Run `scripts/main.py` with the user-selected input.",
        )
        .unwrap();
        fs::write(root.join("scripts/main.py"),"import argparse\nfrom pathlib import Path\nparser = argparse.ArgumentParser()\nparser.add_argument('input')\nargs = parser.parse_args()\ncontent = Path(args.input).read_text()\ndefault = Path('workspace/input.json').read_text()\n").unwrap();
        let items = inspect(&skill).unwrap();
        assert!(items.iter().any(|item| item.target == "运行时输入"));
        assert!(items
            .iter()
            .any(|item| item.target == "workspace/input.json"));
        assert!(items
            .iter()
            .all(|item| !item.required && item.role == "environment"));
        let runtime = local::minimal_runtime_files(&root).unwrap();
        assert_eq!(runtime, vec!["SKILL.md", "scripts/main.py"]);
        assert!(export_resources(&skill, &runtime, &[])
            .unwrap()
            .data
            .is_empty());
        skill["runtimeFiles"] = json!(runtime);
        skill["runtimeConfirmed"] = json!(true);
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.upsert_skill(skill);
        let output = temp.path().join("cli.zip");
        crate::remote::file_request(
            &mut manager,
            &json!({"action":"export","skillId":"synthetic","destination":display(&output)}),
        )
        .unwrap()
        .unwrap();
        let archive = zip::ZipArchive::new(fs::File::open(output).unwrap()).unwrap();
        assert_eq!(archive.len(), 3);
        assert!(!archive
            .file_names()
            .any(|name| name == "workspace/input.json"));
    }
    #[test]
    fn concrete_absolute_script_dependency_missing_still_blocks_export() {
        let (temp, root, skill) = fixture();
        fs::create_dir(root.join("scripts")).unwrap();
        fs::write(root.join("SKILL.md"), "Run `scripts/main.py`.").unwrap();
        let missing = display(&temp.path().join("required-template.json")).replace('\\', "/");
        fs::write(
            root.join("scripts/main.py"),
            format!("from pathlib import Path\ncontent = Path('{missing}').read_text()\n"),
        )
        .unwrap();
        let items = inspect(&skill).unwrap();
        let required = items.iter().find(|item| item.target == missing).unwrap();
        assert!(required.required);
        assert_eq!(required.status, "missing");
        assert!(
            export_resources(&skill, &["SKILL.md".into(), "scripts/main.py".into()], &[])
                .unwrap_err()
                .contains("不可用于导出")
        );
        assert!(!temp.path().join("synthetic.zip").exists());
    }
    #[test]
    fn repeated_reference_reads_one_resource_and_rewrites_both_prose_files() {
        let (temp, root, skill) = fixture();
        let resource = external_file(&temp, "template.json", "one copy");
        let target = display(&resource).replace('\\', "/");
        fs::create_dir(root.join("references")).unwrap();
        fs::write(
            root.join("SKILL.md"),
            format!("Read `references/use.md`.\n必须读取 [模板]({target})。\n"),
        )
        .unwrap();
        fs::write(
            root.join("references/use.md"),
            format!("必须读取 [模板]({target})。\n"),
        )
        .unwrap();
        let exported = export_resources(
            &skill,
            &["SKILL.md".into(), "references/use.md".into()],
            &[],
        )
        .unwrap();
        assert_eq!(exported.data.len(), 1);
        assert_eq!(exported.replacements.len(), 2);
        assert!(
            std::str::from_utf8(&exported.replacements["references/use.md"])
                .unwrap()
                .contains("](../external/")
        );
    }
    #[test]
    fn code_fences_and_dynamic_expressions_are_not_rewritten_as_if_proven_portable() {
        let text = "```python\nread('/absolute/path')\n```\n[resource](/absolute/path)\n";
        let (rewritten, changed) = rewrite_prose(text, "/absolute/path", "external/item");
        assert!(changed);
        assert!(rewritten.contains("read('/absolute/path')"));
        assert!(rewritten.contains("[resource](external/item)"));
        assert_eq!(
            rewrite_prose(
                "[file](/absolute/path)",
                "/absolute/path",
                "external/a b(1).json"
            )
            .0,
            "[file](external/a%20b%281%29.json)"
        );
    }
    #[test]
    fn partial_reference_rewrite_does_not_claim_portability() {
        let (temp, root, skill) = fixture();
        let resource = external_file(&temp, "template.json", "Resource");
        let target = display(&resource).replace('\\', "/");
        fs::write(
            root.join("SKILL.md"),
            format!("必须读取 [模板]({target})。\nAdditional hardcoded reference: {target}\n"),
        )
        .unwrap();
        let item = inspect(&skill).unwrap().remove(0);
        assert!(item.required);
        assert!(!item.portable);
        let exported = export_resources(&skill, &["SKILL.md".into()], &[]).unwrap();
        assert!(!exported.replacements.is_empty());
        assert_eq!(
            exported.provenance["files"][0]["portability"],
            "requires_environment"
        );
    }
    #[test]
    fn export_api_writes_only_final_zip_and_keeps_optional_document_unselected() {
        let (temp, root, mut skill) = fixture();
        let required = external_file(&temp, "template.json", "Required bytes");
        let optional = external_file(&temp, "guide.md", "Optional bytes");
        let source = format!(
            "必须读取 [模板]({})。\n按需读取 `{}`。\n",
            display(&required).replace('\\', "/"),
            display(&optional).replace('\\', "/")
        );
        fs::write(root.join("SKILL.md"), &source).unwrap();
        skill["runtimeFiles"] = json!(["SKILL.md"]);
        skill["runtimeConfirmed"] = json!(true);
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.upsert_skill(skill);
        let before = tree_names(temp.path());
        let output = temp.path().join("synthetic.zip");
        crate::remote::file_request(
            &mut manager,
            &json!({"action":"export","skillId":"synthetic","destination":display(&output)}),
        )
        .unwrap()
        .unwrap();
        let mut archive = zip::ZipArchive::new(fs::File::open(&output).unwrap()).unwrap();
        assert_eq!(archive.len(), 3);
        let names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();
        assert!(names.iter().any(|name| name.ends_with("template.json")));
        assert!(!names.iter().any(|name| name.ends_with("guide.md")));
        let mut zipped = String::new();
        archive
            .by_name("SKILL.md")
            .unwrap()
            .read_to_string(&mut zipped)
            .unwrap();
        assert!(zipped.contains("](external/"));
        let mut provenance = String::new();
        archive
            .by_name("skill-manager-provenance.json")
            .unwrap()
            .read_to_string(&mut provenance)
            .unwrap();
        assert!(!provenance.contains(&display(&required).replace('\\', "/")));
        assert!(!provenance.contains(&display(&optional).replace('\\', "/")));
        assert_eq!(fs::read_to_string(root.join("SKILL.md")).unwrap(), source);
        assert_eq!(local::files(&root).unwrap().len(), 1);
        assert!(required.exists() && optional.exists());
        assert_eq!(
            tree_names(temp.path())
                .difference(&before)
                .cloned()
                .collect::<Vec<_>>(),
            vec![PathBuf::from("synthetic.zip")]
        );
    }

    #[test]
    fn v020_slash_command_is_not_missing_but_actual_absolute_file_stays_required() {
        let (_temp, root, skill) = fixture();
        fs::write(
            root.join("SKILL.md"),
            "客户端菜单提供 `/approve` 命令。\n必须读取 `/actual/file.json`。\n",
        )
        .unwrap();
        let items = inspect(&skill).unwrap();
        let command = items.iter().find(|item| item.target == "/approve").unwrap();
        assert_eq!(command.reference_kind, "command");
        assert_eq!(command.status, "environment");
        assert!(!command.required);
        let file = items
            .iter()
            .find(|item| item.target == "/actual/file.json")
            .unwrap();
        assert_eq!(file.reference_kind, "file");
        assert!(file.required);
        assert_eq!(file.status, "missing");
    }

    #[test]
    fn v020_binding_preserves_history_role_and_recheck_does_not_mutate_records() {
        let (temp, root, skill) = fixture();
        let resource = external_file(&temp, "guide.md", "Historical notes");
        fs::write(
            root.join("SKILL.md"),
            "# Sources\n历史来源 `Z:/synthetic-only/old.md`。\n",
        )
        .unwrap();
        let item = inspect(&skill).unwrap().remove(0);
        assert_eq!(item.role, "history");
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.upsert_skill(skill);
        manager.save().unwrap();
        let result=handle(&mut manager,&json!({"action":"bind_external_dependency","skillId":"synthetic","dependencyId":item.id,"path":display(&resource),"expectedFingerprint":item.reference_fingerprint,"includeRemote":false})).unwrap().unwrap();
        assert_eq!(result["items"][0]["role"], "history");
        let before = manager.state.skills.clone();
        let result=handle(&mut manager,&json!({"action":"batch_external_dependencies","operation":"recheck","skillId":"synthetic","includeRemote":false,"items":[{"id":item.id,"expectedFingerprint":item.reference_fingerprint}]})).unwrap().unwrap();
        assert_eq!(result["results"][0]["status"], "success");
        assert_eq!(manager.state.skills, before);
    }

    #[test]
    fn v020_package_file_reference_retains_purpose_and_zip_has_one_copy() {
        let (temp, root, skill) = fixture();
        fs::create_dir(root.join("assets")).unwrap();
        let resource = root.join("assets/template.json");
        fs::write(&resource, b"local template").unwrap();
        fs::write(
            root.join("SKILL.md"),
            format!(
                "必须读取 [模板]({})。\n",
                display(&resource).replace('\\', "/")
            ),
        )
        .unwrap();
        let item = inspect(&skill).unwrap().remove(0);
        assert_eq!(item.reference_kind, "package_file");
        assert_eq!(item.role, "required");
        assert_eq!(item.package_path.as_deref(), Some("assets/template.json"));
        let before = local::files(&root).unwrap();
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.upsert_skill(skill);
        let destination = temp.path().join("package.zip");
        crate::remote::file_request(
            &mut manager,
            &json!({"action":"export","skillId":"synthetic","destination":display(&destination)}),
        )
        .unwrap()
        .unwrap();
        let archive = zip::ZipArchive::new(fs::File::open(destination).unwrap()).unwrap();
        assert_eq!(
            archive
                .file_names()
                .filter(|name| *name == "assets/template.json")
                .count(),
            1
        );
        assert!(!archive
            .file_names()
            .any(|name| name.starts_with("external/")));
        assert_eq!(local::files(&root).unwrap(), before);
    }

    #[test]
    fn v020_batch_valid_rows_save_together_and_stale_fingerprints_remain_untouched() {
        let (temp, root, skill) = fixture();
        let a = external_file(&temp, "a.txt", "A");
        let b = external_file(&temp, "b.txt", "B");
        fs::write(
            root.join("SKILL.md"),
            format!("按需读取 `{}`。\n按需读取 `{}`。", display(&a), display(&b)),
        )
        .unwrap();
        let items = inspect(&skill).unwrap();
        assert_eq!(items.len(), 2);
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.upsert_skill(skill);
        manager.save().unwrap();
        let response=handle(&mut manager,&json!({"action":"batch_external_dependencies","operation":"role","skillId":"synthetic","includeRemote":false,"items":[
            {"id":items[0].id,"expectedFingerprint":items[0].reference_fingerprint,"role":"required"},
            {"id":items[1].id,"expectedFingerprint":"stale","role":"required"}
        ]})).unwrap().unwrap();
        assert_eq!(response["results"][0]["status"], "success");
        assert_eq!(response["results"][1]["status"], "error");
        assert_eq!(
            manager.skill("synthetic").unwrap()["externalBindings"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let reopened = Manager::new(temp.path().join("data")).unwrap();
        assert_eq!(
            reopened.skill("synthetic").unwrap()["externalBindings"],
            manager.skill("synthetic").unwrap()["externalBindings"]
        );
        let remapped=handle(&mut manager,&json!({"action":"batch_external_dependencies","operation":"remap","skillId":"synthetic","includeRemote":false,"items":[{"id":items[0].id,"expectedFingerprint":items[0].reference_fingerprint}]})).unwrap().unwrap();
        let row = remapped["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == items[0].id)
            .unwrap();
        assert_eq!(row["role"], "required");
        assert_eq!(remapped["results"][0]["status"], "success");
    }

    #[test]
    fn v020_zip_rejects_credential_paths_from_package_required_closure() {
        for name in ["auth.json", ".env", ".env.example"] {
            let (temp, root, skill) = fixture();
            fs::write(root.join(name), b"synthetic credential placeholder").unwrap();
            fs::write(
                root.join("SKILL.md"),
                format!("必须读取 [运行配置]({name})。\n"),
            )
            .unwrap();
            let before = local::files(&root).unwrap();
            let mut manager = Manager::new(temp.path().join("data")).unwrap();
            manager.upsert_skill(skill);
            let destination = temp.path().join("refused.zip");
            let error=crate::remote::file_request(&mut manager,&json!({"action":"export","skillId":"synthetic","destination":display(&destination)})).unwrap_err();
            assert!(error.contains("凭据"), "{name}: {error}");
            assert!(!error.contains("synthetic credential placeholder"));
            assert!(!destination.exists());
            assert_eq!(local::files(&root).unwrap(), before);
        }
    }

    #[test]
    fn v020_zip_keeps_ordinary_required_configuration_and_final_writer_has_same_guard() {
        let (temp, root, skill) = fixture();
        fs::write(root.join("config.toml"), b"format = 1\n").unwrap();
        fs::write(root.join("template.json"), b"{\"format\":1}").unwrap();
        fs::write(
            root.join("SKILL.md"),
            "必须读取 [配置](config.toml) 和 [模板](template.json)。\n",
        )
        .unwrap();
        let before = local::files(&root).unwrap();
        let mut manager = Manager::new(temp.path().join("data")).unwrap();
        manager.upsert_skill(skill);
        let destination = temp.path().join("ordinary.zip");
        crate::remote::file_request(
            &mut manager,
            &json!({"action":"export","skillId":"synthetic","destination":display(&destination)}),
        )
        .unwrap()
        .unwrap();
        let archive = zip::ZipArchive::new(fs::File::open(destination).unwrap()).unwrap();
        let names = archive.file_names().collect::<Vec<_>>();
        assert!(names.contains(&"config.toml"));
        assert!(names.contains(&"template.json"));
        assert_eq!(local::files(&root).unwrap(), before);
        let refused = temp.path().join("direct-refused.zip");
        let data = BTreeMap::from([(
            "external/synthetic/credentials.json".into(),
            b"synthetic private".to_vec(),
        )]);
        let error =
            local::export_package(&root, &display(&refused), &before, &[], &data, &json!({}))
                .unwrap_err();
        assert!(error.contains("凭据"));
        assert!(!refused.exists());
    }
}
