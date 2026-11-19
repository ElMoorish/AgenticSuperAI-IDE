//! Pending user approvals (agent edits and command permissions) and the accepted edits that are
//! waiting for the UI to confirm it applied them.
//!
//! Life of an edit approval:
//!   1. `add` (the agent asked to write a file; the UI shows a card),
//!   2. `resolve(id, accept)`: rejected -> the caller answers the agent with an error;
//!      accepted -> the caller either writes the file itself (new files) or hands the edit to the UI
//!      (`await_confirmation`, then emits `ApplyEdit`),
//!   3. `confirm(id)`: the UI reports it applied (or failed to apply) the edit: only now does the
//!      caller answer the agent's `fs/write_text_file`.
//! `clear` forgets everything when the agent goes away (it fails its own pending calls).
//!
//! The store never talks to the agent: it only tracks state, so it can be tested without a process.

use crate::editor::{PendingCommand, PendingEdit};
use crate::files::FileStamp;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub enum Pending {
    /// `stamp: None` = the file did not exist: the write must be create-only.
    Edit { pe: PendingEdit, stamp: Option<FileStamp> },
    Command(PendingCommand),
}

impl Pending {
    pub fn agent_id(&self) -> &Value {
        match self {
            Pending::Edit { pe, .. } => &pe.agent_id,
            Pending::Command(pc) => &pc.agent_id,
        }
    }
}

/// What the caller must do after `resolve`.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// Answer the permission request with allow (`accept`) or reject.
    Command { agent_id: Value, accept: bool },
    /// Answer the agent's write request with a "rejected by the user" error.
    EditRejected { agent_id: Value },
    /// The user accepted. Nothing has been written and the agent has not been answered.
    EditAccepted { pe: PendingEdit, stamp: Option<FileStamp> },
}

#[derive(Default)]
pub struct PendingStore {
    pending: Mutex<HashMap<u64, Pending>>,
    /// Accepted edits handed to the UI, keyed by approval id, until it confirms.
    awaiting: Mutex<HashMap<u64, PendingEdit>>,
}

