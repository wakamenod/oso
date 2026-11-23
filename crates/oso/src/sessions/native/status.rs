//! A native session's status line (ADR 0165, part 3a): the model, its effort once noted, the
//! permission mode, how much of the context is used, what the session cost and how much of the
//! rate limit's window is used, from what the stream and the control layer say. The modeline's
//! note shows its parts after the run's state, `Haiku 5.5 · default · ctx 3% · $0.82 · 7d 28%`,
//! each once known.
//!
//! Must not depend on: threads, the stream or the UI; plain values over the events a caller
//! gives.
//!
//! - The model is the one that answered last, on the main thread, as plan mode answers with
//!   another (measured: Sonnet under `--model haiku`); until one answered, `system/init`'s. It
//!   is shown by the name `initialize` lists for its id, else as its id less `claude-`.
//! - The context used is the last main-thread answer's input, cache creation and cache read
//!   tokens, as `/context` counts them (measured), over that model's context window, which a
//!   turn's `result` gives; when the `result`s named one model only, its window, whatever the
//!   model's id. A synthetic answer, such as a local command's, says neither.
//! - The cost is the last `result`'s, which counts the session's earlier processes too
//!   (measured); before one, `get_usage`'s, if it is more than nothing.
//! - The window is the last `rate_limit_event`'s, as it names the window that binds; before one,
//!   the more used of `get_usage`'s two. Only `five_hour` and `seven_day` are known.
//! - The permission mode is `mode`'s (part 3b), which the caller gives the line to write.
//! - What the stream gives is held to what reads as itself (`wire::status_line::readable`): a
//!   model or mode that could not, or that the stream gave longer than 256 bytes, is not
//!   shown, and a number out of reason is none.
//! - The parts are joined by [`SEPARATOR`], which oso-core's modeline drops them by, from the
//!   right, in a narrow window: the two must agree, and no part may hold a dot like it.

use std::fmt::Write;

use super::wire::status_line::{Kind, Model, Usage, readable};
use super::wire::{Event, MAX_WINDOWS};

/// A synthetic answer's model: a local command's, which used no model.
const SYNTHETIC: &str = "<synthetic>";
/// What the parts are joined with, as oso-core's modeline drops them by (part 3a).
pub const SEPARATOR: &str = " · ";
/// The most a cost may be and be shown, in US dollars.
const MAX_COST: f64 = 1e6;
/// The most a percentage may be and be shown.
const MAX_PERCENT: f64 = 999.0;

/// What a native session's status line knows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusLine {
    /// The models `initialize` listed, the last process's that listed any, and how many times
    /// a list came, so that completion over them (part 3c) is made again only then.
    models: Vec<Model>,
    listed: u64,
    /// The model `system/init` named last, and the one that answered last.
    init_model: Option<String>,
    answered: Option<String>,
    /// The tokens the last main-thread answer read.
    context: Option<u64>,
    /// Each model's context window, as the last `result`s gave them.
    windows: Vec<(String, u64)>,
    /// The cost a `result` gave, and `get_usage`'s.
    cost: Option<f64>,
    asked_cost: Option<f64>,
    /// The last `rate_limit_event`'s window and its use, in percent.
    limit: Option<(Window, f64)>,
    /// `get_usage`'s windows, in percent.
    usage: [Option<f64>; 2],
}

/// A rate limit's window that the line knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    FiveHour,
    SevenDay,
}

impl Window {
    fn of(kind: &str) -> Option<Self> {
        match kind {
            "five_hour" => Some(Self::FiveHour),
            "seven_day" => Some(Self::SevenDay),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5h",
            Self::SevenDay => "7d",
        }
    }
}

impl StatusLine {
    /// Notes what `event` says. Returns whether the line may read differently.
    pub fn event(&mut self, event: &Event) -> bool {
        match event {
            Event::Init(init) => noted(&mut self.init_model, &init.model, Kind::Id),
            Event::Assistant(answer) if answer.parent.is_none() && answer.model != SYNTHETIC => {
                let model = noted(&mut self.answered, &answer.model, Kind::Id);
                let context = answer.context_tokens.is_some_and(|tokens| {
                    let changed = self.context != Some(tokens);
                    self.context = Some(tokens);
                    changed
                });
                model || context
            }
            Event::Result(result) => {
                for (model, window) in &result.windows {
                    self.window_of(model, *window);
                }
                if let Some(cost) = result.cost_usd.filter(|cost| reasonable(*cost, MAX_COST)) {
                    self.cost = Some(cost);
                }
                true
            }
            Event::RateLimit(limit) => {
                let Some(window) = Window::of(&limit.kind) else {
                    return false;
                };
                let used = limit.utilization.map(|used| used * 100.0);
                let Some(used) = used.filter(|used| reasonable(*used, MAX_PERCENT)) else {
                    return false;
                };
                let changed = self.limit != Some((window, used));
                self.limit = Some((window, used));
                changed
            }
            _ => false,
        }
    }

    /// `initialize` listed `models`.
    pub fn initialized(&mut self, models: Vec<Model>) {
        self.models = models;
        self.listed += 1;
    }

