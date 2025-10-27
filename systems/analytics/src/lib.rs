#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Deterministic analytics system that schedules background recomputation.

use std::collections::VecDeque;

use maze_defence_core::{CellCoord, Command, Event, StatsReport};

/// Pure analytics system that queues recompute requests and emits published reports.
#[derive(Debug, Default)]
pub struct Analytics {
    last_report: Option<StatsReport>,
    pending_requests: VecDeque<RecomputeRequest>,
    scratch_path: Vec<CellCoord>,
    scratch_frontier: VecDeque<CellCoord>,
}

impl Analytics {
    /// Creates a new analytics system with empty caches and scratch buffers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the last analytics report published by the system, if any.
    #[must_use]
    pub fn last_report(&self) -> Option<&StatsReport> {
        self.last_report.as_ref()
    }

    /// Consumes world events and applied commands to publish analytics updates.
    ///
    /// The provided `recompute` closure is invoked at most once per call and only when a
    /// recompute request is pending *and* a tick (`Event::TimeAdvanced`) has been observed.
    /// The closure receives mutable access to reusable scratch buffers so metric
    /// computation can avoid repeated allocations.
    pub fn handle<F>(
        &mut self,
        events: &[Event],
        commands: &[Command],
        mut recompute: F,
        out: &mut Vec<Event>,
    ) where
        F: FnMut(&mut AnalyticsScratch<'_>) -> Option<StatsReport>,
    {
        let mut tick_observed = false;

        for event in events {
            match event {
                Event::MazeLayoutChanged => self.enqueue_request(RecomputeRequest::LayoutChanged),
                Event::TimeAdvanced { .. } => {
                    tick_observed = true;
                }
                _ => {}
            }
        }

        for command in commands {
            if matches!(command, Command::RequestAnalyticsRefresh) {
                self.enqueue_request(RecomputeRequest::ManualRefresh);
            }
        }

        if !tick_observed {
            return;
        }

        if self.pending_requests.pop_front().is_none() {
            return;
        }

        self.pending_requests.clear();

        let mut scratch = AnalyticsScratch::new(&mut self.scratch_path, &mut self.scratch_frontier);
        if let Some(report) = recompute(&mut scratch) {
            self.last_report = Some(report.clone());
            out.push(Event::AnalyticsUpdated { report });
        }
    }

    fn enqueue_request(&mut self, request: RecomputeRequest) {
        match request {
            RecomputeRequest::LayoutChanged => {
                self.pending_requests.clear();
                self.pending_requests.push_back(request);
            }
            RecomputeRequest::ManualRefresh => {
                if self.pending_requests.is_empty() {
                    self.pending_requests.push_back(request);
                }
            }
        }
    }
}

/// Scratch buffers reused by analytics metric computation.
#[derive(Debug)]
pub struct AnalyticsScratch<'a> {
    path: &'a mut Vec<CellCoord>,
    frontier: &'a mut VecDeque<CellCoord>,
}

impl<'a> AnalyticsScratch<'a> {
    fn new(path: &'a mut Vec<CellCoord>, frontier: &'a mut VecDeque<CellCoord>) -> Self {
        Self { path, frontier }
    }

    /// Returns a mutable reference to the reusable path buffer.
    #[must_use]
    pub fn path(&mut self) -> &mut Vec<CellCoord> {
        self.path
    }

    /// Returns a mutable reference to the reusable traversal frontier buffer.
    #[must_use]
    pub fn frontier(&mut self) -> &mut VecDeque<CellCoord> {
        self.frontier
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecomputeRequest {
    LayoutChanged,
    ManualRefresh,
}

#[cfg(test)]
mod tests {
    use super::{Analytics, AnalyticsScratch, RecomputeRequest};

    #[test]
    fn manual_requests_queue_once() {
        let mut analytics = Analytics::default();
        assert!(analytics.pending_requests.is_empty());

        analytics.enqueue_request(RecomputeRequest::ManualRefresh);
        assert_eq!(analytics.pending_requests.len(), 1);

        analytics.enqueue_request(RecomputeRequest::ManualRefresh);
        assert_eq!(analytics.pending_requests.len(), 1);

        analytics.enqueue_request(RecomputeRequest::LayoutChanged);
        assert_eq!(analytics.pending_requests.len(), 1);

        if let Some(request) = analytics.pending_requests.pop_front() {
            assert_eq!(request, RecomputeRequest::LayoutChanged);
        } else {
            panic!("expected pending layout request");
        }

        analytics.enqueue_request(RecomputeRequest::ManualRefresh);
        assert_eq!(analytics.pending_requests.len(), 1);
    }

    #[test]
    fn scratch_access_returns_buffers() {
        use maze_defence_core::CellCoord;
        use std::collections::VecDeque;

        let mut path = Vec::new();
        let mut frontier = VecDeque::new();
        let mut scratch = AnalyticsScratch::new(&mut path, &mut frontier);
        scratch.path().push(CellCoord::new(0, 0));
        scratch.frontier().push_back(CellCoord::new(1, 1));
    }
}
