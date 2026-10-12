//! Native runtime directory selection.
//!
//! Rust port of `mesh/scripts/select-native-runtime.py`. Chooses the single
//! `*/manifest.json` entry under a root directory that matches the requested
//! operating system, architecture, backend, and CUDA toolkit major version.

use crate::command::DynResult;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn select_native_runtime_command(args: &[String]) -> DynResult<()> {
    let parsed = parse_select_args(args)?;
    let selected = select_runtime(
        &parsed.root,
        &parsed.os_name,
        &parsed.arch,
        &parsed.backend,
        &parsed.cuda_major,
    )
    .map_err(|message| format!("native-runtime select: {message}"))?;
    if parsed.json {
        println!("{}", serde_json::json!({ "path": selected }));
    } else {
        println!("{}", selected.display());
    }
    Ok(())
}

struct SelectArgs {
    root: PathBuf,
    os_name: String,
    arch: String,
    backend: String,
    cuda_major: String,
    json: bool,
}

fn parse_select_args(args: &[String]) -> DynResult<SelectArgs> {
    let mut root = None;
    let mut os_name = None;
    let mut arch = None;
    let mut backend = None;
    let mut cuda_major = String::new();
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (arg.as_str(), None),
        };
        match flag {
            "--root" => root = Some(PathBuf::from(take_value(args, &mut index, flag, inline)?)),
            "--os" => os_name = Some(take_value(args, &mut index, flag, inline)?),
            "--arch" => arch = Some(take_value(args, &mut index, flag, inline)?),
            "--backend" => backend = Some(take_value(args, &mut index, flag, inline)?),
            "--cuda-major" => cuda_major = take_value(args, &mut index, flag, inline)?,
            "--json" => json = true,
            other => {
                return Err(format!("native-runtime select: unknown flag {other}").into());
            }
        }
    }
    Ok(SelectArgs {
        root: require("--root", root)?,
        os_name: require("--os", os_name)?,
        arch: require("--arch", arch)?,
        backend: require("--backend", backend)?,
        cuda_major,
        json,
    })
}

fn take_value(
    args: &[String],
    index: &mut usize,
    flag: &str,
    inline: Option<&str>,
) -> DynResult<String> {
    if let Some(value) = inline {
        return Ok(value.to_string());
    }
    if *index < args.len() {
        let value = args[*index].clone();
        *index += 1;
        return Ok(value);
    }
    Err(format!("native-runtime select: {flag} needs a value").into())
}

fn require<T>(flag: &str, value: Option<T>) -> DynResult<T> {
    value.ok_or_else(|| format!("native-runtime select: missing required {flag}").into())
}

fn backend_kind(backend: &str) -> &str {
    match backend {
        "cuda-blackwell" => "cuda",
        "hip" => "rocm",
        other => other,
    }
}

fn select_runtime(
    root: &Path,
    os_name: &str,
    arch: &str,
    backend: &str,
    cuda_major: &str,
) -> Result<PathBuf, String> {
    let expected_kind = backend_kind(backend);
    let mut matches = Vec::new();
    if root.is_dir() {
        for manifest in sorted_manifests(root)? {
            if manifest_matches(&manifest, os_name, arch, expected_kind, cuda_major)?
                && let Some(parent) = manifest.parent()
            {
                matches.push(parent.to_path_buf());
            }
        }
    }
    if matches.len() != 1 {
        let rendered = matches
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let rendered = if rendered.is_empty() {
            "none".to_string()
        } else {
            rendered
        };
        return Err(format!(
            "expected exactly one native runtime for {os_name}/{arch}/{backend}; \
             found {rendered} under {}",
            root.display()
        ));
    }
    Ok(matches.swap_remove(0))
}

fn sorted_manifests(root: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        fs::read_dir(root).map_err(|error| format!("cannot list {}: {error}", root.display()))?;
    let mut manifests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot list {}: {error}", root.display()))?;
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            let manifest = entry.path().join("manifest.json");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }
    manifests.sort();
    Ok(manifests)
}

fn manifest_matches(
    path: &Path,
    os_name: &str,
    arch: &str,
    expected_kind: &str,
    cuda_major: &str,
) -> Result<bool, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let manifest: Value = serde_json::from_str(&text)
        .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))?;
    let runtime = &manifest["runtime"];
    let platform = &runtime["platform"];
    if json_text(&platform["os"]).as_deref() != Some(os_name) {
        return Ok(false);
    }
    if json_text(&platform["arch"]).as_deref() != Some(arch) {
        return Ok(false);
    }
    let runtime_backend = &runtime["backend"];
    if json_text(&runtime_backend["kind"]).as_deref() != Some(expected_kind) {
        return Ok(false);
    }
    if expected_kind == "cuda" && !cuda_major.is_empty() {
        let toolkit = runtime_backend
            .get("cuda")
            .and_then(|cuda| cuda.get("toolkit_major"))
            .and_then(json_text)
            .unwrap_or_default();
        if toolkit != cuda_major {
            return Ok(false);
        }
    }
    Ok(true)
}

fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::unique_temp_dir;

    fn fixture_root() -> PathBuf {
        let root = unique_temp_dir("native-runtime-select");
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    fn write_manifest(
        root: &Path,
        name: &str,
        os: &str,
        arch: &str,
        kind: &str,
        toolkit: Option<&str>,
    ) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).expect("fixture dir");
        let cuda = toolkit
            .map(|major| format!(r#","cuda":{{"toolkit_major":"{major}"}}"#))
            .unwrap_or_default();
        let manifest = format!(
            r#"{{"runtime":{{"platform":{{"os":"{os}","arch":"{arch}"}},"backend":{{"kind":"{kind}"{cuda}}}}}}}"#
        );
        fs::write(dir.join("manifest.json"), manifest).expect("fixture manifest");
    }

    #[test]
    fn selects_single_matching_runtime() {
        let root = fixture_root();
        write_manifest(&root, "a-cuda", "linux", "x86_64", "cuda", Some("12"));
        write_manifest(&root, "b-cpu", "linux", "x86_64", "cpu", None);
        let selected = select_runtime(&root, "linux", "x86_64", "cuda", "").expect("select");
        assert_eq!(selected, root.join("a-cuda"));
    }

    #[test]
    fn maps_backend_aliases() {
        let root = fixture_root();
        write_manifest(&root, "a", "linux", "x86_64", "cuda", None);
        write_manifest(&root, "b", "linux", "x86_64", "rocm", None);
        let cuda = select_runtime(&root, "linux", "x86_64", "cuda-blackwell", "").expect("cuda");
        assert_eq!(cuda, root.join("a"));
        let rocm = select_runtime(&root, "linux", "x86_64", "hip", "").expect("rocm");
        assert_eq!(rocm, root.join("b"));
    }

    #[test]
    fn filters_cuda_toolkit_major() {
        let root = fixture_root();
        write_manifest(&root, "a", "linux", "x86_64", "cuda", Some("11"));
        write_manifest(&root, "b", "linux", "x86_64", "cuda", Some("12"));
        let selected = select_runtime(&root, "linux", "x86_64", "cuda", "12").expect("select");
        assert_eq!(selected, root.join("b"));
    }

    #[test]
    fn errors_when_nothing_matches() {
        let root = fixture_root();
        write_manifest(&root, "a", "linux", "x86_64", "cpu", None);
        let error = select_runtime(&root, "linux", "x86_64", "cuda", "").expect_err("must fail");
        assert!(error.contains("found none under"), "unexpected: {error}");
    }

    #[test]
    fn errors_when_multiple_match() {
        let root = fixture_root();
        write_manifest(&root, "a", "linux", "x86_64", "cpu", None);
        write_manifest(&root, "b", "linux", "x86_64", "cpu", None);
        let error = select_runtime(&root, "linux", "x86_64", "cpu", "").expect_err("must fail");
        assert!(
            error.contains("expected exactly one native runtime"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn parses_json_flag() {
        assert!(parse_select_args(&["--json".to_string()]).is_err());
        let full = vec![
            "--root".to_string(),
            "/tmp/r".to_string(),
            "--os".to_string(),
            "linux".to_string(),
            "--arch".to_string(),
            "x86_64".to_string(),
            "--backend".to_string(),
            "cpu".to_string(),
            "--json".to_string(),
        ];
        let parsed = parse_select_args(&full).expect("parse");
        assert!(parsed.json);
    }

    #[test]
    fn parses_space_and_equals_flags() {
        let args = vec![
            "--root".to_string(),
            "/tmp/r".to_string(),
            "--os=linux".to_string(),
            "--arch".to_string(),
            "x86_64".to_string(),
            "--backend".to_string(),
            "cpu".to_string(),
        ];
        let parsed = parse_select_args(&args).expect("parse");
        assert_eq!(parsed.root, PathBuf::from("/tmp/r"));
        assert_eq!(parsed.os_name, "linux");
        assert_eq!(parsed.cuda_major, "");
        assert!(!parsed.json);
        assert!(parse_select_args(&["--root".to_string()]).is_err());
        assert!(parse_select_args(&["--bogus".to_string()]).is_err());
    }
}
