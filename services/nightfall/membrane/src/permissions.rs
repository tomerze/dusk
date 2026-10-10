use anyhow::{Context, bail};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

const QUARANTINE_ROLE: &str = "quarantine";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionsFile {
    #[serde(default)]
    deny_certificates: Vec<String>,
    #[serde(default)]
    deny_principals: Vec<String>,
    #[serde(default, rename = "role")]
    roles: Vec<RoleFile>,
    #[serde(default, rename = "principal")]
    principals: Vec<PrincipalFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleFile {
    name: String,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    reverse_allow: Vec<String>,
    #[serde(default)]
    quarantine_override: bool,
    #[serde(default)]
    admission_exempt: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrincipalFile {
    name: String,
    roles: Vec<String>,
}

pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut pattern_index, mut text_index) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while text_index < text.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            backtrack = Some((pattern_index, text_index));
            pattern_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == text[text_index] {
            pattern_index += 1;
            text_index += 1;
        } else if let Some((star, matched)) = backtrack {
            pattern_index = star + 1;
            text_index = matched + 1;
            backtrack = Some((star, matched + 1));
        } else {
            return false;
        }
    }
    pattern[pattern_index..].iter().all(|byte| *byte == b'*')
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MethodPattern {
    interface: String,
    method: String,
}

impl MethodPattern {
    fn parse(text: &str) -> anyhow::Result<MethodPattern> {
        let valid = !text.is_empty()
            && text.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'*' || byte == b'.'
            })
            && text.split('.').all(|part| !part.is_empty());
        let Some((interface, method)) = text.rsplit_once('.').filter(|_| valid) else {
            bail!("'{text}' is not an <Interface>.<method> pattern");
        };
        Ok(MethodPattern {
            interface: interface.to_string(),
            method: method.to_string(),
        })
    }

    fn matches(&self, interface: &str, method: &str) -> bool {
        glob_matches(&self.interface, interface) && glob_matches(&self.method, method)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Rules {
    allow: Vec<MethodPattern>,
    deny: Vec<MethodPattern>,
    reverse_allow: Vec<MethodPattern>,
}

impl Rules {
    fn allows(&self, interface: &str, method: &str) -> bool {
        self.allow
            .iter()
            .any(|pattern| pattern.matches(interface, method))
            && !self.denies(interface, method)
    }

    fn denies(&self, interface: &str, method: &str) -> bool {
        self.deny
            .iter()
            .any(|pattern| pattern.matches(interface, method))
    }

    fn reverse_allows(&self, interface: &str, method: &str) -> bool {
        self.reverse_allow
            .iter()
            .any(|pattern| pattern.matches(interface, method))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Role {
    name: String,
    rules: Rules,
    quarantine_override: bool,
    admission_exempt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    AllowedByOverride,
    Denied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    roles: Vec<Role>,
    quarantine: Option<Rules>,
}

impl Policy {
    fn decide(&self, interface: &str, method: &str, reverse: bool) -> Decision {
        if !reverse
            && (self
                .roles
                .iter()
                .any(|role| role.rules.denies(interface, method))
                || self
                    .quarantine
                    .as_ref()
                    .is_some_and(|quarantine| quarantine.denies(interface, method)))
        {
            return Decision::Denied;
        }
        let mut decision = Decision::Denied;
        for role in &self.roles {
            let granted = if reverse {
                role.rules.reverse_allows(interface, method)
            } else {
                role.rules.allows(interface, method)
            };
            if !granted {
                continue;
            }
            match &self.quarantine {
                None => return Decision::Allowed,
                Some(quarantine) => {
                    let quarantine_grants = if reverse {
                        quarantine.reverse_allows(interface, method)
                    } else {
                        quarantine.allows(interface, method)
                    };
                    if quarantine_grants {
                        return Decision::Allowed;
                    }
                    if role.quarantine_override {
                        decision = Decision::AllowedByOverride;
                    }
                }
            }
        }
        decision
    }

    pub fn allows(&self, interface: &str, method: &str) -> Decision {
        self.decide(interface, method, false)
    }

    pub fn reverse_allows(&self, interface: &str, method: &str) -> Decision {
        self.decide(interface, method, true)
    }

    pub fn quarantined(&self) -> bool {
        self.quarantine.is_some()
    }

    pub fn role_names(&self) -> Vec<&str> {
        self.roles.iter().map(|role| role.name.as_str()).collect()
    }

    pub fn admission_exempt_role(&self) -> Option<&str> {
        self.roles
            .iter()
            .find(|role| role.admission_exempt)
            .map(|role| role.name.as_str())
    }
}

fn parse_certificate_digest(text: &str) -> anyhow::Result<[u8; 32]> {
    let hexadecimal: String = text.chars().filter(|character| *character != ':').collect();
    if hexadecimal.len() != 64 || !hexadecimal.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("'{text}' is not a SHA-256 digest of 64 hexadecimal characters");
    }
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hexadecimal[index * 2..index * 2 + 2], 16)?;
    }
    Ok(digest)
}

