//! Release UI identity binding and verification.
//!
//! Rust port of `mesh/scripts/ui-distribution.py`. Binds a prepared release
//! UI directory to its source revision and tag by hashing every file, then
//! stamps or verifies the `.mesh-llm-ui-release.json` manifest.

use crate::command::DynResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST: &str = ".mesh-llm-ui-release.json";

pub(crate) fn ui_distribution_stamp_command(args: &[String]) -> DynResult<()> {
    let parsed = parse_args(args)?;
    let manifest = describe(&parsed.dist, &parsed.source_sha, &parsed.release_tag)
        .map_err(|message| format!("ui-distribution stamp: {message}"))?;
    let text = serde_json::to_string_pretty(&manifest)? + "\n";
    fs::write(parsed.dist.join(MANIFEST), text)?;
    Ok(())
}

pub(crate) fn ui_distribution_verify_command(args: &[String]) -> DynResult<()> {
    let parsed = parse_args(args)?;
    let expected = describe(&parsed.dist, &parsed.source_sha, &parsed.release_tag)
        .map_err(|message| format!("ui-distribution verify: {message}"))?;
    let manifest_path = parsed.dist.join(MANIFEST);
    if !manifest_path.is_file() || manifest_path.is_symlink() {
        return Err(
            "ui-distribution verify: UI distribution is missing its release manifest".into(),
        );
    }
    let text = fs::read_to_string(&manifest_path)?;
    let actual: Manifest = serde_json::from_str(&text)?;
    if actual != expected {
        return Err(
            "ui-distribution verify: UI release identity or file checksums do not match".into(),
        );
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    source_sha: String,
    release_tag: String,
    files: BTreeMap<String, String>,
}

struct Args {
    dist: PathBuf,
    source_sha: String,
    release_tag: String,
}

fn parse_args(args: &[String]) -> DynResult<Args> {
    let mut dist = None;
    let mut source_sha = None;
    let mut release_tag = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (arg.as_str(), None),
        };
        match flag {
            "--dist" => dist = Some(PathBuf::from(take_value(args, &mut index, flag, inline)?)),
            "--source-sha" => source_sha = Some(take_value(args, &mut index, flag, inline)?),
            "--release-tag" => release_tag = Some(take_value(args, &mut index, flag, inline)?),
            other => {
                return Err(format!("ui-distribution: unknown flag {other}").into());
            }
        }
    }
    Ok(Args {
        dist: require("--dist", dist)?,
        source_sha: require("--source-sha", source_sha)?,
        release_tag: require("--release-tag", release_tag)?,
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
    Err(format!("ui-distribution: {flag} needs a value").into())
}

fn require<T>(flag: &str, value: Option<T>) -> DynResult<T> {
    value.ok_or_else(|| format!("ui-distribution: missing required {flag}").into())
}

fn describe(root: &Path, source_sha: &str, release_tag: &str) -> Result<Manifest, String> {
    if !is_source_sha(source_sha) {
        return Err("source SHA must be 40 lowercase hexadecimal characters".to_string());
    }
    if !is_release_tag(release_tag) {
        return Err("release tag must be a versioned v-prefixed tag".to_string());
    }
    if !root.is_dir() || root.is_symlink() {
        return Err("UI distribution must be a real directory".to_string());
    }
    let mut files = BTreeMap::new();
    for (relative, full) in sorted_files(root)? {
        if relative == MANIFEST {
            continue;
        }
        let bytes =
            fs::read(&full).map_err(|error| format!("cannot read {}: {error}", full.display()))?;
        files.insert(relative, hex::encode(Sha256::digest(&bytes)));
    }
    if !files.contains_key("index.html") {
        return Err("UI distribution is missing index.html".to_string());
    }
    let entry = root.join("index.html");
    let html = fs::read_to_string(&entry)
        .map_err(|error| format!("cannot read {}: {error}", entry.display()))?;
    if !has_built_module(&html, &files) {
        return Err("UI index must reference a built local JavaScript module".to_string());
    }
    Ok(Manifest {
        schema: 1,
        source_sha: source_sha.to_string(),
        release_tag: release_tag.to_string(),
        files,
    })
}

fn is_source_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_release_tag(value: &str) -> bool {
    let rest = match value.strip_prefix('v') {
        Some(rest) => rest,
        None => return false,
    };
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let valid_core = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    if !valid_core {
        return false;
    }
    match pre {
        None => true,
        Some(pre) => {
            !pre.is_empty()
                && pre
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        }
    }
}

fn sorted_files(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut found = Vec::new();
    collect_files(root, root, &mut found)?;
    found.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(found)
}

fn collect_files(
    root: &Path,
    dir: &Path,
    found: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|error| format!("cannot list {}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot list {}: {error}", dir.display()))?;
        let path = entry.path();
        let file_type = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot stat {}: {error}", path.display()))?
            .file_type();
        if file_type.is_symlink() {
            return Err("UI distribution must not contain symbolic links".to_string());
        }
        if file_type.is_dir() {
            collect_files(root, &path, found)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| format!("cannot relativize {}", path.display()))?;
            found.push((posix_relative(relative), path));
        }
    }
    Ok(())
}

