//! The selected agent in full: identity, then TASK / NOW / RECENT.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{fmt_age, theme, wrap_to};
use crate::app::App;
use crate::summary::types::or_dash;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    let inner = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width - 1,
        height: area.height,
    };
    let width = inner.width as usize;

    let Some(agent) = app.selected_agent() else {
        frame.render_widget(
            Paragraph::new("Nothing selected.").style(theme::dim()),
            inner,
        );
        return;
    };

    let age = fmt_age(
        Instant::now().saturating_duration_since(agent.status_since),
        agent.age_is_lower_bound,
    );
    let repo = agent.repo.clone().unwrap_or_else(|| "ungrouped".into());

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(agent.label.clone(), theme::heading())),
        Line::from(Span::styled(
            format!(
                "{} · {} · {} · {} {}",
                agent.kind,
                agent.pane_id,
                repo,
                agent.status.as_str(),
                age
            ),
            theme::dim(),
        )),
        Line::from(Span::styled(
            abbreviate_home(&agent.cwd, width),
            theme::dim(),
        )),
        Line::from(""),
    ];

    let slot = app.slots.get(&agent.pane_id);
    let summary = slot.and_then(|s| s.summary.as_ref());

    // What it wants from you comes first — everything else is context.
    if let Some(reason) = app.attention_reason(agent) {
        lines.push(Line::from(Span::styled(
            "⚠ WAITING ON YOU",
            theme::alert().add_modifier(Modifier::BOLD),
        )));
        for l in wrap_to(reason, width, 3) {
            lines.push(Line::from(Span::styled(l, theme::alert())));
        }
        lines.push(Line::from(""));
    }

    // A failed refresh annotates the previous summary rather than replacing
    // it: stale detail is more useful than an empty pane.
    if let Some(err) = slot.and_then(|s| s.error.as_ref()) {
        lines.push(Line::from(Span::styled(
            "⚠ summary unavailable",
            theme::alert(),
        )));
        for l in wrap_to(err, width, 3) {
            lines.push(Line::from(Span::styled(l, theme::dim())));
        }
        if summary.is_some() {
            lines.push(Line::from(Span::styled(
                "showing the last successful summary",
                theme::dim(),
            )));
        }
        lines.push(Line::from(""));
    }

    if let Some(summary) = summary {
        // TASK carries a paragraph, NOW a few sentences — enough to pick the
        // thread back up after an hour away.
        section(
            &mut lines,
            "TASK",
            &[or_dash(&summary.task).to_string()],
            width,
            14,
        );
        section(
            &mut lines,
            "NOW",
            &[or_dash(&summary.now).to_string()],
            width,
            8,
        );
        let recent: Vec<String> = if summary.recent.is_empty() {
            vec![or_dash("").to_string()]
        } else {
            summary.recent.clone()
        };
        bullets(&mut lines, "RECENT", &recent, width);

        if let Some(state) = slot.map(|s| &s.state)
            && let Some(at) = state.generated_at
        {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!(
                    "summary {} ago · rev {}",
                    fmt_age(Instant::now().saturating_duration_since(at), false),
                    state.from_revision.unwrap_or(0)
                ),
                theme::dim(),
            )));
        }
    } else if !app.summaries_enabled() {
        for l in wrap_to(
            "Summaries are off. Set OPENROUTER_API_KEY or drop a key in ~/.openrouter-key.",
            width,
            3,
        ) {
            lines.push(Line::from(Span::styled(l, theme::dim())));
        }
    } else if slot.map(|s| s.state.in_flight).unwrap_or(false) {
        lines.push(Line::from(Span::styled("Summarising…", theme::dim())));
    } else if slot.and_then(|s| s.error.as_ref()).is_none() {
        for l in wrap_to(
            "No summary yet — one is generated as soon as this agent produces output.",
            width,
            3,
        ) {
            lines.push(Line::from(Span::styled(l, theme::dim())));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn section(
    lines: &mut Vec<Line<'static>>,
    title: &str,
    body: &[String],
    width: usize,
    max_lines: usize,
) {
    lines.push(Line::from(Span::styled(
        title.to_string(),
        theme::heading(),
    )));
    for para in body {
        for l in wrap_to(para, width, max_lines) {
            lines.push(Line::from(Span::styled(l, theme::label())));
        }
    }
    lines.push(Line::from(""));
}

fn bullets(lines: &mut Vec<Line<'static>>, title: &str, items: &[String], width: usize) {
    lines.push(Line::from(Span::styled(
        title.to_string(),
        theme::heading(),
    )));
    for item in items {
        for (i, l) in wrap_to(item, width.saturating_sub(2), 3)
            .into_iter()
            .enumerate()
        {
            let prefix = if i == 0 { "· " } else { "  " };
            lines.push(Line::from(Span::styled(
                format!("{prefix}{l}"),
                theme::label(),
            )));
        }
    }
}

/// Replace the home prefix with `~`, then middle-elide if still too wide.
fn abbreviate_home(path: &str, width: usize) -> String {
    let home = crate::config::home_dir();
    let shortened = match path.strip_prefix(home.to_string_lossy().as_ref()) {
        Some(rest) => format!("~{rest}"),
        None => path.to_string(),
    };
    if shortened.chars().count() <= width || width == 0 {
        return shortened;
    }
    let keep = width.saturating_sub(1);
    let tail: String = shortened
        .chars()
        .skip(shortened.chars().count() - keep)
        .collect();
    format!("…{tail}")
}
