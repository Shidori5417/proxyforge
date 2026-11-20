//! The settings panel: editable run options inside the dashboard.
//!
//! Opening the panel with `s` shows the values the batch runs with. Enter
//! restarts the whole batch with the edited values; Escape closes the panel
//! without touching anything.

use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::checker::CheckSettings;
use crate::transport::Target;

/// How one settings field is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Free text, typed directly.
    Text,
    /// Adjusted with ◀ ▶ or typed digits.
    Number { step: u64, min: u64, max: u64 },
    /// Flipped with Enter, ◀ or ▶.
    Toggle,
}

/// One row of the panel.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub label: &'static str,
    pub kind: FieldKind,
}

/// Panel layout, top to bottom.
pub const FIELDS: &[Field] = &[
    Field {
        label: "Judge URL(s)",
        kind: FieldKind::Text,
    },
    Field {
        label: "Timeout (s)",
        kind: FieldKind::Number {
            step: 1,
            min: 1,
            max: 120,
        },
    },
    Field {
        label: "Concurrency",
        kind: FieldKind::Number {
            step: 25,
            min: 1,
            max: 2000,
        },
    },
    Field {
        label: "Retries",
        kind: FieldKind::Number {
            step: 1,
            min: 0,
            max: 10,
        },
    },
    Field {
        label: "Protocol detection",
        kind: FieldKind::Toggle,
    },
];

/// The editable view of a [`CheckSettings`].
#[derive(Debug, Clone, PartialEq)]
pub struct RunSettings {
    /// Judge URLs, space or comma separated.
    pub judges: String,
    pub timeout: u64,
    pub concurrency: usize,
    pub retries: u32,
    pub detect: bool,
}

impl RunSettings {
    /// Captures the settings the current batch runs with.
    pub fn from_settings(settings: &CheckSettings, concurrency: usize) -> Self {
        let judges = settings
            .judges
            .iter()
            .map(|target| target.absolute_uri())
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            judges,
            timeout: settings.timeout.as_secs(),
            concurrency,
            retries: settings.retries,
            detect: settings.detect,
        }
    }

    /// Turns the edits into a fresh settings set, keeping everything the
    /// panel does not show (anonymity, GeoIP, expect, local IP).
    pub fn apply(&self, base: &CheckSettings) -> Result<CheckSettings> {
        let mut judges = Vec::new();
        for token in self.judges.split(|c: char| c == ',' || c.is_whitespace()) {
            if token.is_empty() {
                continue;
            }
            let target =
                Target::parse(token).with_context(|| format!("invalid judge URL `{token}`"))?;
            judges.push(target);
        }
        if judges.is_empty() {
            bail!("at least one judge URL is required");
        }

        Ok(CheckSettings {
            judges,
            timeout: Duration::from_secs(self.timeout.max(1)),
            retries: self.retries,
            detect: self.detect,
            ..base.clone()
        })
    }

    /// The panel is edited through this clone.
    pub fn panel(self) -> Panel {
        Panel {
            settings: self,
            field: 0,
            typing: String::new(),
        }
    }

    /// Text shown for one field.
    pub fn value_of(&self, index: usize) -> String {
        match index {
            0 => self.judges.clone(),
            1 => self.timeout.to_string(),
            2 => self.concurrency.to_string(),
            3 => self.retries.to_string(),
            4 => on_off(self.detect),
            _ => String::new(),
        }
    }
}

fn on_off(enabled: bool) -> String {
    if enabled { "on".into() } else { "off".into() }
}

/// The open settings panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub settings: RunSettings,
    /// Focused field index.
    pub field: usize,
    /// Digits typed since the last ◀ ▶ or focus move; empty when the field
    /// shows a stored value instead of a draft.
    typing: String,
}

impl Panel {
    /// Moves the focus, wrapping around at the ends.
    pub fn focus_next(&mut self) {
        self.field = (self.field + 1) % FIELDS.len();
        self.typing.clear();
    }

    pub fn focus_prev(&mut self) {
        self.field = (self.field + FIELDS.len() - 1) % FIELDS.len();
        self.typing.clear();
    }