fn posix_relative(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn has_built_module(html: &str, files: &BTreeMap<String, String>) -> bool {
    for (tag, attrs) in script_tags(&strip_comments(html)) {
        if tag.eq_ignore_ascii_case("script")
            && attrs.get("type").map(String::as_str) == Some("module")
            && let Some(src) = attrs.get("src")
        {
            let key = src.strip_prefix('/').unwrap_or(src);
            if key.ends_with(".js") && files.contains_key(key) {
                return true;
            }
        }
    }
    false
}

fn strip_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = &rest[start + "<!--".len()..];
        if let Some(end) = rest.find("-->") {
            rest = &rest[end + "-->".len()..];
        } else {
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn script_tags(html: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let mut tags = Vec::new();
    let bytes = html.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'<' || index + 1 >= bytes.len() {
            index += 1;
            continue;
        }
        let mut cursor = index + 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
            cursor += 1;
        }
        if cursor == index + 1 {
            index += 1;
            continue;
        }
        let tag = html[index + 1..cursor].to_string();
        let mut attrs = BTreeMap::new();
        cursor = parse_attributes(html, cursor, &mut attrs);
        if cursor < bytes.len() && bytes[cursor] == b'>' {
            tags.push((tag, attrs));
        }
        index = cursor + 1;
    }
    tags
}

fn parse_attributes(html: &str, mut cursor: usize, attrs: &mut BTreeMap<String, String>) -> usize {
    let bytes = html.as_bytes();
    loop {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] == b'>' {
            break;
        }
        if bytes[cursor] == b'/' {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && bytes[cursor] != b'='
            && bytes[cursor] != b'>'
            && bytes[cursor] != b'/'
        {
            cursor += 1;
        }
        if cursor == start {
            cursor += 1;
            continue;
        }
        let name = html[start..cursor].to_ascii_lowercase();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            attrs.insert(name, String::new());
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        attrs.insert(name, parse_attr_value(html, &mut cursor));
    }
    cursor
}

