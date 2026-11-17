//! State behind the interactive dashboard.
//!
//! The model is deliberately free of any terminal dependency so navigation,
//! filtering and sorting can be unit tested without a real terminal.

use std::time::{Duration, Instant};

use crate::cli::Sort;
use crate::report::CheckResult;

/// Which results the dashboard shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Alive,
    Dead,
}

impl Filter {
    pub const fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Alive => "alive",
            Filter::Dead => "dead",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Filter::All => Filter::Alive,
            Filter::Alive => Filter::Dead,
            Filter::Dead => Filter::All,
        }
    }

    pub const fn matches(self, result: &CheckResult) -> bool {
        match self {
            Filter::All => true,
            Filter::Alive => result.alive,
            Filter::Dead => !result.alive,
        }
    }
}

/// Live view of a running (or finished) check batch.
#[derive(Debug)]
pub struct Dashboard {
    total: usize,
    results: Vec<CheckResult>,
    filter: Filter,
    sort: Sort,
    /// Identity of the selected row, so selection survives re-sorting.
    selected: Option<String>,
    started: Instant,
    finished_at: Option<Duration>,
    /// Whether the help overlay is visible.
    pub help: bool,
}

impl Dashboard {
    pub fn new(total: usize, sort: Sort) -> Self {
        Self {
            total,
            results: Vec::with_capacity(total),
            filter: Filter::All,
            sort,
            selected: None,
            started: Instant::now(),
            finished_at: None,
            help: false,
        }
    }

    /// Adds a result that arrived from a worker.
    pub fn push(&mut self, result: CheckResult) {
        if self.selected.is_none() {
            self.selected = Some(result.proxy.key());
        }
        self.results.push(result);
        if self.finished_at.is_none() && self.checked() == self.total {
            self.finished_at = Some(self.started.elapsed());
        }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn checked(&self) -> usize {
        self.results.len()
    }

    pub fn alive(&self) -> usize {
        self.results.iter().filter(|result| result.alive).count()
    }

    pub fn dead(&self) -> usize {
        self.checked() - self.alive()
    }

    pub fn is_finished(&self) -> bool {
        self.finished_at.is_some()
    }

    pub fn elapsed(&self) -> Duration {
        self.finished_at.unwrap_or_else(|| self.started.elapsed())
    }

    /// Fraction of the batch that has been checked, in `0.0..=1.0`.
    pub fn progress(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.checked() as f64 / self.total as f64
        }
    }

    pub fn filter(&self) -> Filter {
        self.filter
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    /// Filtered and sorted rows, ready for rendering.
    pub fn visible(&self) -> Vec<&CheckResult> {
        let mut rows: Vec<&CheckResult> = self
            .results
            .iter()
            .filter(|result| self.filter.matches(result))
            .collect();

        match self.sort {
            Sort::Latency => rows.sort_by_key(|result| (!result.alive, result.latency_ms())),
            Sort::Proxy => rows.sort_by_key(|result| result.proxy.redacted()),
            Sort::Status => rows.sort_by_key(|result| !result.alive),
        }

        rows
    }

    pub fn visible_count(&self) -> usize {
        self.visible().len()
    }

    pub fn selected_key(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn selected_index(&self) -> usize {
        let rows = self.visible();
        match &self.selected {
            Some(key) => rows
                .iter()
                .position(|result| &result.proxy.key() == key)
                .unwrap_or(0),
            None => 0,
        }
    }

    pub fn selected(&self) -> Option<&CheckResult> {
        let index = self.selected_index();
        self.visible().get(index).copied()
    }

    pub fn select_next(&mut self) {
        self.select_offset(1);
    }

    pub fn select_prev(&mut self) {
        self.select_offset(-1);
    }

    /// Moves by `delta` rows, clamped to the visible list.
    pub fn select_page(&mut self, delta: isize) {
        let rows = self.visible();
        if rows.is_empty() {
            self.selected = None;
            return;
        }
        let current = self.selected_index() as isize;
        let last = rows.len() as isize - 1;
        let index = (current + delta).clamp(0, last) as usize;
        self.selected = Some(rows[index].proxy.key());
    }

    pub fn select_first(&mut self) {
        self.selected = self.visible().first().map(|result| result.proxy.key());
    }

    pub fn select_last(&mut self) {
        self.selected = self.visible().last().map(|result| result.proxy.key());
    }

    pub fn cycle_filter(&mut self) {
        self.filter = self.filter.next();
        // Keep the cursor on a row that is still visible.
        self.select_first();
    }

    pub fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            Sort::Latency => Sort::Proxy,
            Sort::Proxy => Sort::Status,
            Sort::Status => Sort::Latency,
        };
    }

    pub fn toggle_help(&mut self) {
        self.help = !self.help;
    }

