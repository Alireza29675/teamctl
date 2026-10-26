//! Additions use a forward cursor; only the latest bounded window tracks revisions.

use std::collections::BTreeMap;

use team_core::chat::{ChatMessage, Repository, Result};

const REVISION_WINDOW: usize = 200;

pub(super) struct Transcript {
    initialized: bool,
    last: i64,
    initial_limit: usize,
    page_limit: usize,
    revisions: BTreeMap<i64, i64>,
}

impl Transcript {
    pub(super) fn new(initial_limit: usize, page_limit: usize) -> Self {
        Self {
            initialized: false,
            last: 0,
            initial_limit,
            page_limit,
            revisions: BTreeMap::new(),
        }
    }

    pub(super) fn poll(
        &mut self,
        repo: &Repository<'_>,
        conversation: &str,
    ) -> Result<Vec<ChatMessage>> {
        // Derive the startup cursor from exactly the page being displayed. A
        // second latest-row query could skip an arrival between these reads.
        let mut rows = if self.initialized {
            repo.history(conversation, None, Some(self.last), self.page_limit)?
        } else {
            repo.history(conversation, None, None, self.initial_limit)?
        };
        // Fetch revisions before changing state: a failed read must not consume
        // additions that the caller has not displayed.
        let revisions = if self.initialized {
            repo.history(conversation, None, None, REVISION_WINDOW)?
        } else {
            Vec::new()
        };
        self.initialized = true;
        for row in &rows {
            self.last = self.last.max(row.id);
            self.remember(row);
        }

        // Never advance the forward cursor from this snapshot: it may include
        // newer arrivals beyond the page above. Those will be drained in order.
        for row in revisions {
            if row.id <= self.last
                && self
                    .revisions
                    .get(&row.id)
                    .is_some_and(|revision| *revision != row.revision)
            {
                self.remember(&row);
                rows.push(row);
            }
        }
        Ok(rows)
    }

    fn remember(&mut self, row: &ChatMessage) {
        self.revisions.insert(row.id, row.revision);
        while self.revisions.len() > REVISION_WINDOW {
            self.revisions.pop_first();
        }
    }

    #[cfg(test)]
    pub(super) fn cached_revisions(&self) -> usize {
        self.revisions.len()
    }
}
