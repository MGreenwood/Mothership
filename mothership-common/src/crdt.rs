use std::cmp::Ordering;
use uuid::Uuid;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiftCRDT {
    site_id: Uuid,
    lamport_clock: u64,
    operations: Vec<Operation>,
    tombstones: HashMap<OperationId, bool>,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationId {
    timestamp: u64,
    site_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    id: OperationId,
    position: LogicalPosition,
    content: String,
    dependencies: Vec<OperationId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogicalPosition {
    path: Vec<u32>,
    site_id: Uuid,
}

impl RiftCRDT {
    pub fn new(site_id: Uuid) -> Self {
        Self {
            site_id,
            lamport_clock: 0,
            operations: Vec::new(),
            tombstones: HashMap::new(),
        }
    }

    pub fn insert(&mut self, position: LogicalPosition, content: String) -> Operation {
        self.lamport_clock += 1;
        let op = Operation {
            id: OperationId {
                timestamp: self.lamport_clock,
                site_id: self.site_id,
            },
            position,
            content,
            dependencies: self.operations.iter()
                .map(|op| op.id.clone())
                .collect(),
        };
        self.operations.push(op.clone());
        op
    }

    pub fn delete(&mut self, op_id: OperationId) {
        self.tombstones.insert(op_id, true);
    }

    pub fn merge(&mut self, other: &RiftCRDT) {
        // Update Lamport clock
        self.lamport_clock = std::cmp::max(self.lamport_clock, other.lamport_clock) + 1;

        // Merge operations
        for op in &other.operations {
            if !self.operations.iter().any(|existing| existing.id == op.id) {
                self.operations.push(op.clone());
            }
        }

        // Merge tombstones
        for (op_id, deleted) in &other.tombstones {
            self.tombstones.insert(op_id.clone(), *deleted);
        }

        // Sort operations by position and timestamp
        self.operations.sort_by(|a, b| {
            match a.position.path.cmp(&b.position.path) {
                Ordering::Equal => a.id.timestamp.cmp(&b.id.timestamp),
                ord => ord,
            }
        });
    }

    pub fn get_content(&self) -> String {
        self.operations.iter()
            .filter(|op| !self.tombstones.contains_key(&op.id))
            .map(|op| op.content.clone())
            .collect()
    }
}

impl LogicalPosition {
    pub fn new(path: Vec<u32>, site_id: Uuid) -> Self {
        LogicalPosition { path, site_id }
    }

    pub fn between(left: &LogicalPosition, right: &LogicalPosition, site_id: Uuid) -> Self {
        let mut path = Vec::new();
        let mut i = 0;

        while i < left.path.len() && i < right.path.len() {
            if left.path[i] != right.path[i] {
                let mid = (left.path[i] + right.path[i]) / 2;
                path.push(mid);
                break;
            }
            path.push(left.path[i]);
            i += 1;
        }

        if path.len() == i {
            path.push(if i >= left.path.len() { 0 } else { left.path[i] + 1 });
        }

        LogicalPosition { path, site_id }
    }
} 