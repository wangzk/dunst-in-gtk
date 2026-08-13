//! Pure notification queue logic (ticket 05): which notifications display
//! now and which wait, honoring `notification_limit` and do-not-disturb.
//!
//! This module is GTK-free and D-Bus-free: the daemon feeds it plain
//! notification data and acts on the returned instructions, so the whole
//! queue contract is unit-testable here.
//!
//! Semantics (dunst):
//! - Notifications arrive in order; up to `limit` display at once (0 =
//!   unlimited). Excess notifications wait in a FIFO queue.
//! - While do-not-disturb is active, arrivals go straight to the queue.
//! - When a displayed notification closes, the oldest waiting notification
//!   is promoted (and the daemon displays it).
//! - `replaces_id` updates an existing notification in place (either
//!   displayed or waiting) instead of adding a new one.

use std::collections::VecDeque;

/// Everything needed to (re)create a notification window later.
#[derive(Debug, Clone)]
pub struct Pending {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub client: Option<String>,
    pub expire_timeout: i32,
    pub urgency: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyAction {
    /// Display the notification immediately.
    ShowNow,
    /// Queue it (limit reached or do-not-disturb active).
    Queue,
}

#[derive(Debug)]
pub struct QueueState {
    /// Max simultaneously displayed notifications; 0 = unlimited.
    limit: usize,
    /// How many are currently displayed (tracked by the daemon).
    displayed: usize,
    /// Do-not-disturb: while active, everything queues.
    paused: bool,
    waiting: VecDeque<Pending>,
}

impl QueueState {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            displayed: 0,
            paused: false,
            waiting: VecDeque::new(),
        }
    }

    /// A new notification arrived (or a replace that missed). Returns what
    /// the daemon should do. On `ShowNow` the caller keeps the data and
    /// displays it; on `Queue` a clone is stored here.
    pub fn notify(&mut self, pending: &Pending) -> NotifyAction {
        if self.paused || (self.limit > 0 && self.displayed >= self.limit) {
            self.waiting.push_back(pending.clone());
            NotifyAction::Queue
        } else {
            self.displayed += 1;
            NotifyAction::ShowNow
        }
    }

    /// A waiting notification was replaced by id; returns true on hit.
    pub fn replace_waiting(&mut self, id: u32, pending: Pending) -> bool {
        for p in self.waiting.iter_mut() {
            if p.id == id {
                *p = pending;
                return true;
            }
        }
        false
    }

    /// A displayed notification closed; promote the oldest waiting one if any.
    pub fn display_closed(&mut self) -> Option<Pending> {
        self.displayed = self.displayed.saturating_sub(1);
        if self.paused {
            // Keep waiting; do-not-disturb means nothing new displays.
            return None;
        }
        if self.limit == 0 || self.displayed < self.limit {
            return self.waiting.pop_front();
        }
        None
    }

    /// Called when the daemon actually displayed a promoted notification.
    #[allow(dead_code)] // used by tests; the daemon tracks counts via counters
    pub fn display_started(&mut self) {
        self.displayed += 1;
    }

    /// Toggle do-not-disturb; returns the notifications to display now (the
    /// daemon creates their windows), up to the limit.
    pub fn set_paused(&mut self, paused: bool) -> Vec<Pending> {
        self.paused = paused;
        if paused {
            return Vec::new();
        }
        let mut promote = Vec::new();
        while self.limit == 0 || self.displayed < self.limit {
            match self.waiting.pop_front() {
                Some(p) => {
                    self.displayed += 1;
                    promote.push(p);
                }
                None => break,
            }
        }
        promote
    }

    #[allow(dead_code)]
    pub fn paused(&self) -> bool {
        self.paused
    }

    #[allow(dead_code)]
    pub fn displayed_len(&self) -> usize {
        self.displayed
    }

    #[allow(dead_code)]
    pub fn waiting_len(&self) -> usize {
        self.waiting.len()
    }

    /// Remove a waiting notification by id (e.g. CloseNotification); returns
    /// it if found.
    pub fn remove_waiting(&mut self, id: u32) -> Option<Pending> {
        let idx = self.waiting.iter().position(|p| p.id == id)?;
        self.waiting.remove(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(id: u32) -> Pending {
        Pending {
            id,
            app_name: "t".into(),
            app_icon: String::new(),
            summary: format!("s{id}"),
            body: String::new(),
            actions: vec![],
            client: None,
            expire_timeout: 5000,
            urgency: 1,
        }
    }

    #[test]
    fn unlimited_by_default() {
        let mut q = QueueState::new(0);
        for i in 1..=5 {
            assert_eq!(q.notify(&pending(i)), NotifyAction::ShowNow);
        }
        assert_eq!(q.displayed_len(), 5);
        assert_eq!(q.waiting_len(), 0);
    }

    #[test]
    fn limit_queues_excess_and_promotes_fifo() {
        let mut q = QueueState::new(2);
        assert_eq!(q.notify(&pending(1)), NotifyAction::ShowNow);
        assert_eq!(q.notify(&pending(2)), NotifyAction::ShowNow);
        assert_eq!(q.notify(&pending(3)), NotifyAction::Queue);
        assert_eq!(q.notify(&pending(4)), NotifyAction::Queue);
        assert_eq!(q.waiting_len(), 2);

        // Closing one displayed notification promotes the oldest waiter.
        let promoted = q.display_closed().expect("a waiter");
        assert_eq!(promoted.id, 3);
        q.display_started();
        assert_eq!(q.displayed_len(), 2);
        assert_eq!(q.waiting_len(), 1);

        let promoted = q.display_closed().expect("a waiter");
        assert_eq!(promoted.id, 4);
        q.display_started();
        assert!(q.display_closed().is_none(), "nothing waiting");
        assert_eq!(q.displayed_len(), 1);
    }

    #[test]
    fn paused_queues_everything_and_unpause_promotes() {
        let mut q = QueueState::new(1);
        q.set_paused(true);
        assert!(q.paused());
        assert_eq!(q.notify(&pending(1)), NotifyAction::Queue);
        assert_eq!(q.notify(&pending(2)), NotifyAction::Queue);
        assert_eq!(q.waiting_len(), 2);

        let promote = q.set_paused(false);
        assert_eq!(promote.len(), 1, "limit 1: only one promoted");
        assert_eq!(promote[0].id, 1);
        q.display_started();
        assert_eq!(q.waiting_len(), 1);

        // While paused, closing displayed notifications does not promote.
        q.set_paused(true);
        assert!(q.display_closed().is_none());
    }

    #[test]
    fn paused_displayed_count_stays_consistent() {
        let mut q = QueueState::new(0);
        q.notify(&pending(1));
        q.set_paused(true);
        q.notify(&pending(2));
        assert_eq!(q.displayed_len(), 1);
        assert_eq!(q.waiting_len(), 1);
        q.set_paused(false);
        assert_eq!(q.waiting_len(), 0);
        assert_eq!(q.displayed_len(), 2);
    }

    #[test]
    fn replace_updates_waiting_in_place() {
        let mut q = QueueState::new(1);
        q.set_paused(true);
        q.notify(&pending(7));
        assert!(q.replace_waiting(7, pending(7)));
        assert_eq!(q.waiting_len(), 1);
        assert!(!q.replace_waiting(99, pending(99)));
    }

    #[test]
    fn remove_waiting_by_id() {
        let mut q = QueueState::new(0);
        q.set_paused(true);
        q.notify(&pending(1));
        q.notify(&pending(2));
        let removed = q.remove_waiting(1).expect("found");
        assert_eq!(removed.id, 1);
        assert_eq!(q.waiting_len(), 1);
        assert!(q.remove_waiting(42).is_none());
    }
}