#[derive(Debug)]
pub struct Permissions {
    deny_certificates: HashSet<[u8; 32]>,
    deny_principals: Vec<String>,
    roles: HashMap<String, Role>,
    principals: Vec<(String, Vec<String>)>,
}

impl Permissions {
    pub fn from_toml(text: &str) -> anyhow::Result<Permissions> {
        let file: PermissionsFile = toml::from_str(text).context("parse the permissions file")?;
        let mut deny_certificates = HashSet::new();
        for digest in &file.deny_certificates {
            deny_certificates
                .insert(parse_certificate_digest(digest).context("deny_certificates")?);
        }
        let mut roles = HashMap::new();
        for role in file.roles {
            let parse = |patterns: &[String], list: &str| -> anyhow::Result<Vec<MethodPattern>> {
                patterns
                    .iter()
                    .map(|pattern| MethodPattern::parse(pattern))
                    .collect::<anyhow::Result<_>>()
                    .with_context(|| format!("role {}: {list}", role.name))
            };
            let rules = Rules {
                allow: parse(&role.allow, "allow")?,
                deny: parse(&role.deny, "deny")?,
                reverse_allow: parse(&role.reverse_allow, "reverse_allow")?,
            };
            if rules
                .allow
                .iter()
                .chain(&rules.reverse_allow)
                .any(|pattern| pattern.interface == "*" && pattern.method == "*")
            {
                tracing::warn!(role = %role.name, "a role allows *.*, every method of every interface");
            }
            let name = role.name.clone();
            let parsed = Role {
                name: role.name,
                rules,
                quarantine_override: role.quarantine_override,
                admission_exempt: role.admission_exempt,
            };
            if roles.insert(name.clone(), parsed).is_some() {
                bail!("the role {name} is defined twice");
            }
        }
        let mut principals = Vec::new();
        for principal in file.principals {
            for role in &principal.roles {
                if !roles.contains_key(role) {
                    bail!(
                        "principal {} names the undefined role {role}",
                        principal.name
                    );
                }
            }
            principals.push((principal.name, principal.roles));
        }
        Ok(Permissions {
            deny_certificates,
            deny_principals: file.deny_principals,
            roles,
            principals,
        })
    }

    pub fn policy_for(&self, principal: &str, quarantined: bool) -> Option<Policy> {
        if self.denies_principal(principal) {
            return None;
        }
        let mut roles: Vec<Role> = Vec::new();
        for (pattern, role_names) in &self.principals {
            if !glob_matches(pattern, principal) {
                continue;
            }
            for role_name in role_names {
                if roles.iter().all(|role| role.name != *role_name)
                    && let Some(role) = self.roles.get(role_name)
                {
                    roles.push(role.clone());
                }
            }
        }
        if roles.is_empty() {
            return None;
        }
        let quarantine = quarantined.then(|| {
            self.roles
                .get(QUARANTINE_ROLE)
                .map(|role| role.rules.clone())
                .unwrap_or_default()
        });
        Some(Policy { roles, quarantine })
    }

    pub fn denies_certificate(&self, sha256_der: &[u8; 32]) -> bool {
        self.deny_certificates.contains(sha256_der)
    }

