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

/// Where the results table sits on screen, so mouse clicks can map to rows.
#[derive(Debug, Clone, Copy, Default)]
pub struct Geometry {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
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
    /// Transient message shown in the footer, e.g. export results.
    pub notice: Option<String>,
    /// Table position on screen, kept up to date by the renderer.
    geometry: Geometry,
    /// First visible row of the table, kept up to date by the renderer.
    table_offset: usize,
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
            notice: None,
            geometry: Geometry::default(),
            table_offset: 0,
        }
    }

    /// Adds a result that arrived from a worker, or replaces the previous
    /// result for the same endpoint (used by the re-check key).
    pub fn push(&mut self, result: CheckResult) {
        if self.selected.is_none() {
            self.selected = Some(result.proxy.key());
        }

        let key = result.proxy.key();
        if let Some(position) = self.results.iter().position(|r| r.proxy.key() == key) {
            self.results[position] = result;
            return;
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

    pub fn notice(&mut self, message: impl Into<String>) {
        self.notice = Some(message.into());
    }

    /// The message currently shown in the footer.
    pub fn current_notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    /// Selects the visible row at `index`, clamped to the list.
    pub fn select_visible_index(&mut self, index: usize) {
        if let Some(result) = self.visible().get(index) {
            self.selected = Some(result.proxy.key());
        }
    }

    /// Maps an absolute screen row to a visible result index.
    pub fn row_at(&self, row: u16) -> Option<usize> {
        // One border row and one header row sit above the data rows.
        let first_data_row = self.geometry.y.saturating_add(2);
        if row < first_data_row || row >= self.geometry.y + self.geometry.height {
            return None;
        }
        let index = self.table_offset + (row - first_data_row) as usize;
        (index < self.visible_count()).then_some(index)
    }

    /// Records where the renderer placed the table and how far it scrolled.
    pub fn set_table_geometry(&mut self, offset: usize, geometry: Geometry) {
        self.table_offset = offset;
        self.geometry = geometry;
    }

    /// Endpoints of the visible alive rows, ready to be written to a file.
    pub fn export_endpoints(&self, show_credentials: bool) -> String {
        let mut output = String::new();
        for result in self.visible() {
            if !result.alive {
                continue;
            }
            let endpoint = if show_credentials {
                result.proxy.to_url()
            } else {
                result.proxy.redacted()
            };
            output.push_str(&endpoint);
            output.push('\n');
        }
        output
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
    fn push_replaces_the_previous_result_for_the_same_endpoint() {
        let mut app = dashboard(2);
        app.push(alive(1, 10));
        let rechecked = CheckResult::dead(
            Proxy::new(Protocol::Http, "10.0.0.1", 1),
            Duration::from_millis(5),
            "refused on retry",
        );
        app.push(rechecked);

        assert_eq!(app.checked(), 1);
        assert!(!app.results[0].alive);
        assert_eq!(app.results[0].proxy.port, 1);
    }

    #[test]
    fn export_lists_visible_alive_endpoints() {
        let mut app = dashboard(3);
        app.push(alive(1, 10));
        app.push(dead(2));
        app.push(alive(3, 20));

        let output = app.export_endpoints(false);
        assert_eq!(output, "http://10.0.0.1:1\nhttp://10.0.0.1:3\n");

        app.cycle_filter(); // alive only
        assert_eq!(
            app.export_endpoints(false),
            "http://10.0.0.1:1\nhttp://10.0.0.1:3\n"
        );

        app.cycle_filter(); // dead only
        assert_eq!(app.export_endpoints(false), "");
    }

    #[test]
    fn row_at_maps_screen_rows_to_results() {
        let mut app = dashboard(2);
        app.push(alive(1, 10));
        app.push(alive(2, 20));
        app.set_table_geometry(
            0,
            Geometry {
                x: 0,
                y: 5,
                width: 100,
                height: 20,
            },
        );

        // y = 5 is the top border, y = 6 the header, y = 7 the first row.
        assert_eq!(app.row_at(5), None);
        assert_eq!(app.row_at(6), None);
        assert_eq!(app.row_at(7), Some(0));
        assert_eq!(app.row_at(8), Some(1));
        assert_eq!(app.row_at(24), None); // past the table area
    }

    #[test]
    fn row_at_honours_the_scroll_offset() {
        let mut app = dashboard(5);
        for port in 1..=5 {
            app.push(alive(port, 10 * port as u64));
        }
        app.set_table_geometry(
            3,
            Geometry {
                x: 0,
                y: 5,
                width: 100,
                height: 20,
            },
        );
        assert_eq!(app.row_at(7), Some(3));
        assert_eq!(app.row_at(8), Some(4));
    }

    #[test]
    fn select_visible_index_moves_the_cursor() {
        let mut app = dashboard(2);
        app.push(alive(1, 10));
        app.push(alive(2, 20));
        app.select_visible_index(1);
        assert_eq!(app.selected_index(), 1);
    }

    #[test]
    fn anonymity_is_exposed_for_rendering() {
        let mut result = alive(1, 10);
        result.anonymity = Some(AnonymityLevel::Elite);
        assert_eq!(result.anonymity.map(|level| level.as_str()), Some("elite"));
    }
}
