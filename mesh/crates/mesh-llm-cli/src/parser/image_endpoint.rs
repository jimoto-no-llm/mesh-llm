/// Parse a `NAME=URL` pair for `--image-endpoint`. Splits on the first `=`
/// only; the runtime validates the name and URL before serving.
pub(super) fn parse_image_endpoint_pair(s: &str) -> Result<(String, String), String> {
    let Some((name, url)) = s.split_once('=') else {
        return Err("expected NAME=URL, no '=' separator found".to_string());
    };
    if name.is_empty() {
        return Err("expected NAME=URL, got an empty model name".to_string());
    }
    if url.is_empty() {
        return Err(format!("expected NAME=URL, got an empty URL for {name:?}"));
    }
    Ok((name.to_string(), url.to_string()))
}

#[cfg(test)]
mod tests {
    use super::parse_image_endpoint_pair;
    use crate::parser::{Cli, normalize_runtime_surface_args};
    use clap::Parser;

    #[test]
    fn splits_on_the_first_separator() {
        assert_eq!(
            parse_image_endpoint_pair("sd-turbo=http://127.0.0.1:1234/v1").unwrap(),
            (
                "sd-turbo".to_string(),
                "http://127.0.0.1:1234/v1".to_string()
            )
        );
        assert_eq!(
            parse_image_endpoint_pair("flux=http://127.0.0.1:1234/v1?x=y").unwrap(),
            (
                "flux".to_string(),
                "http://127.0.0.1:1234/v1?x=y".to_string()
            )
        );
    }

    #[test]
    fn rejects_missing_parts() {
        assert!(
            parse_image_endpoint_pair("http://127.0.0.1:1234/v1")
                .unwrap_err()
                .contains("NAME=URL")
        );
        assert!(
            parse_image_endpoint_pair("=http://127.0.0.1:1234/v1")
                .unwrap_err()
                .contains("empty model name")
        );
        assert!(
            parse_image_endpoint_pair("sd-turbo=")
                .unwrap_err()
                .contains("empty URL")
        );
    }

    #[test]
    fn serve_accepts_repeated_image_endpoints() {
        let normalized = normalize_runtime_surface_args([
            "mesh-llm",
            "serve",
            "--image-endpoint",
            "sd-turbo=http://127.0.0.1:1234/v1",
            "--image-endpoint=flux=http://127.0.0.1:1235/v1",
        ]);
        let cli = Cli::try_parse_from(normalized.normalized).unwrap();
        assert_eq!(
            cli.image_endpoint,
            vec![
                (
                    "sd-turbo".to_string(),
                    "http://127.0.0.1:1234/v1".to_string()
                ),
                ("flux".to_string(), "http://127.0.0.1:1235/v1".to_string()),
            ]
        );
    }

    /// The flag takes a value, so `serve` after it is still the surface.
    #[test]
    fn image_endpoint_value_before_serve_is_not_the_subcommand() {
        let normalized = normalize_runtime_surface_args([
            "mesh-llm",
            "--image-endpoint",
            "sd-turbo=http://127.0.0.1:1234/v1",
            "serve",
        ]);
        let cli = Cli::try_parse_from(normalized.normalized).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.image_endpoint.len(), 1);
    }

    #[test]
    fn client_and_local_model_only_reject_image_endpoints() {
        for surface in [
            vec!["mesh-llm", "client"],
            vec![
                "mesh-llm",
                "serve",
                "--local-model-only",
                "--model",
                "/m.gguf",
            ],
        ] {
            let mut args = surface.clone();
            args.extend(["--image-endpoint", "sd-turbo=http://127.0.0.1:1234/v1"]);
            let normalized = normalize_runtime_surface_args(args);
            assert!(
                Cli::try_parse_from(normalized.normalized).is_err(),
                "{surface:?} must reject --image-endpoint"
            );
        }
    }
}
