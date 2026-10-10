use serde_json::{Map, Value};

use crate::canonical::CanonicalError;
use crate::entry::{
    CHAIN_START_HASH, ChainFields, EntryContent, Event, Kind, LedgerEntry, NIGHTFALL_PRINCIPAL,
    entry_hash,
};
use crate::signing::{CheckpointSigner, checkpoint_message};

#[derive(Debug, thiserror::Error)]
pub enum ChainError {
    #[error("the partition tail is not a ledger entry: {0}")]
    UnreadableTail(String),
    #[error("the partition tail's hash {stored} does not match its content ({computed})")]
    TamperedTail { stored: String, computed: String },
    #[error("the partition tail names partition {found}, this chain writes partition {expected}")]
    WrongPartition { found: u32, expected: u32 },
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Continuation {
    Start,
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub sequence: u64,
    pub hash: String,
}

pub struct Chain {
    instance: String,
    partition: u32,
    next_sequence: u64,
    previous_hash: String,
    tail_signed: bool,
}

impl Chain {
    pub fn start(
        instance: &str,
        partition: u32,
        tail: Option<&[u8]>,
        continuation: Continuation,
    ) -> Result<(Chain, Option<EntryContent>), ChainError> {
        let mut chain = Chain {
            instance: String::from(instance),
            partition,
            next_sequence: 0,
            previous_hash: String::from(CHAIN_START_HASH),
            tail_signed: true,
        };
        let Some(tail) = tail else {
            return Ok((chain, None));
        };
        let value: Value = serde_json::from_slice(tail)
            .map_err(|error| ChainError::UnreadableTail(error.to_string()))?;
        let computed = entry_hash(&value)?;
        let entry: LedgerEntry = serde_json::from_value(value)
            .map_err(|error| ChainError::UnreadableTail(error.to_string()))?;
        if entry.hash != computed {
            return Err(ChainError::TamperedTail {
                stored: entry.hash,
                computed,
            });
        }
        if entry.partition != partition {
            return Err(ChainError::WrongPartition {
                found: entry.partition,
                expected: partition,
            });
        }
        let mut detail = Map::new();
        if entry.instance == instance {
            chain.next_sequence = entry.sequence + 1;
            chain.previous_hash = entry.hash.clone();
            chain.tail_signed = entry.kind == Kind::Checkpoint;
            if continuation == Continuation::Start {
                return Ok((chain, None));
            }
            detail.insert(String::from("sequence"), Value::from(entry.sequence));
            detail.insert(String::from("hash"), Value::from(entry.hash));
            let resumed = EntryContent {
                event_detail: Some(detail),
                ..EntryContent::event(Event::ChainResumed, NIGHTFALL_PRINCIPAL)
            };
            return Ok((chain, Some(resumed)));
        }
        detail.insert(String::from("instance"), Value::from(entry.instance));
        detail.insert(String::from("sequence"), Value::from(entry.sequence));
        detail.insert(String::from("hash"), Value::from(entry.hash));
        let link = EntryContent {
            event_detail: Some(detail),
            ..EntryContent::event(Event::ChainLink, NIGHTFALL_PRINCIPAL)
        };
        Ok((chain, Some(link)))
    }

    pub fn append(
        &mut self,
        id: String,
        time: String,
        content: EntryContent,
    ) -> Result<LedgerEntry, CanonicalError> {
        let mut entry = LedgerEntry::from_content(
            content,
            ChainFields {
                id,
                time,
                instance: &self.instance,
                partition: self.partition,
                sequence: self.next_sequence,
                previous_hash: self.previous_hash.clone(),
            },
        );
        entry.seal()?;
        self.advance(&entry);
        Ok(entry)
    }

    pub fn checkpoint(
        &mut self,
        signer: &CheckpointSigner,
        id: String,
        time: String,
    ) -> Result<Option<LedgerEntry>, CanonicalError> {
        if self.tail_signed {
            return Ok(None);
        }
        let message = checkpoint_message(
            &self.instance,
            self.partition,
            self.next_sequence - 1,
            &self.previous_hash,
        );
        let mut entry = LedgerEntry::from_content(
            EntryContent::blank(Kind::Checkpoint, NIGHTFALL_PRINCIPAL),
            ChainFields {
                id,
                time,
                instance: &self.instance,
                partition: self.partition,
                sequence: self.next_sequence,
                previous_hash: self.previous_hash.clone(),
            },
        );
        entry.key_id = Some(String::from(signer.key_id()));
        entry.signature = Some(signer.sign(&message));
        entry.seal()?;
        self.advance(&entry);
        Ok(Some(entry))
    }

    pub fn position(&self) -> Option<Position> {
        self.next_sequence.checked_sub(1).map(|sequence| Position {
            sequence,
            hash: self.previous_hash.clone(),
        })
    }

    pub fn tail_signed(&self) -> bool {
        self.tail_signed
    }

