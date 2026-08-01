use serde_json::{json, Value};
use std::io::{self, Read};
use std::process::{Command, Output};

const DEFAULT_IMAGE: &str = "buzz-acp:local";
const PUBKEY_LABEL: &str = "com.buzz.managed-agent-pubkey";

fn main() {
    if let Err(error) = run() {
        eprintln!("buzz-backend-docker: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("failed to read request: {error}"))?;
    let request: Value = serde_json::from_str(input.trim())
        .map_err(|error| format!("request is not valid JSON: {error}"))?;

    let response = match request.get("op").and_then(Value::as_str) {
        Some("info") => provider_info(),
        Some("deploy") => deploy(&request)?,
        Some(operation) => {
            return Err(format!("unsupported operation '{operation}'"));
        }
        None => return Err("request is missing string field 'op'".to_string()),
    };

    println!(
        "{}",
        serde_json::to_string(&response).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn provider_info() -> Value {
    json!({
        "name": "Local Docker",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Runs a detached Buzz ACP harness in a local Docker container.",
        "config_schema": {
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "title": "Harness image",
                    "default": DEFAULT_IMAGE
                },
                "docker_context": {
                    "type": "string",
                    "title": "Docker context",
                    "description": "Optional Docker context; leave blank for the default daemon."
                },
                "network": {
                    "type": "string",
                    "title": "Docker network",
                    "description": "Optional Docker network for relay access."
                },
                "home_volume": {
                    "type": "string",
                    "title": "Agent home volume",
                    "description": "Optional Docker volume for subscription login state; blank uses a stable per-agent volume."
                }
            },
            "required": ["image"]
        }
    })
}

fn deploy(request: &Value) -> Result<Value, String> {
    let agent = request
        .get("agent")
        .ok_or_else(|| "deploy request is missing 'agent'".to_string())?;
    let config = request.get("provider_config").unwrap_or(&Value::Null);

    // Older official Desktop releases do not include `agent.pubkey` in the
    // provider payload yet. Derive the same stable identity from the agent's
    // private key so those releases can use this provider too. Newer Desktop
    // releases send the pubkey explicitly and avoid this parse.
    let pubkey = agent_pubkey(agent)?;
    let container_name = container_name(&pubkey)?;
    let image = config_string(config, "image").unwrap_or(DEFAULT_IMAGE);
    let context = config_string(config, "docker_context");
    let network = config_string(config, "network");
    let home_volume = config_string(config, "home_volume")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("buzz-agent-home-{}", &pubkey[..pubkey.len().min(32)]));
    validate_volume_name(&home_volume)?;

    if let Some(existing_id) = inspect_container(&container_name, context)? {
        if !container_running(&container_name, context)? {
            docker(context, ["start", container_name.as_str()])?;
        }
        return Ok(json!({ "agent_id": existing_id }));
    }

    let mut args = vec![
        "create".to_string(),
        "--name".to_string(),
        container_name.clone(),
        "--restart".to_string(),
        "unless-stopped".to_string(),
        "--label".to_string(),
        format!("{PUBKEY_LABEL}={pubkey}"),
    ];
    if let Some(network) = network {
        args.extend(["--network".to_string(), network.to_string()]);
    }
    args.extend([
        "--mount".to_string(),
        format!("type=volume,source={home_volume},target=/var/lib/buzz"),
    ]);
    add_agent_environment(&mut args, agent)?;
    args.push(image.to_string());

    let created = docker(context.as_deref(), args.iter().map(String::as_str))?;
    let container_id = output_text(&created)?;
    docker(context, ["start", container_name.as_str()])?;

    Ok(json!({ "agent_id": container_id }))
}

fn add_agent_environment(args: &mut Vec<String>, agent: &Value) -> Result<(), String> {
    let required_env = [
        ("BUZZ_PRIVATE_KEY", "private_key_nsec"),
        ("BUZZ_RELAY_URL", "relay_url"),
        ("BUZZ_ACP_AGENT_COMMAND", "agent_command"),
    ];
    for (name, field) in required_env {
        if let Some(value) = agent.get(field).and_then(Value::as_str) {
            if !value.is_empty() {
                add_env(args, name, value)?;
            }
        }
    }

    for (name, field) in [
        ("BUZZ_AUTH_TAG", "auth_tag"),
        ("BUZZ_ACP_SYSTEM_PROMPT", "system_prompt"),
        ("BUZZ_ACP_MODEL", "model"),
        ("BUZZ_ACP_IDLE_TIMEOUT", "idle_timeout_seconds"),
        ("BUZZ_ACP_MAX_TURN_DURATION", "max_turn_duration_seconds"),
    ] {
        if let Some(value) = agent.get(field).and_then(value_as_string) {
            add_env(args, name, &value)?;
        }
    }

    // Mirror Desktop's runtime metadata injection for the two runtimes whose
    // provider/model selection is environment-based. Custom runtimes can
    // receive their own settings through agent.env_vars.
    let runtime = agent
        .get("agent_command")
        .and_then(Value::as_str)
        .and_then(command_basename);
    match runtime.as_deref() {
        Some("buzz-agent") => {
            if let Some(provider) = agent.get("provider").and_then(Value::as_str) {
                add_env(args, "BUZZ_AGENT_PROVIDER", provider)?;
            }
            if let Some(model) = agent.get("model").and_then(Value::as_str) {
                add_env(args, "BUZZ_AGENT_MODEL", model)?;
            }
        }
        Some("goose") => {
            add_env(args, "GOOSE_MODE", "auto")?;
            if let Some(provider) = agent.get("provider").and_then(Value::as_str) {
                add_env(args, "GOOSE_PROVIDER", provider)?;
            }
            if let Some(model) = agent.get("model").and_then(Value::as_str) {
                add_env(args, "GOOSE_MODEL", model)?;
            }
        }
        _ => {}
    }

    if let Some(args_value) = agent.get("agent_args").and_then(Value::as_array) {
        let values = args_value
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if !values.is_empty() {
            add_env(args, "BUZZ_ACP_AGENT_ARGS", &values.join(","))?;
        }
    }
    if let Some(mcp_command) = agent.get("mcp_command").and_then(Value::as_str) {
        if !mcp_command.is_empty() {
            add_env(args, "BUZZ_ACP_MCP_COMMAND", mcp_command)?;
        }
    }
    if let Some(parallelism) = agent.get("parallelism").and_then(value_as_string) {
        add_env(args, "BUZZ_ACP_AGENTS", &parallelism)?;
    }
    if let Some(respond_to) = agent.get("respond_to").and_then(Value::as_str) {
        add_env(args, "BUZZ_ACP_RESPOND_TO", respond_to)?;
    }
    if let Some(allowlist) = agent.get("respond_to_allowlist").and_then(Value::as_array) {
        let values = allowlist
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if !values.is_empty() {
            add_env(args, "BUZZ_ACP_RESPOND_TO_ALLOWLIST", &values.join(","))?;
        }
    }

    // These are user/runtime-specific credentials and settings. They are
    // passed as separate Docker env arguments, never interpolated into a shell.
    if let Some(env_vars) = agent.get("env_vars").and_then(Value::as_object) {
        for (name, value) in env_vars {
            let value = value
                .as_str()
                .ok_or_else(|| format!("agent.env_vars.{name} must be a string"))?;
            add_env(args, name, value)?;
        }
    }

    Ok(())
}

fn add_env(args: &mut Vec<String>, name: &str, value: &str) -> Result<(), String> {
    if !valid_env_name(name) {
        return Err(format!("invalid environment variable name '{name}'"));
    }
    args.extend(["--env".to_string(), format!("{name}={value}")]);
    Ok(())
}

fn inspect_container(name: &str, context: Option<&str>) -> Result<Option<String>, String> {
    let output = docker(context, ["inspect", "--format={{.Id}}", name])?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(output_text(&output)?))
}