    /// The models `initialize` listed, and how many times a list came (part 3c).
    pub fn models(&self) -> (&[Model], u64) {
        (&self.models, self.listed)
    }

    /// `get_usage` answered `usage`.
    pub fn usage_answered(&mut self, usage: Usage) {
        let percent = |used: Option<f64>| used.filter(|used| reasonable(*used, MAX_PERCENT));
        self.usage = [percent(usage.five_hour), percent(usage.seven_day)];
        self.asked_cost = usage
            .cost_usd
            .filter(|cost| *cost > 0.0 && reasonable(*cost, MAX_COST));
    }

    /// Notes `model`'s context window, replacing what an earlier `result` gave.
    fn window_of(&mut self, model: &str, window: u64) {
        if let Some(known) = self.windows.iter_mut().find(|(known, _)| known == model) {
            known.1 = window;
        } else if self.windows.len() < MAX_WINDOWS {
            self.windows.push((model.to_owned(), window));
        }
    }

    /// The model shown: the one that answered last, else `system/init`'s.
    pub fn model(&self) -> Option<&str> {
        self.answered.as_deref().or(self.init_model.as_deref())
    }

    /// The line's parts, joined by [`SEPARATOR`], into `out`, after what it holds, with `mode`
    /// as the permission mode, which `mode::ModeState` holds (part 3b), and `effort` as the
    /// effort level, which no stream field gives and `choice::Choice` notes (part 3c, the
    /// author's Q-C1), shown after the model: nothing for a part not known.
    pub fn write(&self, mode: Option<&str>, effort: Option<&str>, out: &mut String) {
        let part = |out: &mut String| {
            if !out.is_empty() {
                out.push_str(SEPARATOR);
            }
        };
        if let Some(model) = self.model() {
            part(out);
            out.push_str(&display_name(&self.models, model));
            if let Some(effort) = effort.filter(|e| readable(e, Kind::Word)) {
                out.push(' ');
                out.push_str(effort);
            }
        }
        if let Some(mode) = mode {
            part(out);
            out.push_str(mode);
        }
        if let Some(used) = self.context_used() {
            part(out);
            let _ = write!(out, "ctx {used:.0}%");
        }
        if let Some(cost) = self.cost.or(self.asked_cost) {
            part(out);
            out.push_str(&dollars(cost));
        }
        if let Some((window, used)) = self.window() {
            part(out);
            let _ = write!(out, "{} {used:.0}%", window.label());
        }
    }

    /// How much of the shown model's context window is used, in percent, once both are known.
    fn context_used(&self) -> Option<f64> {
        let used = self.context?;
        let model = self.model()?;
        let window = match self.windows.as_slice() {
            [(_, only)] => Some(*only),
            windows => (windows.iter())
                .find(|(known, _)| known == model)
                .map(|(_, window)| *window),
        };
        let window = window.filter(|window| *window > 0)?;
        #[expect(clippy::cast_precision_loss, reason = "a percentage's precision")]
        let percent = used as f64 * 100.0 / window as f64;
        Some(percent.min(MAX_PERCENT))
    }

    /// The window shown and its use: the last event's, else the more used of `get_usage`'s.
    fn window(&self) -> Option<(Window, f64)> {
        if self.limit.is_some() {
            return self.limit;
        }
        match self.usage {
            [Some(five), Some(seven)] if five > seven => Some((Window::FiveHour, five)),
            [_, Some(seven)] => Some((Window::SevenDay, seven)),
            [Some(five), None] => Some((Window::FiveHour, five)),
            [None, None] => None,
        }
    }
}

/// Notes `text` in `held`, a part of kind `kind`; returns whether that changed it. Empty, it
/// says nothing; given but not readable as itself, the part is no more shown, rather than
/// left saying what the CLI no more says.
fn noted(held: &mut Option<String>, text: &str, kind: Kind) -> bool {
    if text.is_empty() {
        return false;
    }
    let text = Some(text).filter(|text| readable(text, kind));
    if held.as_deref() == text {
        return false;
    }
    *held = text.map(str::to_owned);
    true
}

/// Whether `value` is a number from 0 to `max`, and not -0, which would show as `-0`.
fn reasonable(value: f64, max: f64) -> bool {
    value.is_finite() && value.is_sign_positive() && value <= max
}

/// The name model `id` is shown by: the name `models` lists for it, an alias's rather than
/// `default`'s, which names no model; else the id less `claude-`.
fn display_name(models: &[Model], id: &str) -> String {
    let named = |model: &&Model| model.value == id || model.resolved == id;
    let listed = (models.iter().filter(named))
        .min_by_key(|model| (model.value == "default", model.value != id));
    match listed {
        Some(model) => model.display.clone(),
        None => id.strip_prefix("claude-").unwrap_or(id).to_owned(),
    }
}

/// `cost` in US dollars: `$0.82`, and whole from $10, `$12`. A turn's row says its cost to the
/// hundredth of a cent (`render`); the line says it short, as a modeline's part.
fn dollars(cost: f64) -> String {
    if cost >= 9.995 {
        format!("${cost:.0}")
    } else {
        format!("${cost:.2}")
    }
}

#[cfg(test)]
mod tests;
