// src/dataplane/multipath.rs
//! Multipath Transport
//! ===================
//!
//! Send data across multiple paths simultaneously

use std::collections::HashMap;
use crate::dataplane::transport::DataTransport;

/// Path identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathId(u64);

/// Path state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathState {
    Active,
    Degraded,
    Failed,
}

/// Path selector for multipath
pub struct PathSelector {
    paths: HashMap<PathId, PathState>,
    active_paths: Vec<PathId>,
}

impl PathSelector {
    pub fn new() -> Self {
        Self {
            paths: HashMap::new(),
            active_paths: Vec::new(),
        }
    }

    /// Add path
    pub fn add_path(&mut self, path_id: PathId) {
        self.paths.insert(path_id, PathState::Active);
        self.active_paths.push(path_id);
    }

    /// Select best path
    pub fn select_best_path(&self) -> Option<PathId> {
        // Return first active path
        for &path_id in &self.active_paths {
            if let Some(&PathState::Active) = self.paths.get(&path_id) {
                return Some(path_id);
            }
        }
        None
    }

    /// Mark path as degraded
    pub fn mark_degraded(&mut self, path_id: PathId) {
        self.paths.insert(path_id, PathState::Degraded);
    }

    /// Mark path as failed
    pub fn mark_failed(&mut self, path_id: PathId) {
        self.paths.insert(path_id, PathState::Failed);
    }

    /// Get active paths count
    pub fn active_count(&self) -> usize {
        self.active_paths.iter()
            .filter(|id| {
                self.paths.get(id)
                    .map(|s| *s == PathState::Active)
                    .unwrap_or(false)
            })
            .count()
    }
}

/// Multipath manager
pub struct MultipathManager {
    selector: PathSelector,
    transports: HashMap<PathId, DataTransport>,
}

impl MultipathManager {
    pub fn new() -> Self {
        Self {
            selector: PathSelector::new(),
            transports: HashMap::new(),
        }
    }

    /// Add transport
    pub fn add_transport(&mut self, path_id: PathId, transport: DataTransport) {
        self.selector.add_path(path_id);
        self.transports.insert(path_id, transport);
    }

    /// Select best transport
    pub fn select_transport(&self) -> Option<&DataTransport> {
        self.selector.select_best_path()
            .and_then(|id| self.transports.get(&id))
    }

    /// Get transport by ID
    pub fn get_transport(&self, path_id: PathId) -> Option<&DataTransport> {
        self.transports.get(&path_id)
    }

    /// Get active paths count
    pub fn active_paths(&self) -> usize {
        self.selector.active_count()
    }
}