fn container_running(name: &str, context: Option<&str>) -> Result<bool, String> {
    let output = docker(context, ["inspect", "--format={{.State.Running}}", name])?;
    Ok(output.status.success() && output_text(&output)? == "true")
}

fn docker<'a, I>(context: Option<&str>, args: I) -> Result<Output, String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut command = Command::new(resolve_docker_binary()?);
    if let Some(context) = context.filter(|value| !value.is_empty()) {
        command.args(["--context", context]);
    }
    command.args(args);
    command
        .output()
        .map_err(|error| format!("failed to execute docker: {error}"))
}

/// Resolve Docker independently of the GUI process's minimal PATH.
///
/// macOS applications launched from Finder commonly inherit only
/// `/usr/bin:/bin:/usr/sbin:/sbin`, while Docker Desktop exposes its CLI via
/// `/usr/local/bin` or directly inside the application bundle.
fn resolve_docker_binary() -> Result<std::path::PathBuf, String> {
    let mut candidates: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join("docker"))
        .collect();

    #[cfg(target_os = "macos")]
    candidates.extend([
        std::path::PathBuf::from("/opt/homebrew/bin/docker"),
        std::path::PathBuf::from("/usr/local/bin/docker"),
        std::path::PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
    ]);

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "docker executable not found; install Docker Desktop or configure its CLI path"
                .to_string()
        })
}