    fn select_offset(&mut self, delta: isize) {
        let rows = self.visible();
        if rows.is_empty() {
            self.selected = None;
            return;
        }
        let current = self.selected_index() as isize;
        let last = rows.len() as isize - 1;
        let index = (current + delta).clamp(0, last) as usize;
        self.selected = Some(rows[index].proxy.key());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anonymity::AnonymityLevel;
    use crate::proxy::{Protocol, Proxy};

    fn alive(port: u16, latency_ms: u64) -> CheckResult {
        CheckResult::alive(
            Proxy::new(Protocol::Http, "10.0.0.1", port),
            Duration::from_millis(latency_ms),
            Some(200),
        )
    }

    fn dead(port: u16) -> CheckResult {
        CheckResult::dead(
            Proxy::new(Protocol::Socks5, "10.0.0.2", port),
            Duration::from_millis(500),
            "refused",
        )
    }

    fn dashboard(total: usize) -> Dashboard {
        Dashboard::new(total, Sort::Latency)
    }

    #[test]
    fn counts_results_and_progress() {
        let mut app = dashboard(2);
        assert_eq!(app.checked(), 0);
        assert_eq!(app.progress(), 0.0);

        app.push(alive(1, 10));
        assert_eq!(app.checked(), 1);
        assert_eq!(app.alive(), 1);
        assert_eq!(app.dead(), 0);
        assert!((app.progress() - 0.5).abs() < f64::EPSILON);
        assert!(!app.is_finished());

        app.push(dead(2));
        assert_eq!(app.checked(), 2);
        assert_eq!(app.alive(), 1);
        assert_eq!(app.dead(), 1);
        assert_eq!(app.progress(), 1.0);
        assert!(app.is_finished());
    }

    #[test]
    fn elapsed_freezes_when_the_batch_finishes() {
        let mut app = dashboard(1);
        app.push(alive(1, 5));
        let elapsed = app.elapsed();
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(app.elapsed(), elapsed);
    }

    #[test]
    fn filter_cycles_and_hides_rows() {
        let mut app = dashboard(3);
        app.push(alive(1, 10));
        app.push(dead(2));
        app.push(alive(3, 20));

        assert_eq!(app.filter(), Filter::All);
        assert_eq!(app.visible_count(), 3);

        app.cycle_filter();
        assert_eq!(app.filter(), Filter::Alive);
        assert_eq!(app.visible_count(), 2);
        assert!(app.visible().iter().all(|result| result.alive));

        app.cycle_filter();
        assert_eq!(app.filter(), Filter::Dead);
        assert_eq!(app.visible_count(), 1);
        assert!(!app.visible()[0].alive);

        app.cycle_filter();
        assert_eq!(app.filter(), Filter::All);
        assert_eq!(app.visible_count(), 3);
    }

    #[test]
    fn latency_sort_puts_dead_last_and_fastest_first() {
        let mut app = dashboard(3);
        app.push(dead(1));
        app.push(alive(2, 300));
        app.push(alive(3, 100));

        let ports: Vec<u16> = app.visible().iter().map(|r| r.proxy.port).collect();
        assert_eq!(ports, vec![3, 2, 1]);
    }

    #[test]
    fn proxy_sort_is_alphabetical() {
        let mut app = Dashboard::new(2, Sort::Proxy);
        app.push(alive(2, 10));
        app.push(alive(1, 20));
        let ports: Vec<u16> = app.visible().iter().map(|r| r.proxy.port).collect();
        assert_eq!(ports, vec![1, 2]);
    }

    #[test]
    fn status_sort_keeps_input_order_within_groups() {
        let mut app = Dashboard::new(3, Sort::Status);
        app.push(dead(1));
        app.push(alive(2, 30));
        app.push(alive(3, 20));
        let ports: Vec<u16> = app.visible().iter().map(|r| r.proxy.port).collect();
        assert_eq!(ports, vec![2, 3, 1]);
    }

    #[test]
    fn sort_cycles_and_keeps_selection() {
        let mut app = dashboard(2);
        app.push(alive(1, 10));
        app.push(alive(2, 20));
        app.select_last();
        let selected = app.selected_key().map(str::to_string);

        app.cycle_sort();
        assert_eq!(app.sort(), Sort::Proxy);
        assert_eq!(app.selected_key().map(str::to_string), selected);

        app.cycle_sort();
        assert_eq!(app.sort(), Sort::Status);
        app.cycle_sort();
        assert_eq!(app.sort(), Sort::Latency);
    }

    #[test]
    fn navigation_clamps_to_the_visible_rows() {
        let mut app = dashboard(3);
        app.push(alive(1, 10));
        app.push(alive(2, 20));
        app.push(alive(3, 30));
        assert_eq!(app.selected_index(), 0);

        app.select_prev();
        assert_eq!(app.selected_index(), 0);

        app.select_next();
        assert_eq!(app.selected_index(), 1);

        app.select_page(10);
        assert_eq!(app.selected_index(), 2);

        app.select_next();
        assert_eq!(app.selected_index(), 2);

        app.select_first();
        assert_eq!(app.selected_index(), 0);

        app.select_last();
        assert_eq!(app.selected_index(), 2);
    }

    #[test]
    fn selection_survives_new_results() {
        let mut app = dashboard(3);
        app.push(alive(1, 100));
        app.push(alive(2, 200));
        app.select_last();
        let selected = app.selected_key().map(str::to_string);

        app.push(alive(3, 50));
        assert_eq!(app.selected_key().map(str::to_string), selected);
        // A faster result moves the row above the selection.
        assert_eq!(app.selected_index(), 2);
    }

    #[test]
    fn filtering_resets_selection_to_a_visible_row() {
        let mut app = dashboard(2);
        app.push(dead(1));
        app.push(alive(2, 10));
        app.select_last();

        app.cycle_filter(); // alive only
        assert_eq!(app.selected().map(|r| r.proxy.port), Some(2));

        app.cycle_filter(); // dead only
        assert_eq!(app.selected().map(|r| r.proxy.port), Some(1));
    }

    #[test]
    fn anonymity_is_exposed_for_rendering() {
        let mut result = alive(1, 10);
        result.anonymity = Some(AnonymityLevel::Elite);
        assert_eq!(result.anonymity.map(|level| level.as_str()), Some("elite"));
    }
}