    /// Applies one key press to the focused field.
    pub fn edit(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        let kind = FIELDS[self.field].kind;
        let code = key.code;

        if code == KeyCode::Backspace {
            if self.typing.is_empty() {
                self.edit_text(|text| {
                    text.pop();
                });
            } else {
                self.typing.pop();
                self.commit_typing(0, u64::MAX);
            }
            return;
        }

        match kind {
            FieldKind::Text => {
                if let KeyCode::Char(ch) = code
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.edit_text(|text| text.push(ch));
                }
            }
            FieldKind::Number { step, min, max } => {
                match code {
                    KeyCode::Left => {
                        self.typing.clear();
                        self.edit_number(min, max, -(step as i64));
                    }
                    KeyCode::Right => {
                        self.typing.clear();
                        self.edit_number(min, max, step as i64);
                    }
                    KeyCode::Char(ch) if ch.is_ascii_digit() => {
                        // The first digit starts a fresh draft, replacing the
                        // stored value; the rest extend it.
                        self.typing.push(ch);
                        self.commit_typing(min, max);
                    }
                    _ => {}
                }
            }
            FieldKind::Toggle => {
                if matches!(code, KeyCode::Enter | KeyCode::Left | KeyCode::Right) {
                    self.toggle();
                }
            }
        }
    }

    /// The settings as they stand, for Enter to apply.
    pub fn snapshot(&self) -> RunSettings {
        self.settings.clone()
    }

    fn toggle(&mut self) {
        if self.field == 4 {
            self.settings.detect = !self.settings.detect;
        }
    }

    fn edit_number(&mut self, min: u64, max: u64, delta: i64) {
        let step = |current: u64, delta: i64| -> u64 {
            let next = current as i64 + delta;
            next.clamp(min as i64, max as i64) as u64
        };

        match self.field {
            1 => self.settings.timeout = step(self.settings.timeout, delta),
            2 => self.settings.concurrency = step(self.settings.concurrency as u64, delta) as usize,
            3 => self.settings.retries = step(self.settings.retries as u64, delta) as u32,
            _ => {}
        }
    }

    /// Writes the digit draft into the focused number, clamped to its range.
    fn commit_typing(&mut self, min: u64, max: u64) {
        let value = self.typing.parse::<u64>().unwrap_or(min).clamp(min, max);
        match self.field {
            1 => self.settings.timeout = value,
            2 => self.settings.concurrency = value as usize,
            3 => self.settings.retries = value as u32,
            _ => {}
        }
    }

    fn edit_text(&mut self, change: impl FnOnce(&mut String)) {
        match self.field {
            0 => change(&mut self.settings.judges),
            1 => {
                let mut text = self.settings.timeout.to_string();
                change(&mut text);
                self.settings.timeout = text.parse().unwrap_or(self.settings.timeout);
            }
            2 => {
                let mut text = self.settings.concurrency.to_string();
                change(&mut text);
                self.settings.concurrency = text.parse().unwrap_or(self.settings.concurrency);
            }
            3 => {
                let mut text = self.settings.retries.to_string();
                change(&mut text);
                self.settings.retries = text.parse().unwrap_or(self.settings.retries);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{Protocol, Proxy};
    use crossterm::event::KeyCode;

    fn settings() -> CheckSettings {
        CheckSettings {
            judges: vec![Target::parse("https://httpbin.org/get").unwrap()],
            timeout: Duration::from_secs(8),
            retries: 0,
            expect: None,
            detect: false,
            geo: None,
            anonymity: true,
            local_ip: Some("198.51.100.1".into()),
        }
    }

    fn panel() -> Panel {
        RunSettings::from_settings(&settings(), 100).panel()
    }

    #[test]
    fn captures_the_running_settings() {
        let run = RunSettings::from_settings(&settings(), 150);
        assert!(run.judges.contains("httpbin.org"));
        assert_eq!(run.timeout, 8);
        assert_eq!(run.concurrency, 150);
        assert!(!run.detect);
    }

    #[test]
    fn apply_parses_judges_and_keeps_hidden_settings() {
        let mut run = RunSettings::from_settings(&settings(), 100);
        run.judges = "https://a.example/get, https://b.example/get".into();
        run.timeout = 3;
        run.detect = true;

        let applied = run.apply(&settings()).unwrap();
        assert_eq!(applied.judges.len(), 2);
        assert_eq!(applied.timeout, Duration::from_secs(3));
        assert!(applied.detect);
        // Untouched parts survive.
        assert_eq!(applied.local_ip.as_deref(), Some("198.51.100.1"));
        assert!(applied.anonymity);
    }

    #[test]
    fn apply_rejects_empty_and_broken_judges() {
        let mut run = RunSettings::from_settings(&settings(), 100);
        run.judges = "   ".into();
        assert!(run.apply(&settings()).is_err());

        run.judges = "not a url".into();
        assert!(run.apply(&settings()).is_err());
    }

    #[test]
    fn focus_wraps_around() {
        let mut panel = panel();
        assert_eq!(panel.field, 0);
        panel.focus_prev();
        assert_eq!(panel.field, FIELDS.len() - 1);
        panel.focus_next();
        assert_eq!(panel.field, 0);
    }

    fn press(panel: &mut Panel, code: crossterm::event::KeyCode) {
        panel.edit(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        ));
    }

    #[test]
    fn numbers_step_and_clamp() {
        let mut panel = panel();
        panel.field = 1; // timeout
        press(&mut panel, KeyCode::Right);
        assert_eq!(panel.settings.timeout, 9);
        press(&mut panel, KeyCode::Left);
        press(&mut panel, KeyCode::Left);
        assert_eq!(panel.settings.timeout, 7);

        for _ in 0..200 {
            press(&mut panel, KeyCode::Right);
        }
        assert_eq!(panel.settings.timeout, 120);

        for _ in 0..300 {
            press(&mut panel, KeyCode::Left);
        }
        assert_eq!(panel.settings.timeout, 1);
    }

    #[test]
    fn concurrency_steps_by_its_own_pace() {
        let mut panel = panel();
        panel.field = 2;
        assert_eq!(panel.settings.concurrency, 100);
        press(&mut panel, KeyCode::Right);
        assert_eq!(panel.settings.concurrency, 125);
    }

    #[test]
    fn digits_replace_and_extend_numbers() {
        let mut panel = panel();
        panel.field = 1;
        press(&mut panel, KeyCode::Char('3'));
        press(&mut panel, KeyCode::Char('0'));
        assert_eq!(panel.settings.timeout, 30);
        press(&mut panel, KeyCode::Backspace);
        assert_eq!(panel.settings.timeout, 3);
    }

    #[test]
    fn toggles_flip_on_enter_and_arrows() {
        let mut panel = panel();
        panel.field = 4;
        assert!(!panel.settings.detect);
        press(&mut panel, KeyCode::Enter);
        assert!(panel.settings.detect);
        press(&mut panel, KeyCode::Right);
        assert!(!panel.settings.detect);
    }

    #[test]
    fn text_fields_take_characters() {
        let mut panel = panel();
        panel.field = 0;
        press(&mut panel, KeyCode::Char(' '));
        press(&mut panel, KeyCode::Char('x'));
        assert!(panel.settings.judges.ends_with(" x"));
    }

    #[test]
    fn value_of_renders_every_field() {
        let run = RunSettings::from_settings(&settings(), 7);
        assert!(run.value_of(0).contains("httpbin"));
        assert_eq!(run.value_of(1), "8");
        assert_eq!(run.value_of(2), "7");
        assert_eq!(run.value_of(3), "0");
        assert_eq!(run.value_of(4), "off");
    }

    #[test]
    fn hidden_settings_stay_out_of_the_panel() {
        // The panel never touches these, so apply must carry them over.
        let mut base = settings();
        base.expect = Some("origin".into());
        let run = RunSettings::from_settings(&base, 10);
        let applied = run.apply(&base).unwrap();
        assert_eq!(applied.expect.as_deref(), Some("origin"));
        let _ = Proxy::new(Protocol::Http, "0.0.0.0", 1);
    }
}