fn parse_attr_value(html: &str, cursor: &mut usize) -> String {
    let bytes = html.as_bytes();
    if *cursor < bytes.len() && (bytes[*cursor] == b'"' || bytes[*cursor] == b'\'') {
        let quote = bytes[*cursor];
        *cursor += 1;
        let start = *cursor;
        while *cursor < bytes.len() && bytes[*cursor] != quote {
            *cursor += 1;
        }
        let value = html[start..*cursor].to_string();
        if *cursor < bytes.len() {
            *cursor += 1;
        }
        value
    } else {
        let start = *cursor;
        while *cursor < bytes.len()
            && !bytes[*cursor].is_ascii_whitespace()
            && bytes[*cursor] != b'>'
        {
            *cursor += 1;
        }
        html[start..*cursor].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::unique_temp_dir;

    const SOURCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TAG: &str = "v0.74.0-rc.1";

    fn fixture_dist() -> PathBuf {
        let root = unique_temp_dir("ui-distribution");
        fs::create_dir_all(root.join("assets")).expect("fixture dirs");
        fs::write(root.join("assets/app.js"), "console.log('release');").expect("fixture js");
        fs::write(
            root.join("index.html"),
            r#"<script type="module" src="/assets/app.js"></script>"#,
        )
        .expect("fixture html");
        root
    }

    #[test]
    fn stamp_and_verify_round_trip() {
        let dist = fixture_dist();
        let manifest = describe(&dist, SOURCE, TAG).expect("describe");
        assert_eq!(
            manifest.files.keys().collect::<Vec<_>>(),
            vec![&"assets/app.js".to_string(), &"index.html".to_string()]
        );
        ui_distribution_stamp_command(&[
            "--dist".to_string(),
            dist.to_string_lossy().into_owned(),
            "--source-sha".to_string(),
            SOURCE.to_string(),
            "--release-tag".to_string(),
            TAG.to_string(),
        ])
        .expect("stamp");
        ui_distribution_verify_command(&[
            "--dist".to_string(),
            dist.to_string_lossy().into_owned(),
            "--source-sha".to_string(),
            SOURCE.to_string(),
            "--release-tag".to_string(),
            TAG.to_string(),
        ])
        .expect("verify");
    }

    #[test]
    fn wrong_identity_is_rejected() {
        let dist = fixture_dist();
        ui_distribution_stamp_command(&[
            "--dist".to_string(),
            dist.to_string_lossy().into_owned(),
            "--source-sha".to_string(),
            SOURCE.to_string(),
            "--release-tag".to_string(),
            TAG.to_string(),
        ])
        .expect("stamp");
        for (source, tag) in [
            ("b".repeat(40), TAG.to_string()),
            (SOURCE.to_string(), "v0.74.0".to_string()),
        ] {
            let error = ui_distribution_verify_command(&[
                "--dist".to_string(),
                dist.to_string_lossy().into_owned(),
                "--source-sha".to_string(),
                source,
                "--release-tag".to_string(),
                tag,
            ])
            .expect_err("must fail");
            assert!(
                error.to_string().contains("identity"),
                "unexpected: {error}"
            );
        }
    }

    #[test]
    fn changed_and_extra_files_are_rejected() {
        let dist = fixture_dist();
        let args = [
            "--dist".to_string(),
            dist.to_string_lossy().into_owned(),
            "--source-sha".to_string(),
            SOURCE.to_string(),
            "--release-tag".to_string(),
            TAG.to_string(),
        ];
        ui_distribution_stamp_command(&args).expect("stamp");
        fs::write(dist.join("assets/app.js"), "changed").expect("change");
        let error = ui_distribution_verify_command(&args).expect_err("must fail");
        assert!(
            error.to_string().contains("checksums"),
            "unexpected: {error}"
        );
        ui_distribution_stamp_command(&args).expect("stamp again");
        fs::write(dist.join("assets/unexpected.js"), "extra").expect("extra");
        let error = ui_distribution_verify_command(&args).expect_err("must fail");
        assert!(
            error.to_string().contains("checksums"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn missing_module_script_is_rejected() {
        let dist = fixture_dist();
        fs::remove_file(dist.join("assets/app.js")).expect("remove js");
        let error = describe(&dist, SOURCE, TAG).expect_err("must fail");
        assert!(error.contains("JavaScript"), "unexpected: {error}");
        fs::write(dist.join("index.html"), "<html></html>").expect("plain html");
        fs::write(dist.join("assets/app.js"), "console.log('release');").expect("restore js");
        let error = describe(&dist, SOURCE, TAG).expect_err("must fail");
        assert!(error.contains("JavaScript"), "unexpected: {error}");
    }

    #[test]
    fn missing_manifest_and_entry_are_rejected() {
        let dist = fixture_dist();
        let args = [
            "--dist".to_string(),
            dist.to_string_lossy().into_owned(),
            "--source-sha".to_string(),
            SOURCE.to_string(),
            "--release-tag".to_string(),
            TAG.to_string(),
        ];
        let error = ui_distribution_verify_command(&args).expect_err("must fail");
        assert!(
            error.to_string().contains("release manifest"),
            "unexpected: {error}"
        );
        fs::remove_file(dist.join("index.html")).expect("remove entry");
        let error = describe(&dist, SOURCE, TAG).expect_err("must fail");
        assert!(error.contains("index.html"), "unexpected: {error}");
    }

    #[test]
    #[cfg(unix)]
    fn symlinks_are_rejected() {
        let dist = fixture_dist();
        std::os::unix::fs::symlink(dist.join("assets/app.js"), dist.join("assets/link.js"))
            .expect("symlink");
        let error = describe(&dist, SOURCE, TAG).expect_err("must fail");
        assert!(error.contains("symbolic"), "unexpected: {error}");
    }

    #[test]
    fn identity_shape_is_checked() {
        let dist = fixture_dist();
        assert!(describe(&dist, "main", TAG).is_err());
        assert!(describe(&dist, SOURCE, "main").is_err());
        assert!(describe(&dist, &"A".repeat(40), TAG).is_err());
        assert!(is_release_tag("v1.2.3"));
        assert!(!is_release_tag("1.2.3"));
    }
}