    fn advance(&mut self, entry: &LedgerEntry) {
        self.next_sequence = entry.sequence + 1;
        self.previous_hash = entry.hash.clone();
        self.tail_signed = entry.kind == Kind::Checkpoint;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::fixtures::call;
    use crate::signing::fixtures::signer;

    const TIME: &str = "2026-10-06T09:15:40.512408217Z";

    fn identifier(number: u32) -> String {
        format!("01a112fe-d1ba-7684-ae83-{number:012x}")
    }

    #[test]
    fn starts_an_empty_partition_at_sequence_zero_without_a_link() {
        let (mut chain, link) = Chain::start("nightfall-0", 0, None, Continuation::Start).unwrap();
        assert!(link.is_none());
        let first = chain
            .append(identifier(1), String::from(TIME), call())
            .unwrap();
        assert_eq!(first.sequence, 0);
        assert_eq!(first.previous_hash, CHAIN_START_HASH);
        let second = chain
            .append(identifier(2), String::from(TIME), call())
            .unwrap();
        assert_eq!(second.sequence, 1);
        assert_eq!(second.previous_hash, first.hash);
        assert_eq!(
            chain.position(),
            Some(Position {
                sequence: 1,
                hash: second.hash
            })
        );
    }

    #[test]
    fn continues_its_own_tail_and_links_a_foreign_one() {
        let (mut previous, _) = Chain::start("nightfall-0", 2, None, Continuation::Start).unwrap();
        let tail = previous
            .append(identifier(1), String::from(TIME), call())
            .unwrap();
        let payload = tail.payload().unwrap();

        let (mut same, link) =
            Chain::start("nightfall-0", 2, Some(&payload), Continuation::Start).unwrap();
        assert!(link.is_none());
        let next = same
            .append(identifier(2), String::from(TIME), call())
            .unwrap();
        assert_eq!(
            (next.sequence, next.previous_hash.as_str()),
            (1, tail.hash.as_str())
        );

        let (mut other, link) =
            Chain::start("nightfall-2", 2, Some(&payload), Continuation::Start).unwrap();
        let link = other
            .append(identifier(3), String::from(TIME), link.unwrap())
            .unwrap();
        assert_eq!(link.event, Some(Event::ChainLink));
        assert_eq!(link.sequence, 0);
        let detail = link.event_detail.unwrap();
        assert_eq!(detail["instance"], "nightfall-0");
        assert_eq!(detail["sequence"], 0);
        assert_eq!(detail["hash"], tail.hash.as_str());

        let (mut resumed, event) =
            Chain::start("nightfall-0", 2, Some(&payload), Continuation::Resume).unwrap();
        let event = resumed
            .append(identifier(4), String::from(TIME), event.unwrap())
            .unwrap();
        assert_eq!(event.event, Some(Event::ChainResumed));
        assert_eq!(
            (event.sequence, event.previous_hash.as_str()),
            (1, tail.hash.as_str())
        );
    }

    #[test]
    fn refuses_a_tampered_or_misplaced_tail() {
        let (mut chain, _) = Chain::start("nightfall-0", 0, None, Continuation::Start).unwrap();
        let mut tail = chain
            .append(identifier(1), String::from(TIME), call())
            .unwrap();
        assert!(matches!(
            Chain::start(
                "nightfall-0",
                1,
                Some(&tail.payload().unwrap()),
                Continuation::Start
            ),
            Err(ChainError::WrongPartition {
                found: 0,
                expected: 1
            })
        ));
        tail.principal = String::from("mallory");
        assert!(matches!(
            Chain::start(
                "nightfall-0",
                0,
                Some(&tail.payload().unwrap()),
                Continuation::Start
            ),
            Err(ChainError::TamperedTail { .. })
        ));
        assert!(matches!(
            Chain::start("nightfall-0", 0, Some(b"not json"), Continuation::Start),
            Err(ChainError::UnreadableTail(_))
        ));
    }

    #[test]
    fn signs_the_preceding_entry_once() {
        let signer = signer(3);
        let (mut chain, _) = Chain::start("nightfall-0", 0, None, Continuation::Start).unwrap();
        assert!(
            chain
                .checkpoint(&signer, identifier(1), String::from(TIME))
                .unwrap()
                .is_none()
        );
        let signed = chain
            .append(identifier(2), String::from(TIME), call())
            .unwrap();
        let checkpoint = chain
            .checkpoint(&signer, identifier(3), String::from(TIME))
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.kind, Kind::Checkpoint);
        assert_eq!(checkpoint.principal, NIGHTFALL_PRINCIPAL);
        assert_eq!(checkpoint.previous_hash, signed.hash);
        assert_eq!(checkpoint.key_id.as_deref(), Some(signer.key_id()));
        let keys =
            crate::signing::VerifyingKeys::from_jwks(&signer.public_jwks().to_string()).unwrap();
        keys.verify(
            signer.key_id(),
            &checkpoint_message("nightfall-0", 0, signed.sequence, &signed.hash),
            checkpoint.signature.as_deref().unwrap(),
        )
        .unwrap();
        assert!(
            chain
                .checkpoint(&signer, identifier(4), String::from(TIME))
                .unwrap()
                .is_none()
        );
    }
}
