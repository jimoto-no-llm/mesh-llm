//! `mesh-llm jimoto` handler: fork identity and pointers.

use anyhow::Result;
use serde::Serialize;
use std::io::Write;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JimotoInfo {
    pub fork: String,
    pub version: String,
    pub upstream: String,
    pub guide: String,
    pub issues: String,
}

pub fn jimoto_info() -> JimotoInfo {
    JimotoInfo {
        fork: "jimoto-no-llm/mesh-llm".to_string(),
        version: mesh_llm_build_info::BUILD_VERSION.to_string(),
        upstream: "https://github.com/Mesh-LLM/mesh-llm".to_string(),
        guide: "https://github.com/jimoto-no-llm/mesh-llm/blob/main/JIMOTO.md".to_string(),
        issues: "https://github.com/jimoto-no-llm/mesh-llm/issues".to_string(),
    }
}

pub fn run_jimoto_command(json_output: bool) -> Result<()> {
    let info = jimoto_info();
    if json_output {
        let mut out = mesh_llm_events::machine_out();
        writeln!(out, "{}", serde_json::to_string_pretty(&info)?)?;
        return Ok(());
    }
    let mut out = mesh_llm_events::console_out();
    writeln!(out, "mesh-llm fork {} {}", info.fork, info.version)?;
    writeln!(out, "upstream: {}", info.upstream)?;
    writeln!(out, "guide: {}", info.guide)?;
    writeln!(out, "issues: {}", info.issues)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jimoto_info_points_at_fork_and_upstream() {
        let info = jimoto_info();
        assert_eq!(info.fork, "jimoto-no-llm/mesh-llm");
        assert!(!info.version.is_empty());
        assert!(info.upstream.contains("Mesh-LLM/mesh-llm"));
        assert!(info.guide.contains("jimoto-no-llm/mesh-llm"));
        assert!(info.issues.contains("jimoto-no-llm/mesh-llm"));
        let json = serde_json::to_value(&info).expect("info serializes");
        assert_eq!(json["fork"], serde_json::json!("jimoto-no-llm/mesh-llm"));
    }
}
