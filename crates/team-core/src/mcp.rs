//! Provider-independent MCP transports and credential references.

use crate::compose::{Agent, McpServer, SandboxType};

pub fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn validate_server(server: &McpServer) -> Result<(), String> {
    if let Some(endpoint) = &server.url {
        if !server.command.is_empty()
            || !server.args.is_empty()
            || !server.env.is_empty()
            || !server.env_vars.is_empty()
        {
            return Err("HTTP MCP uses url, without command/args/env/env_vars".into());
        }
        let url =
            url::Url::parse(endpoint).map_err(|_| "MCP URL must be an absolute HTTPS endpoint")?;
        let local = url.host_str().is_some_and(|h| {
            h == "localhost"
                || h.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if url.scheme() != "https" && !(url.scheme() == "http" && local) {
            return Err("MCP requires HTTPS (HTTP is allowed only for loopback hosts)".into());
        }
        if url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query().is_some()
        {
            return Err("MCP URL must have a host and contain no user credentials, query or fragment; use environment authentication references".into());
        }
    } else if server.command.trim().is_empty() {
        return Err("MCP requires exactly one of command or url".into());
    } else if server.bearer_token_env_var.is_some() || !server.env_http_headers.is_empty() {
        return Err("HTTP authentication fields require url".into());
    }
    for name in server.required_env_vars() {
        if !valid_env_name(name) {
            return Err(format!("invalid environment variable name `{name}`"));
        }
    }
    for name in &server.env_vars {
        if server.env.contains_key(name) {
            return Err(format!(
                "environment `{name}` is both a literal value and a forwarded reference"
            ));
        }
    }
    for header in server.env_http_headers.keys() {
        if header.is_empty()
            || !header
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        {
            return Err(format!("invalid HTTP header name `{header}`"));
        }
        if header.eq_ignore_ascii_case("authorization") && server.bearer_token_env_var.is_some() {
            return Err("Authorization cannot be configured through both bearer_token_env_var and env_http_headers".into());
        }
    }
    Ok(())
}

/// Launch preflight: report credential names only, without writing or logging values.
pub fn validate_mcp_environment(
    agent: &Agent,
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> Result<(), String> {
    for (name, server) in &agent.mcps {
        for variable in server.required_env_vars() {
            let value=lookup(variable).filter(|value|!value.trim().is_empty()).ok_or_else(||format!("MCP `{name}` requires environment variable `{variable}`; set it in the team .env or launch environment"))?;
            if server.url.is_some() && value.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
                return Err(format!("MCP `{name}` credential reference `{variable}` contains an invalid header character"));
            }
            if server.url.is_some()
                && agent.runtime == "claude-code"
                && matches!(
                    variable,
                    "ANTHROPIC_API_KEY"
                        | "ANTHROPIC_AUTH_TOKEN"
                        | "AWS_BEARER_TOKEN_BEDROCK"
                        | "NPM_TOKEN"
                        | "HTTPS_PROXY"
                )
            {
                return Err(format!("Claude Code protects `{variable}` from remote MCP expansion; copy the credential into a dedicated MCP variable and reference that name"));
            }
            if agent.execution.as_ref().is_some_and(|e| {
                e.sandbox.r#type == SandboxType::Docker
                    && !e.sandbox.env.iter().any(|v| v == variable)
            }) {
                return Err(format!("MCP `{name}` requires Docker environment grant `{variable}` in execution.sandbox.env"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_ambiguous_transports_insecure_endpoints_and_invalid_auth_refs() {
        for yaml in [
            "{}", "command: mcp\nurl: https://mcp.example.com", "url: http://remote.example.com/mcp",
            "url: https://token@example.com/mcp", "url: https://example.com/mcp?api_key=secret",
            "url: https://example.com/mcp\nenv_http_headers: {Authorization: 'NOT AN ENV'}",
            "url: https://example.com/mcp\nbearer_token_env_var: TOKEN\nenv_http_headers: {authorization: TOKEN}",
            "command: mcp\nenv_vars: [TOKEN]\nenv: {TOKEN: secret}",
        ] {
            let server:McpServer=serde_yaml::from_str(yaml).unwrap();assert!(validate_server(&server).is_err(),"{yaml}");
        }
        for endpoint in [
            "http://127.0.0.1:3456/mcp",
            "http://[::1]:3456/mcp",
            "http://localhost/mcp",
            "https://mcp.example.com/mcp",
        ] {
            let server = McpServer {
                url: Some(endpoint.into()),
                ..Default::default()
            };
            assert!(validate_server(&server).is_ok(), "{endpoint}");
        }
    }
    #[test]
    fn missing_secrets_and_container_grants_fail_without_disclosing_values() {
        let agent:Agent=serde_yaml::from_str("runtime: codex\nmcps:\n  remote:\n    url: https://mcp.example.com/mcp\n    bearer_token_env_var: MCP_TOKEN\n").unwrap();
        assert!(validate_mcp_environment(&agent, |_| None)
            .unwrap_err()
            .contains("MCP_TOKEN"));
        assert!(validate_mcp_environment(&agent, |_| Some("secret-canary".into())).is_ok());
        let mut agent = agent;
        agent.execution = Some(
            serde_yaml::from_str("mode: native\nsandbox: {type: docker,image: teamctl:test}\n")
                .unwrap(),
        );
        let error = validate_mcp_environment(&agent, |_| Some("secret-canary".into())).unwrap_err();
        assert!(error.contains("execution.sandbox.env"));
        assert!(!error.contains("secret-canary"));
        agent
            .execution
            .as_mut()
            .unwrap()
            .sandbox
            .env
            .push("MCP_TOKEN".into());
        assert!(validate_mcp_environment(&agent, |_| Some("secret-canary".into())).is_ok());
    }
}
