use jsonschema::{Retrieve, Uri, Validator};
use serde_json::Value;

const BASE: &str = "file:///services/contracts/kafka/";
const COMMON: &str = include_str!("../../contracts/kafka/common.schema.json");
const CONNECTIONS: &str = include_str!("../../contracts/kafka/dusk.connections.schema.json");
const CENSUS: &str = include_str!("../../contracts/kafka/dusk.census.schema.json");
const NODE_STATE: &str = include_str!("../../contracts/kafka/dusk.node-state.schema.json");
const INTENDED_PROCESSES: &str =
    include_str!("../../contracts/kafka/dusk.intended-processes.schema.json");
const CREDENTIAL_QUOTA: &str =
    include_str!("../../contracts/kafka/dusk.credential-quota.schema.json");

struct Common;

impl Retrieve for Common {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        if uri.as_str() == format!("{BASE}common.schema.json") {
            return Ok(serde_json::from_str(COMMON)?);
        }
        Err(format!("{uri} is not a dusk contract").into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contract {
    Connections,
    Census,
    NodeState,
    IntendedProcesses,
    CredentialQuota,
}

impl Contract {
    fn source(self) -> (&'static str, &'static str) {
        match self {
            Contract::Connections => ("dusk.connections.schema.json", CONNECTIONS),
            Contract::Census => ("dusk.census.schema.json", CENSUS),
            Contract::NodeState => ("dusk.node-state.schema.json", NODE_STATE),
            Contract::IntendedProcesses => {
                ("dusk.intended-processes.schema.json", INTENDED_PROCESSES)
            }
            Contract::CredentialQuota => ("dusk.credential-quota.schema.json", CREDENTIAL_QUOTA),
        }
    }

    pub fn validator(self) -> anyhow::Result<ContractValidator> {
        let (name, text) = self.source();
        let schema: Value = serde_json::from_str(text)?;
        let validator = jsonschema::options()
            .with_base_uri(format!("{BASE}{name}"))
            .with_retriever(Common)
            .build(&schema)
            .map_err(|error| anyhow::anyhow!("compile {name}: {error}"))?;
        Ok(ContractValidator { validator })
    }
}

pub struct ContractValidator {
    validator: Validator,
}

impl ContractValidator {
    pub fn check(&self, payload: &[u8]) -> Result<Value, String> {
        let value: Value = serde_json::from_slice(payload).map_err(|error| error.to_string())?;
        if let Some(error) = self.validator.iter_errors(&value).next() {
            return Err(format!("{error} at {}", error.instance_path()));
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn examples(topic: &str) -> Vec<(String, Vec<u8>)> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../contracts/kafka/examples")
            .join(topic);
        let mut examples: Vec<(String, Vec<u8>)> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    path.file_name().unwrap().to_string_lossy().to_string(),
                    std::fs::read(&path).unwrap(),
                )
            })
            .collect();
        examples.sort();
        examples
    }

    #[test]
    fn accepts_every_valid_example_and_refuses_every_invalid_one() {
        for (contract, topic) in [
            (Contract::Connections, "dusk.connections"),
            (Contract::Census, "dusk.census"),
            (Contract::NodeState, "dusk.node-state"),
            (Contract::IntendedProcesses, "dusk.intended-processes"),
            (Contract::CredentialQuota, "dusk.credential-quota"),
        ] {
            let validator = contract.validator().unwrap();
            let examples = examples(topic);
            assert!(examples.len() > 2, "{topic}");
            for (name, payload) in examples {
                let outcome = validator.check(&payload);
                assert_eq!(
                    outcome.is_err(),
                    name.starts_with("invalid-"),
                    "{topic}/{name}: {outcome:?}"
                );
            }
        }
    }

    #[test]
    fn refuses_what_is_not_json() {
        let validator = Contract::NodeState.validator().unwrap();
        assert!(validator.check(b"not json").is_err());
        assert!(validator.check(b"{}").is_err());
    }
}