    pub fn denies_principal(&self, principal: &str) -> bool {
        self.deny_principals
            .iter()
            .any(|pattern| glob_matches(pattern, principal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIPPED: &str = r#"
deny_certificates = ["00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"]
deny_principals = ["dawn-evil*"]
[[role]]
name = "dawn"
allow = ["Dusk.process", "Dusk.run", "Dusk.ps", "Dusk.kill", "Dusk.hostname", "Dusk.waitpid", "Dusk.time", "Dusk.programs", "Dusk.namespaceId", "Dusk.dusk", "Process.*", "Portal.*", "OutputPortal.*", "ShPortal.*", "KvsPortal.get", "KvsPortal.exists", "KvsPortal.scan", "LogsPortal.*", "CpPortal.*", "SignalBatch.Ack.ack"]
deny = ["Dusk.settime", "Dusk.fleetToken"]
reverse_allow = ["Stream.*", "Created.created", "Sink.*", "LogsArgs.Server.openStream", "LogsArgs.Stream.*", "CpArgs.Server.write", "CpArgs.Server.stat", "ShStop.stop"]
quarantine_override = false
[[role]]
name = "quarantine"
allow = ["Dusk.hostname", "Dusk.programs", "Dusk.namespaceId", "Dusk.time"]
reverse_allow = []
[[role]]
name = "operator"
allow = ["Dusk.*"]
deny = ["Dusk.settime"]
reverse_allow = ["Stream.*"]
quarantine_override = true
[[principal]]
name = "dawn-*"
roles = ["dawn"]
[[principal]]
name = "operator-*"
roles = ["operator"]
"#;

    fn shipped() -> Permissions {
        Permissions::from_toml(SHIPPED).unwrap()
    }

    #[test]
    fn matches_globs() {
        assert!(glob_matches("dawn-*", "dawn-0"));
        assert!(glob_matches("*", ""));
        assert!(glob_matches("a*b*c", "aXXbYYc"));
        assert!(!glob_matches("a*b*c", "aXXbYY"));
        assert!(glob_matches("LogsArgs.*", "LogsArgs.Server"));
        assert!(!glob_matches("dawn-*", "twilight-0"));
        assert!(!glob_matches("Dusk", "Dusks"));
    }

    #[test]
    fn allows_listed_methods_and_denies_the_rest() {
        let policy = shipped().policy_for("dawn-3", false).unwrap();
        assert_eq!(policy.allows("Dusk", "process"), Decision::Allowed);
        assert_eq!(policy.allows("ShPortal", "sh"), Decision::Allowed);
        assert_eq!(policy.allows("KvsPortal", "get"), Decision::Allowed);
        assert_eq!(policy.allows("KvsPortal", "set"), Decision::Denied);
        assert_eq!(policy.allows("Dusk", "dusk"), Decision::Allowed);
        assert_eq!(policy.allows("Dusk", "fleetToken"), Decision::Denied);
        assert_eq!(policy.allows("Dusk", "settime"), Decision::Denied);
        assert_eq!(policy.allows("Nightfall", "job"), Decision::Denied);
        assert_eq!(policy.allows("JobPortal", "status"), Decision::Denied);
        assert_eq!(policy.allows("Unknown", "anything"), Decision::Denied);
        assert!(!policy.quarantined());
    }

    #[test]
    fn deny_wins_over_allow() {
        let policy = shipped().policy_for("operator-1", false).unwrap();
        assert_eq!(policy.allows("Dusk", "process"), Decision::Allowed);
        assert_eq!(policy.allows("Dusk", "settime"), Decision::Denied);
        let combined = Permissions::from_toml(
            r#"
[[role]]
name = "wide"
allow = ["Dusk.*"]
[[role]]
name = "narrow"
allow = ["Dusk.ps"]
deny = ["Dusk.kill"]
[[principal]]
name = "both"
roles = ["wide", "narrow"]
"#,
        )
        .unwrap();
        let policy = combined.policy_for("both", false).unwrap();
        assert_eq!(policy.allows("Dusk", "kill"), Decision::Denied);
        assert_eq!(policy.allows("Dusk", "run"), Decision::Allowed);
    }

    #[test]
    fn governs_node_to_client_calls_by_reverse_allow_only() {
        let policy = shipped().policy_for("dawn-0", false).unwrap();
        assert_eq!(policy.reverse_allows("Stream", "send"), Decision::Allowed);
        assert_eq!(
            policy.reverse_allows("LogsArgs.Server", "openStream"),
            Decision::Allowed
        );
        assert_eq!(
            policy.reverse_allows("CpArgs.Server", "read"),
            Decision::Denied
        );
        assert_eq!(policy.reverse_allows("ShStop", "stop"), Decision::Allowed);
        assert_eq!(policy.reverse_allows("Dusk", "process"), Decision::Denied);
        assert_eq!(policy.allows("Stream", "send"), Decision::Denied);
        assert_eq!(
            policy.reverse_allows("SignalBatch.Ack", "ack"),
            Decision::Denied
        );
        assert_eq!(policy.allows("SignalBatch.Ack", "ack"), Decision::Allowed);
    }

    #[test]
    fn intersects_with_the_quarantine_role_for_quarantined_nodes() {
        let permissions = shipped();
        let policy = permissions.policy_for("dawn-0", true).unwrap();
        assert!(policy.quarantined());
        assert_eq!(policy.allows("Dusk", "hostname"), Decision::Allowed);
        assert_eq!(policy.allows("Dusk", "dusk"), Decision::Denied);
        assert_eq!(policy.allows("Dusk", "process"), Decision::Denied);
        assert_eq!(policy.allows("ShPortal", "sh"), Decision::Denied);
        assert_eq!(policy.reverse_allows("Stream", "send"), Decision::Denied);
        assert_ne!(policy, permissions.policy_for("dawn-0", false).unwrap());
    }

    #[test]
    fn lets_an_override_role_through_quarantine_and_says_so() {
        let policy = shipped().policy_for("operator-0", true).unwrap();
        assert_eq!(policy.allows("Dusk", "hostname"), Decision::Allowed);
        assert_eq!(
            policy.allows("Dusk", "process"),
            Decision::AllowedByOverride
        );
        assert_eq!(policy.allows("Dusk", "settime"), Decision::Denied);
        assert_eq!(
            policy.reverse_allows("Stream", "send"),
            Decision::AllowedByOverride
        );
    }

    #[test]
    fn names_the_role_that_exempts_a_principal_from_admission() {
        assert_eq!(
            shipped()
                .policy_for("operator-0", false)
                .unwrap()
                .admission_exempt_role(),
            None
        );
        let permissions = Permissions::from_toml(
            r#"
[[role]]
name = "reader"
allow = ["Dusk.ps"]
[[role]]
name = "break-glass"
allow = ["Dusk.*"]
admission_exempt = true
[[principal]]
name = "operator-*"
roles = ["reader", "break-glass"]
[[principal]]
name = "dawn-*"
roles = ["reader"]
"#,
        )
        .unwrap();
        let exempt = permissions.policy_for("operator-1", false).unwrap();
        assert_eq!(exempt.admission_exempt_role(), Some("break-glass"));
        assert_eq!(
            permissions
                .policy_for("dawn-0", false)
                .unwrap()
                .admission_exempt_role(),
            None
        );
    }

    #[test]
    fn refuses_unknown_and_denied_principals() {
        let permissions = shipped();
        assert!(permissions.policy_for("twilight-0", false).is_none());
        assert!(permissions.policy_for("dawn-evil-1", false).is_none());
        assert!(permissions.denies_principal("dawn-evil-1"));
        assert!(!permissions.denies_principal("dawn-1"));
    }

    #[test]
    fn denies_listed_certificates() {
        let permissions = shipped();
        let mut digest = [0u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = ((index % 16) as u8) * 0x11;
        }
        assert!(permissions.denies_certificate(&digest));
        assert!(!permissions.denies_certificate(&[0u8; 32]));
        let colons = Permissions::from_toml(&format!(
            "deny_certificates = [\"{}\"]",
            digest
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(":")
        ))
        .unwrap();
        assert!(colons.denies_certificate(&digest));
    }

    #[test]
    fn refuses_malformed_files() {
        for text in [
            "[[principal]]\nname = \"x\"\nroles = [\"missing\"]",
            "[[role]]\nname = \"a\"\nallow = [\"Dusk\"]",
            "[[role]]\nname = \"a\"\nallow = [\"Dusk..ps\"]",
            "[[role]]\nname = \"a\"\nallow = [\"Dusk.p s\"]",
            "[[role]]\nname = \"a\"\n[[role]]\nname = \"a\"",
            "deny_certificates = [\"abc\"]",
            "unknown_key = 1",
            "[[role]]\nname = \"a\"\nallows = []",
        ] {
            assert!(Permissions::from_toml(text).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn treats_a_missing_quarantine_role_as_denying_everything() {
        let permissions = Permissions::from_toml(
            "[[role]]\nname = \"a\"\nallow = [\"Dusk.*\"]\n[[principal]]\nname = \"p\"\nroles = [\"a\"]",
        )
        .unwrap();
        let policy = permissions.policy_for("p", true).unwrap();
        assert_eq!(policy.allows("Dusk", "hostname"), Decision::Denied);
    }
}