impl PendingStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, id: u64, p: Pending) {
        self.pending.lock().unwrap().insert(id, p);
    }

    /// Remove and interpret an approval. `None` = unknown id (stale, duplicate or cleared).
    pub fn resolve(&self, id: u64, accept: bool) -> Option<Resolution> {
        let p = self.pending.lock().unwrap().remove(&id)?;
        Some(match p {
            Pending::Command(pc) => Resolution::Command { agent_id: pc.agent_id, accept },
            Pending::Edit { pe, .. } if !accept => Resolution::EditRejected { agent_id: pe.agent_id },
            Pending::Edit { pe, stamp } => Resolution::EditAccepted { pe, stamp },
        })
    }

    /// Record that `pe` was handed to the UI and the agent is still waiting for its answer.
    pub fn await_confirmation(&self, id: u64, pe: PendingEdit) {
        self.awaiting.lock().unwrap().insert(id, pe);
    }

    /// The UI (or the fallback timer) reports on an edit handed over earlier. `None` = already
    /// confirmed, never handed over, or cleared (so a duplicate confirm cannot answer twice).
    pub fn confirm(&self, id: u64) -> Option<PendingEdit> {
        self.awaiting.lock().unwrap().remove(&id)
    }

    /// Still-pending approvals, oldest first (replayed when the UI re-subscribes).
    pub fn pending_snapshot(&self) -> Vec<(u64, Pending)> {
        let g = self.pending.lock().unwrap();
        let mut v: Vec<(u64, Pending)> = g.iter().map(|(id, p)| (*id, p.clone())).collect();
        v.sort_by_key(|(id, _)| *id);
        v
    }

    /// Accepted edits still waiting for the UI, oldest first (replayed on re-subscribe).
    pub fn awaiting_snapshot(&self) -> Vec<(u64, PendingEdit)> {
        let g = self.awaiting.lock().unwrap();
        let mut v: Vec<(u64, PendingEdit)> = g.iter().map(|(id, pe)| (*id, pe.clone())).collect();
        v.sort_by_key(|(id, _)| *id);
        v
    }

    /// Forget everything (agent exited, restarted, gave up, workspace closed). Returns how many
    /// approvals and awaiting edits were dropped.
    pub fn clear(&self) -> (usize, usize) {
        let mut p = self.pending.lock().unwrap();
        let mut a = self.awaiting.lock().unwrap();
        let n = (p.len(), a.len());
        p.clear();
        a.clear();
        n
    }

    pub fn is_empty(&self) -> bool {
        self.pending.lock().unwrap().is_empty() && self.awaiting.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit(id: u64) -> (u64, Pending) {
        let pe = PendingEdit { id, agent_id: json!(100 + id), path: "a.txt".into(), old: "o".into(), new: "n".into() };
        (id, Pending::Edit { pe, stamp: None })
    }
    fn cmd(id: u64) -> (u64, Pending) {
        let pc = PendingCommand { id, agent_id: json!(200 + id), title: "run".into(), command: "cargo test".into() };
        (id, Pending::Command(pc))
    }

    #[test]
    fn accept_waits_for_ui_confirmation_before_replying() {
        let s = PendingStore::new();
        let (id, p) = edit(1);
        s.add(id, p);
        // accepting hands the edit over but answers nobody: the caller has no reply to send yet
        let Some(Resolution::EditAccepted { pe, .. }) = s.resolve(1, true) else { panic!("expected EditAccepted") };
        s.await_confirmation(1, pe);
        assert_eq!(s.awaiting_snapshot().len(), 1, "still waiting for the UI");
        assert!(s.pending_snapshot().is_empty());
        assert!(s.resolve(1, true).is_none(), "a second accept does nothing");
        // only the confirmation yields the agent request to answer, and only once
        let pe = s.confirm(1).expect("confirm returns the edit to answer");
        assert_eq!(pe.agent_id, json!(101));
        assert!(s.confirm(1).is_none(), "duplicate confirm cannot answer the agent twice");
        assert!(s.is_empty());
    }

    #[test]
    fn reject_replies_reject() {
        let s = PendingStore::new();
        let (i, p) = edit(1);
        s.add(i, p);
        let (i, p) = cmd(2);
        s.add(i, p);
        match s.resolve(1, false) {
            Some(Resolution::EditRejected { agent_id }) => assert_eq!(agent_id, json!(101)),
            other => panic!("{other:?}"),
        }
        match s.resolve(2, false) {
            Some(Resolution::Command { agent_id, accept }) => {
                assert_eq!(agent_id, json!(202));
                assert!(!accept);
            }
            other => panic!("{other:?}"),
        }
        assert!(s.is_empty());
        // allowing a command is not a rejection
        let (i, p) = cmd(3);
        s.add(i, p);
        assert!(matches!(s.resolve(3, true), Some(Resolution::Command { accept: true, .. })));
    }

    #[test]
    fn unresolved_approval_cleared_on_agent_exit() {
        let s = PendingStore::new();
        for (i, p) in [edit(1), cmd(2), edit(3)] {
            s.add(i, p);
        }
        let Some(Resolution::EditAccepted { pe, .. }) = s.resolve(3, true) else { panic!() };
        s.await_confirmation(3, pe);
        assert_eq!(s.clear(), (2, 1));
        assert!(s.is_empty());
        assert!(s.confirm(3).is_none(), "a late UI confirmation after the agent died answers nothing");
        assert!(s.resolve(1, true).is_none());
    }

    #[test]
    fn snapshots_are_ordered_oldest_first() {
        let s = PendingStore::new();
        for (i, p) in [cmd(5), edit(2), cmd(9)] {
            s.add(i, p);
        }
        let ids: Vec<u64> = s.pending_snapshot().into_iter().map(|(i, _)| i).collect();
        assert_eq!(ids, vec![2, 5, 9]);
    }
}
