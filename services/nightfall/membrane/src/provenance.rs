use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenanceEntry {
    pub cap_id: u64,
    pub parent_cap_id: Option<u64>,
    pub call_id: Option<String>,
    pub action: String,
    pub interface_id: u64,
    pub principal: String,
    pub pid: u64,
    pub live: bool,
}

struct Node {
    entry: ProvenanceEntry,
    children: usize,
}

pub(crate) struct Provenance {
    nodes: HashMap<u64, Node>,
    bound: usize,
}

impl Default for Provenance {
    fn default() -> Provenance {
        Provenance::bounded(u64::MAX)
    }
}

impl Provenance {
    pub(crate) fn bounded(bound: u64) -> Provenance {
        Provenance {
            nodes: HashMap::new(),
            bound: usize::try_from(bound).unwrap_or(usize::MAX).max(1),
        }
    }

    fn collapse(&mut self) {
        let released: Vec<u64> = self
            .nodes
            .iter()
            .filter(|(_, node)| !node.entry.live)
            .map(|(cap_id, _)| *cap_id)
            .collect();
        if released.is_empty() {
            return;
        }
        let mut parents: HashMap<u64, Option<u64>> = HashMap::new();
        for cap_id in &released {
            parents.insert(
                *cap_id,
                self.nodes
                    .get(cap_id)
                    .and_then(|node| node.entry.parent_cap_id),
            );
        }
        for node in self.nodes.values_mut() {
            let mut parent = node.entry.parent_cap_id;
            while let Some(released_parent) = parent.and_then(|cap_id| parents.get(&cap_id)) {
                parent = *released_parent;
            }
            node.entry.parent_cap_id = parent;
            node.children = 0;
        }
        for cap_id in &released {
            self.nodes.remove(cap_id);
        }
        let parents: Vec<u64> = self
            .nodes
            .values()
            .filter_map(|node| node.entry.parent_cap_id)
            .collect();
        for parent in parents {
            if let Some(node) = self.nodes.get_mut(&parent) {
                node.children += 1;
            }
        }
        tracing::debug!(
            collapsed = released.len(),
            live = self.nodes.len(),
            "released capabilities were folded out of the provenance tree"
        );
    }

    pub(crate) fn add(&mut self, entry: ProvenanceEntry) {
        if self.nodes.len() >= self.bound {
            self.collapse();
        }
        if let Some(parent) = entry
            .parent_cap_id
            .and_then(|parent| self.nodes.get_mut(&parent))
        {
            parent.children += 1;
        }
        self.nodes.insert(entry.cap_id, Node { entry, children: 0 });
    }

    pub(crate) fn release(&mut self, cap_id: u64) {
        if let Some(node) = self.nodes.get_mut(&cap_id) {
            node.entry.live = false;
        }
        let mut current = Some(cap_id);
        while let Some(cap_id) = current {
            let removable = self
                .nodes
                .get(&cap_id)
                .is_some_and(|node| !node.entry.live && node.children == 0);
            if !removable {
                return;
            }
            let removed = self.nodes.remove(&cap_id);
            current = removed.and_then(|node| node.entry.parent_cap_id);
            if let Some(parent) = current.and_then(|parent| self.nodes.get_mut(&parent)) {
                parent.children = parent.children.saturating_sub(1);
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<ProvenanceEntry> {
        let mut entries: Vec<ProvenanceEntry> =
            self.nodes.values().map(|node| node.entry.clone()).collect();
        entries.sort_by_key(|entry| entry.cap_id);
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(cap_id: u64, parent_cap_id: Option<u64>) -> ProvenanceEntry {
        ProvenanceEntry {
            cap_id,
            parent_cap_id,
            call_id: None,
            action: "Dusk.process".to_string(),
            interface_id: 0,
            principal: "dawn-0".to_string(),
            pid: 0,
            live: true,
        }
    }

    #[test]
    fn keeps_a_dead_entry_while_a_live_descendant_needs_it() {
        let mut provenance = Provenance::default();
        provenance.add(entry(0, None));
        provenance.add(entry(1, Some(0)));
        provenance.add(entry(2, Some(1)));
        provenance.add(entry(3, Some(1)));
        provenance.release(1);
        assert_eq!(provenance.snapshot().len(), 4);
        assert!(!provenance.snapshot()[1].live);
        provenance.release(2);
        assert_eq!(provenance.snapshot().len(), 3);
        provenance.release(3);
        assert_eq!(provenance.snapshot().len(), 1);
        assert_eq!(provenance.snapshot()[0].cap_id, 0);
        provenance.release(0);
        assert_eq!(provenance.snapshot().len(), 0);
    }

    #[test]
    fn folds_released_ancestors_into_the_nearest_live_one_past_its_bound() {
        let mut provenance = Provenance::bounded(3);
        provenance.add(entry(0, None));
        provenance.add(entry(1, Some(0)));
        provenance.add(entry(2, Some(1)));
        provenance.release(1);
        assert_eq!(provenance.snapshot().len(), 3);
        provenance.add(entry(3, Some(2)));
        assert_eq!(provenance.snapshot().len(), 3);
        let snapshot = provenance.snapshot();
        assert_eq!(
            snapshot
                .iter()
                .map(|entry| (entry.cap_id, entry.parent_cap_id))
                .collect::<Vec<_>>(),
            vec![(0, None), (2, Some(0)), (3, Some(2))]
        );
        provenance.release(3);
        provenance.release(2);
        assert_eq!(provenance.snapshot().len(), 1);
        for cap_id in 10..20 {
            provenance.add(entry(cap_id, Some(cap_id - 1)));
            provenance.release(cap_id - 1);
        }
        assert!(
            provenance.snapshot().len() <= 3,
            "{}",
            provenance.snapshot().len()
        );
    }

    #[test]
    fn ignores_unknown_releases() {
        let mut provenance = Provenance::default();
        provenance.release(7);
        provenance.add(entry(1, Some(99)));
        provenance.release(1);
        assert_eq!(provenance.snapshot().len(), 0);
    }
}
