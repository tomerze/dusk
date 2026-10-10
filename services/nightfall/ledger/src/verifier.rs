use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::io::BufRead;

use serde_json::Value;

use crate::entry::{CHAIN_START_HASH, LedgerEntry, entry_hash};
use crate::signing::{SignatureProblem, VerifyingKeys, checkpoint_message};

pub const BREAKS_KEPT: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyOptions {
    pub duplicate_window: usize,
}

impl Default for VerifyOptions {
    fn default() -> VerifyOptions {
        VerifyOptions {
            duplicate_window: 65_536,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakKind {
    Unparseable,
    HashMismatch,
    PreviousHashMismatch,
    SequenceGap,
    OutOfOrder,
    ConflictingEntry,
    StaleEntry,
    InvalidChainStart,
    UnlinkedChainStart,
    ChainLinkMismatch,
    ChainResumedMismatch,
    UnknownKey,
    ForgedCheckpoint,
}

impl BreakKind {
    pub fn name(self) -> &'static str {
        match self {
            BreakKind::Unparseable => "unparseable",
            BreakKind::HashMismatch => "hash_mismatch",
            BreakKind::PreviousHashMismatch => "previous_hash_mismatch",
            BreakKind::SequenceGap => "sequence_gap",
            BreakKind::OutOfOrder => "out_of_order",
            BreakKind::ConflictingEntry => "conflicting_entry",
            BreakKind::StaleEntry => "stale_entry",
            BreakKind::InvalidChainStart => "invalid_chain_start",
            BreakKind::UnlinkedChainStart => "unlinked_chain_start",
            BreakKind::ChainLinkMismatch => "chain_link_mismatch",
            BreakKind::ChainResumedMismatch => "chain_resumed_mismatch",
            BreakKind::UnknownKey => "unknown_key",
            BreakKind::ForgedCheckpoint => "forged_checkpoint",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Break {
    pub record: u64,
    pub kind: BreakKind,
    pub instance: Option<String>,
    pub partition: Option<u32>,
    pub sequence: Option<u64>,
    pub detail: String,
}

impl fmt::Display for Break {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "record {}: {}", self.record, self.kind.name())?;
        if let Some(instance) = &self.instance {
            write!(formatter, " instance {instance}")?;
        }
        if let Some(partition) = self.partition {
            write!(formatter, " partition {partition}")?;
        }
        if let Some(sequence) = self.sequence {
            write!(formatter, " sequence {sequence}")?;
        }
        write!(formatter, ": {}", self.detail)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSummary {
    pub instance: String,
    pub partition: u32,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub last_hash: String,
    pub entries: u64,
    pub checkpoints: u64,
    pub last_checkpoint_sequence: Option<u64>,
    pub unsigned_tail: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerificationReport {
    pub records: u64,
    pub entries: u64,
    pub duplicates: u64,
    pub checkpoints: u64,
    pub break_count: u64,
    pub breaks: Vec<Break>,
    pub chains: Vec<ChainSummary>,
}

impl VerificationReport {
    pub fn is_clean(&self) -> bool {
        self.break_count == 0
    }
}

impl fmt::Display for VerificationReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_clean() {
            writeln!(
                formatter,
                "ledger verified: {} entries in {} chains, {} checkpoints, {} duplicates",
                self.entries,
                self.chains.len(),
                self.checkpoints,
                self.duplicates
            )?;
        } else {
            writeln!(
                formatter,
                "ledger verification failed: {} breaks in {} records",
                self.break_count, self.records
            )?;
            if let Some(first) = self.breaks.first() {
                writeln!(formatter, "first break: {first}")?;
            }
            for found in self.breaks.iter().skip(1) {
                writeln!(formatter, "break: {found}")?;
            }
            if self.break_count > self.breaks.len() as u64 {
                writeln!(
                    formatter,
                    "{} further breaks not listed",
                    self.break_count - self.breaks.len() as u64
                )?;
            }
        }
        for chain in &self.chains {
            writeln!(
                formatter,
                "chain {} partition {}: sequences {}..={}, {} entries, last checkpoint {}, {} entries after it",
                chain.instance,
                chain.partition,
                chain.first_sequence,
                chain.last_sequence,
                chain.entries,
                chain
                    .last_checkpoint_sequence
                    .map(|sequence| sequence.to_string())
                    .unwrap_or_else(|| String::from("none")),
                chain.unsigned_tail
            )?;
        }
        Ok(())
    }
}

struct ChainState {
    summary: ChainSummary,
    recent: VecDeque<(u64, String)>,
}

struct PartitionTail {
    instance: String,
    sequence: u64,
    hash: String,
}

pub struct Verifier {
    keys: VerifyingKeys,
    options: VerifyOptions,
    chains: HashMap<(String, u32), ChainState>,
    finished: Vec<ChainSummary>,
    partitions: HashMap<u32, PartitionTail>,
    report: VerificationReport,
}

impl Verifier {
    pub fn new(keys: VerifyingKeys, options: VerifyOptions) -> Verifier {
        Verifier {
            keys,
            options,
            chains: HashMap::new(),
            finished: Vec::new(),
            partitions: HashMap::new(),
            report: VerificationReport::default(),
        }
    }

    pub fn push_record(&mut self, record: &[u8]) {
        self.report.records += 1;
        let position = self.report.records;
        let value: Value = match serde_json::from_slice(record) {
            Ok(value) => value,
            Err(error) => {
                return self.add_break(
                    position,
                    BreakKind::Unparseable,
                    None,
                    None,
                    None,
                    format!("not JSON: {error}"),
                );
            }
        };
        let computed = match entry_hash(&value) {
            Ok(computed) => computed,
            Err(error) => {
                return self.add_break(
                    position,
                    BreakKind::Unparseable,
                    None,
                    None,
                    None,
                    error.to_string(),
                );
            }
        };
        let entry: LedgerEntry = match serde_json::from_value(value) {
            Ok(entry) => entry,
            Err(error) => {
                return self.add_break(
                    position,
                    BreakKind::Unparseable,
                    None,
                    None,
                    None,
                    format!("not a ledger entry: {error}"),
                );
            }
        };
        self.push_entry(position, entry, computed);
    }

    fn push_entry(&mut self, position: u64, entry: LedgerEntry, computed: String) {
        let instance = Some(entry.instance.clone());
        let partition = Some(entry.partition);
        let sequence = Some(entry.sequence);
        if computed != entry.hash {
            self.add_break(
                position,
                BreakKind::HashMismatch,
                instance.clone(),
                partition,
                sequence,
                format!(
                    "stored hash {} but the content hashes to {computed}",
                    entry.hash
                ),
            );
        }
        let key = (entry.instance.clone(), entry.partition);
        if let Some(state) = self.chains.get(&key)
            && state.recent.iter().any(|(seen_sequence, seen_hash)| {
                *seen_sequence == entry.sequence && *seen_hash == entry.hash
            })
        {
            self.report.duplicates += 1;
            return;
        }
        if entry.sequence == 0 {
            self.start_chain(position, &entry);
        } else if let Some(state) = self.chains.get(&key) {
            let last = state.summary.last_sequence;
            if entry.sequence <= last {
                let seen = state
                    .recent
                    .iter()
                    .find(|(seen_sequence, _)| *seen_sequence == entry.sequence);
                let oldest = state
                    .recent
                    .front()
                    .map(|(oldest, _)| *oldest)
                    .unwrap_or(last);
                match seen {
                    Some((_, hash)) => {
                        let detail = format!(
                            "sequence already holds an entry with hash {hash}, this one has {}",
                            entry.hash
                        );
                        return self.add_break(
                            position,
                            BreakKind::ConflictingEntry,
                            instance,
                            partition,
                            sequence,
                            detail,
                        );
                    }
                    None if entry.sequence < oldest => {
                        let detail = format!(
                            "older than the {} entries kept for duplicate detection",
                            self.options.duplicate_window
                        );
                        return self.add_break(
                            position,
                            BreakKind::StaleEntry,
                            instance,
                            partition,
                            sequence,
                            detail,
                        );
                    }
                    None => {
                        let detail = format!("arrived after sequence {last}");
                        return self.add_break(
                            position,
                            BreakKind::OutOfOrder,
                            instance,
                            partition,
                            sequence,
                            detail,
                        );
                    }
                }
            }
            if entry.sequence > last + 1 {
                let detail = format!(
                    "sequences {} to {} are missing",
                    last + 1,
                    entry.sequence - 1
                );
                self.add_break(
                    position,
                    BreakKind::SequenceGap,
                    instance.clone(),
                    partition,
                    sequence,
                    detail,
                );
            } else if entry.previous_hash != state.summary.last_hash {
                let detail = format!(
                    "previous_hash {} but the preceding entry has hash {}",
                    entry.previous_hash, state.summary.last_hash
                );
                self.add_break(
                    position,
                    BreakKind::PreviousHashMismatch,
                    instance.clone(),
                    partition,
                    sequence,
                    detail,
                );
            }
            if entry.event == Some(crate::entry::Event::ChainResumed) {
                self.check_resumed(position, &entry);
            }
        } else {
            if let Some(tail) = self.partitions.get(&entry.partition)
                && tail.instance != entry.instance
            {
                let detail = format!(
                    "the first entry of this chain follows instance {} sequence {} on the partition without a chain_link",
                    tail.instance, tail.sequence
                );
                self.add_break(
                    position,
                    BreakKind::UnlinkedChainStart,
                    instance.clone(),
                    partition,
                    sequence,
                    detail,
                );
            }
            self.chains.insert(key.clone(), new_state(&entry));
        }
        self.check_checkpoint(position, &entry);
        self.advance(&key, &entry);
    }

    fn start_chain(&mut self, position: u64, entry: &LedgerEntry) {
        let instance = Some(entry.instance.clone());
        let partition = Some(entry.partition);
        if entry.previous_hash != CHAIN_START_HASH {
            self.add_break(
                position,
                BreakKind::InvalidChainStart,
                instance.clone(),
                partition,
                Some(0),
                format!(
                    "a chain start carries previous_hash {} instead of 64 zeros",
                    entry.previous_hash
                ),
            );
        }
        let previous_tail = self.partitions.get(&entry.partition);
        let link = (entry.event == Some(crate::entry::Event::ChainLink))
            .then_some(entry.event_detail.as_ref())
            .flatten();
        match (previous_tail, link) {
            (None, _) => {}
            (Some(tail), None) => {
                let detail = format!(
                    "a chain starts after instance {} sequence {} on this partition without a chain_link",
                    tail.instance, tail.sequence
                );
                self.add_break(
                    position,
                    BreakKind::UnlinkedChainStart,
                    instance.clone(),
                    partition,
                    Some(0),
                    detail,
                );
            }
            (Some(tail), Some(detail)) => {
                let named = (
                    detail.get("instance").and_then(Value::as_str),
                    detail.get("sequence").and_then(Value::as_u64),
                    detail.get("hash").and_then(Value::as_str),
                );
                if named
                    != (
                        Some(tail.instance.as_str()),
                        Some(tail.sequence),
                        Some(tail.hash.as_str()),
                    )
                {
                    let detail = format!(
                        "chain_link names {:?} but the partition's previous entry is instance {} sequence {} hash {}",
                        named, tail.instance, tail.sequence, tail.hash
                    );
                    self.add_break(
                        position,
                        BreakKind::ChainLinkMismatch,
                        instance.clone(),
                        partition,
                        Some(0),
                        detail,
                    );
                }
            }
        }
        let key = (entry.instance.clone(), entry.partition);
        if let Some(replaced) = self.chains.insert(key, new_state(entry)) {
            self.finished.push(replaced.summary);
        }
    }

    fn check_resumed(&mut self, position: u64, entry: &LedgerEntry) {
        let named = entry.event_detail.as_ref().map(|detail| {
            (
                detail.get("sequence").and_then(Value::as_u64),
                detail.get("hash").and_then(Value::as_str),
            )
        });
        let expected = (Some(entry.sequence - 1), Some(entry.previous_hash.as_str()));
        if named != Some(expected) {
            self.add_break(
                position,
                BreakKind::ChainResumedMismatch,
                Some(entry.instance.clone()),
                Some(entry.partition),
                Some(entry.sequence),
                format!(
                    "chain_resumed names {named:?} but follows sequence {} hash {}",
                    entry.sequence - 1,
                    entry.previous_hash
                ),
            );
        }
    }

    fn check_checkpoint(&mut self, position: u64, entry: &LedgerEntry) {
        if entry.kind != crate::entry::Kind::Checkpoint {
            return;
        }
        self.report.checkpoints += 1;
        let instance = Some(entry.instance.clone());
        let partition = Some(entry.partition);
        let sequence = Some(entry.sequence);
        let (Some(key_id), Some(signature)) = (&entry.key_id, &entry.signature) else {
            return self.add_break(
                position,
                BreakKind::ForgedCheckpoint,
                instance,
                partition,
                sequence,
                String::from("a checkpoint without key_id or signature"),
            );
        };
        let Some(signed_sequence) = entry.sequence.checked_sub(1) else {
            return self.add_break(
                position,
                BreakKind::ForgedCheckpoint,
                instance,
                partition,
                sequence,
                String::from("a checkpoint cannot start a chain"),
            );
        };
        let message = checkpoint_message(
            &entry.instance,
            entry.partition,
            signed_sequence,
            &entry.previous_hash,
        );
        match self.keys.verify(key_id, &message, signature) {
            Ok(()) => {}
            Err(SignatureProblem::UnknownKey(_)) => self.add_break(
                position,
                BreakKind::UnknownKey,
                instance,
                partition,
                sequence,
                format!("no configured public key has key id {key_id}"),
            ),
            Err(problem) => self.add_break(
                position,
                BreakKind::ForgedCheckpoint,
                instance,
                partition,
                sequence,
                problem.to_string(),
            ),
        }
    }

    fn advance(&mut self, key: &(String, u32), entry: &LedgerEntry) {
        self.report.entries += 1;
        if let Some(state) = self.chains.get_mut(key) {
            let summary = &mut state.summary;
            summary.last_sequence = entry.sequence;
            summary.last_hash = entry.hash.clone();
            summary.entries += 1;
            if entry.kind == crate::entry::Kind::Checkpoint {
                summary.checkpoints += 1;
                summary.last_checkpoint_sequence = Some(entry.sequence);
                summary.unsigned_tail = 0;
            } else {
                summary.unsigned_tail += 1;
            }
            state.recent.push_back((entry.sequence, entry.hash.clone()));
            while state.recent.len() > self.options.duplicate_window {
                state.recent.pop_front();
            }
        }
        self.partitions.insert(
            entry.partition,
            PartitionTail {
                instance: entry.instance.clone(),
                sequence: entry.sequence,
                hash: entry.hash.clone(),
            },
        );
    }

    fn add_break(
        &mut self,
        record: u64,
        kind: BreakKind,
        instance: Option<String>,
        partition: Option<u32>,
        sequence: Option<u64>,
        detail: String,
    ) {
        self.report.break_count += 1;
        if self.report.breaks.len() < BREAKS_KEPT {
            self.report.breaks.push(Break {
                record,
                kind,
                instance,
                partition,
                sequence,
                detail,
            });
        }
    }

    pub fn finish(mut self) -> VerificationReport {
        let mut chains: BTreeMap<(u32, String, u64), ChainSummary> = BTreeMap::new();
        for summary in self
            .finished
            .into_iter()
            .chain(self.chains.into_values().map(|state| state.summary))
        {
            chains.insert(
                (
                    summary.partition,
                    summary.instance.clone(),
                    summary.first_sequence,
                ),
                summary,
            );
        }
        self.report.chains = chains.into_values().collect();
        self.report
    }
}

fn new_state(entry: &LedgerEntry) -> ChainState {
    ChainState {
        summary: ChainSummary {
            instance: entry.instance.clone(),
            partition: entry.partition,
            first_sequence: entry.sequence,
            last_sequence: entry.sequence,
            last_hash: String::new(),
            entries: 0,
            checkpoints: 0,
            last_checkpoint_sequence: None,
            unsigned_tail: 0,
        },
        recent: VecDeque::new(),
    }
}

pub fn verify_lines(
    input: impl BufRead,
    keys: VerifyingKeys,
    options: VerifyOptions,
) -> std::io::Result<VerificationReport> {
    let mut verifier = Verifier::new(keys, options);
    for line in input.split(b'\n') {
        let line = line?;
        let trimmed = line.strip_suffix(b"\r").unwrap_or(&line);
        if trimmed.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        verifier.push_record(trimmed);
    }
    Ok(verifier.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{Chain, Continuation};
    use crate::entry::fixtures::{call, result, session_open};
    use crate::signing::fixtures::signer;

    struct Fixture {
        records: Vec<Vec<u8>>,
        entries: Vec<LedgerEntry>,
        keys: VerifyingKeys,
    }

    fn time(number: usize) -> String {
        format!("2026-10-06T09:15:{:02}.000000000Z", number % 60)
    }

    fn identifier(number: usize) -> String {
        format!("01a112fe-d1ba-7684-ae83-{number:012x}")
    }

    fn fixture() -> Fixture {
        let signer = signer(9);
        let keys = VerifyingKeys::from_jwks(&signer.public_jwks().to_string()).unwrap();
        let (mut chain, _) = Chain::start("nightfall-0", 0, None, Continuation::Start).unwrap();
        let mut entries = Vec::new();
        for number in 0..10 {
            let content = match number % 3 {
                0 => session_open(),
                1 => call(),
                _ => result(),
            };
            entries.push(
                chain
                    .append(identifier(number), time(number), content)
                    .unwrap(),
            );
            if number % 4 == 3 {
                entries.push(
                    chain
                        .checkpoint(&signer, identifier(100 + number), time(number))
                        .unwrap()
                        .unwrap(),
                );
            }
        }
        let records = entries
            .iter()
            .map(|entry| entry.payload().unwrap())
            .collect();
        Fixture {
            records,
            entries,
            keys,
        }
    }

    fn verify(records: &[Vec<u8>], keys: &VerifyingKeys) -> VerificationReport {
        let mut verifier = Verifier::new(keys.clone(), VerifyOptions::default());
        for record in records {
            verifier.push_record(record);
        }
        verifier.finish()
    }

    fn kinds(report: &VerificationReport) -> Vec<BreakKind> {
        report.breaks.iter().map(|found| found.kind).collect()
    }

    #[test]
    fn accepts_an_intact_chain_with_checkpoints() {
        let fixture = fixture();
        let report = verify(&fixture.records, &fixture.keys);
        assert!(report.is_clean(), "{report}");
        assert_eq!(report.entries, fixture.entries.len() as u64);
        assert_eq!(report.checkpoints, 2);
        assert_eq!(report.chains.len(), 1);
        let chain = &report.chains[0];
        assert_eq!(chain.first_sequence, 0);
        assert_eq!(chain.last_sequence, fixture.entries.len() as u64 - 1);
        assert_eq!(chain.last_checkpoint_sequence, Some(9));
        assert_eq!(chain.unsigned_tail, 2);
    }

    #[test]
    fn detects_an_edited_entry() {
        let fixture = fixture();
        let mut records = fixture.records.clone();
        let mut edited: Value = serde_json::from_slice(&records[4]).unwrap();
        edited["principal"] = Value::from("mallory");
        records[4] = serde_json::to_vec(&edited).unwrap();
        let report = verify(&records, &fixture.keys);
        assert_eq!(kinds(&report), vec![BreakKind::HashMismatch]);
        assert_eq!(report.breaks[0].sequence, Some(4));
        assert_eq!(report.breaks[0].record, 5);
    }

    #[test]
    fn detects_an_edit_that_recomputes_the_hash() {
        let fixture = fixture();
        let mut records = fixture.records.clone();
        let mut edited = fixture.entries[3].clone();
        edited.principal = String::from("mallory");
        edited.seal().unwrap();
        records[3] = edited.payload().unwrap();
        let report = verify(&records, &fixture.keys);
        assert_eq!(kinds(&report), vec![BreakKind::PreviousHashMismatch]);
        assert_eq!(report.breaks[0].sequence, Some(4));
    }

    #[test]
    fn detects_a_rewritten_chain_by_its_checkpoints() {
        let fixture = fixture();
        let mut entries = fixture.entries.clone();
        entries[3].principal = String::from("mallory");
        let mut previous_hash = entries[2].hash.clone();
        for entry in entries.iter_mut().skip(3) {
            entry.previous_hash = previous_hash;
            entry.seal().unwrap();
            previous_hash = entry.hash.clone();
        }
        let records: Vec<Vec<u8>> = entries
            .iter()
            .map(|entry| entry.payload().unwrap())
            .collect();
        let report = verify(&records, &fixture.keys);
        assert_eq!(
            kinds(&report),
            vec![BreakKind::ForgedCheckpoint, BreakKind::ForgedCheckpoint]
        );
        assert_eq!(report.breaks[0].sequence, Some(4));
        assert_eq!(report.breaks[1].sequence, Some(9));
    }

    #[test]
    fn detects_a_deleted_entry() {
        let fixture = fixture();
        let mut records = fixture.records.clone();
        records.remove(6);
        let report = verify(&records, &fixture.keys);
        assert_eq!(kinds(&report), vec![BreakKind::SequenceGap]);
        assert_eq!(report.breaks[0].sequence, Some(7));
    }

    #[test]
    fn detects_reordered_entries() {
        let fixture = fixture();
        let mut records = fixture.records.clone();
        records.swap(2, 3);
        let report = verify(&records, &fixture.keys);
        assert_eq!(
            kinds(&report),
            vec![BreakKind::SequenceGap, BreakKind::OutOfOrder]
        );
    }

    #[test]
    fn detects_a_forged_and_an_unknown_checkpoint() {
        let fixture = fixture();
        let mut forged = fixture.entries[4].clone();
        assert_eq!(forged.kind, crate::entry::Kind::Checkpoint);
        forged.signature = Some(signer(9).sign(b"something else"));
        forged.seal().unwrap();
        let mut records = fixture.records[..4].to_vec();
        records.push(forged.payload().unwrap());
        let report = verify(&records, &fixture.keys);
        assert_eq!(kinds(&report), vec![BreakKind::ForgedCheckpoint]);

        let report = verify(&fixture.records, &VerifyingKeys::default());
        assert_eq!(
            kinds(&report),
            vec![BreakKind::UnknownKey, BreakKind::UnknownKey]
        );
    }

    #[test]
    fn accepts_duplicates_but_not_conflicts() {
        let fixture = fixture();
        let mut records = fixture.records.clone();
        records.insert(5, fixture.records[3].clone());
        records.push(fixture.records[0].clone());
        let report = verify(&records, &fixture.keys);
        assert!(report.is_clean(), "{report}");
        assert_eq!(report.duplicates, 2);

        let mut conflicting = fixture.entries[2].clone();
        conflicting.principal = String::from("dawn-1");
        conflicting.seal().unwrap();
        let mut records = fixture.records.clone();
        records.push(conflicting.payload().unwrap());
        let report = verify(&records, &fixture.keys);
        assert_eq!(kinds(&report), vec![BreakKind::ConflictingEntry]);
    }

    #[test]
    fn requires_a_chain_link_after_another_chain() {
        let fixture = fixture();
        let tail = fixture.records.last().unwrap();
        let (mut linked, link) =
            Chain::start("nightfall-0b", 0, Some(tail), Continuation::Start).unwrap();
        let mut records = fixture.records.clone();
        records.push(
            linked
                .append(identifier(200), time(0), link.unwrap())
                .unwrap()
                .payload()
                .unwrap(),
        );
        records.push(
            linked
                .append(identifier(201), time(1), call())
                .unwrap()
                .payload()
                .unwrap(),
        );
        let report = verify(&records, &fixture.keys);
        assert!(report.is_clean(), "{report}");
        assert_eq!(report.chains.len(), 2);

        let (mut unlinked, _) = Chain::start("nightfall-0c", 0, None, Continuation::Start).unwrap();
        let mut records = fixture.records.clone();
        records.push(
            unlinked
                .append(identifier(300), time(0), call())
                .unwrap()
                .payload()
                .unwrap(),
        );
        let report = verify(&records, &fixture.keys);
        assert_eq!(kinds(&report), vec![BreakKind::UnlinkedChainStart]);
    }

    #[test]
    fn checks_what_a_chain_link_names() {
        let fixture = fixture();
        let wrong_tail = fixture.records[3].clone();
        let (mut linked, link) =
            Chain::start("nightfall-0b", 0, Some(&wrong_tail), Continuation::Start).unwrap();
        let mut records = fixture.records.clone();
        records.push(
            linked
                .append(identifier(200), time(0), link.unwrap())
                .unwrap()
                .payload()
                .unwrap(),
        );
        let report = verify(&records, &fixture.keys);
        assert_eq!(kinds(&report), vec![BreakKind::ChainLinkMismatch]);
    }

    #[test]
    fn reports_unparseable_records_and_reads_lines() {
        let fixture = fixture();
        let mut input = Vec::new();
        for record in &fixture.records {
            input.extend_from_slice(record);
            input.extend_from_slice(b"\r\n");
        }
        input.extend_from_slice(b"\n{\"schema\":1}\n");
        let report = verify_lines(
            input.as_slice(),
            fixture.keys.clone(),
            VerifyOptions::default(),
        )
        .unwrap();
        assert_eq!(kinds(&report), vec![BreakKind::Unparseable]);
        assert_eq!(report.records, fixture.records.len() as u64 + 1);
        assert!(
            report
                .to_string()
                .starts_with("ledger verification failed: 1 breaks")
        );
    }
}