fn output_text(output: &Output) -> Result<String, String> {
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        let detail = error.trim();
        return Err(if detail.is_empty() {
            format!("docker exited with status {}", output.status)
        } else {
            format!("docker failed: {detail}")
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("agent is missing non-empty string field '{field}'"))
}

fn agent_pubkey(agent: &Value) -> Result<String, String> {
    if let Some(pubkey) = agent
        .get("pubkey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(pubkey.to_string());
    }

    let private_key = required_string(agent, "private_key_nsec")?;
    let keys = nostr::Keys::parse(private_key)
        .map_err(|error| format!("agent pubkey is missing and private key is invalid: {error}"))?;
    Ok(keys.public_key().to_hex())
}

fn config_string<'a>(config: &'a Value, field: &str) -> Option<&'a str> {
    config
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn value_as_string(value: &Value) -> Option<String> {
    value.as_str().map(String::from).or_else(|| {
        value
            .as_u64()
            .map(|number| number.to_string())
            .or_else(|| value.as_i64().map(|number| number.to_string()))
    })
}

fn command_basename(command: &str) -> Option<String> {
    command
        .trim()
        .rsplit(|character| character == '/' || character == '\\')
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn container_name(pubkey: &str) -> Result<String, String> {
    if pubkey.len() < 8
        || !pubkey
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err("agent pubkey must be an ASCII identifier".to_string());
    }
    Ok(format!("buzz-agent-{}", &pubkey[..pubkey.len().min(32)]))
}

fn valid_env_name(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn validate_volume_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
        })
        || !name
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
    {
        return Err("home_volume must be a valid Docker volume name".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::ToBech32;

    #[test]
    fn info_exposes_docker_configuration() {
        let info = provider_info();
        assert_eq!(info["name"], "Local Docker");
        assert_eq!(
            info["config_schema"]["properties"]["image"]["default"],
            DEFAULT_IMAGE
        );
    }

    #[test]
    fn container_name_is_stable_and_bounded() {
        let pubkey = "a".repeat(64);
        assert_eq!(
            container_name(&pubkey).unwrap(),
            format!("buzz-agent-{}", "a".repeat(32))
        );
    }

    #[test]
    fn derives_pubkey_for_older_desktop_payloads() {
        let keys = nostr::Keys::generate();
        let agent = json!({
            "private_key_nsec": keys.secret_key().to_bech32().unwrap(),
        });
        assert_eq!(agent_pubkey(&agent).unwrap(), keys.public_key().to_hex());
    }

    #[test]
    fn prefers_explicit_pubkey() {
        let agent = json!({
            "pubkey": "aabbccdd",
            "private_key_nsec": "not-used",
        });
        assert_eq!(agent_pubkey(&agent).unwrap(), "aabbccdd");
    }

    #[test]
    fn environment_names_are_strict() {
        assert!(valid_env_name("BUZZ_AGENT_MODEL"));
        assert!(valid_env_name("_PRIVATE"));
        assert!(!valid_env_name("bad-name"));
        assert!(!valid_env_name("1BAD"));
    }

    #[test]
    fn volume_names_are_strict() {
        assert!(validate_volume_name("buzz-agent-home-a").is_ok());
        assert!(validate_volume_name("my_volume.v1").is_ok());
        assert!(validate_volume_name("../host-path").is_err());
        assert!(validate_volume_name("-invalid").is_err());
    }

    #[test]
    fn buzz_agent_receives_custom_provider_and_model() {
        let agent = json!({
            "agent_command": "buzz-agent",
            "provider": "my-provider",
            "model": "my-model",
        });
        let mut args = Vec::new();
        add_agent_environment(&mut args, &agent).unwrap();
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--env" && pair[1] == "BUZZ_AGENT_PROVIDER=my-provider"));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--env" && pair[1] == "BUZZ_AGENT_MODEL=my-model"));
    }
}
