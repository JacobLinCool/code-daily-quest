use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use code_daily_quest_core::{
    DailyQuest, DoctorReport, HistoryDay, QuestDifficulty, TodayView, Tracker,
};
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Tabs, Wrap,
};
use ratatui::{Frame, Terminal};

const DB_POLL_INTERVAL: Duration = Duration::from_secs(1);
const SERVICE_STATUS_REFRESH: Duration = Duration::from_secs(30);
const VIEW_HEARTBEAT: Duration = Duration::from_secs(15);
const QUEST_BAR_WIDTH: usize = 20;

// ── Theme ─────────────────────────────────────────────────────────
const C_BORDER: Color = Color::Rgb(58, 65, 82);
const C_DIM: Color = Color::Rgb(90, 100, 120);
const C_ACCENT: Color = Color::Rgb(110, 175, 255);
const C_SUCCESS: Color = Color::Rgb(80, 200, 120);
const C_WARNING: Color = Color::Rgb(255, 200, 60);
const C_DANGER: Color = Color::Rgb(255, 85, 85);
const C_STREAK: Color = Color::Rgb(255, 160, 50);
const C_BAR_EMPTY: Color = Color::Rgb(45, 50, 62);
const C_TOOL_CODEX: Color = Color::Rgb(100, 180, 255);
const C_TOOL_CLAUDE: Color = Color::Rgb(190, 130, 255);

pub fn run_tui(mut tracker: Tracker) -> Result<()> {
    let mut session = TuiSession::enter()?;
    run_event_loop(session.terminal_mut(), &mut tracker)
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    tracker: &mut Tracker,
) -> Result<()> {
    let mut selected_tab = 0usize;
    let mut selected_history = 0usize;
    let mut last_data_refresh = Instant::now() - VIEW_HEARTBEAT;
    let mut last_db_check = Instant::now();
    let mut last_service_refresh = Instant::now();
    let mut last_db_token = database_change_token(tracker.db_path())?;
    let mut service_status = tracker.service_status()?;
    let mut today: Option<TodayView> = None;
    let mut history: Vec<HistoryDay> = Vec::new();
    let mut doctor: Option<DoctorReport> = None;
    let mut dirty = true;

    loop {
        if dirty {
            if today.is_none() || last_data_refresh.elapsed() >= VIEW_HEARTBEAT {
                today = Some(tracker.today_view_with_service_status(service_status.clone())?);
                history = tracker.history_days(30)?;
                doctor = Some(tracker.doctor_snapshot()?);
                if selected_history >= history.len() && !history.is_empty() {
                    selected_history = history.len() - 1;
                }
                last_data_refresh = Instant::now();
            }

            terminal.draw(|frame| {
                draw_ui(
                    frame,
                    selected_tab,
                    selected_history,
                    today.as_ref(),
                    &history,
                    doctor.as_ref(),
                );
            })?;
            dirty = false;
        }

        let now = Instant::now();
        let next_deadline = [
            last_db_check + DB_POLL_INTERVAL,
            last_data_refresh + VIEW_HEARTBEAT,
            last_service_refresh + SERVICE_STATUS_REFRESH,
        ]
        .into_iter()
        .min()
        .unwrap_or(now + Duration::from_millis(250));
        let poll_timeout = next_deadline.saturating_duration_since(now);

        if event::poll(poll_timeout)?
            && let Event::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('r') if selected_tab == 2 => {
                    doctor = Some(tracker.doctor_rescan()?);
                    dirty = true;
                }
                KeyCode::Tab | KeyCode::Right => {
                    selected_tab = (selected_tab + 1) % 3;
                    dirty = true;
                }
                KeyCode::BackTab | KeyCode::Left => {
                    selected_tab = selected_tab.checked_sub(1).unwrap_or(2);
                    dirty = true;
                }
                KeyCode::Down if selected_tab == 1 && !history.is_empty() => {
                    let next = (selected_history + 1).min(history.len() - 1);
                    if next != selected_history {
                        selected_history = next;
                        dirty = true;
                    }
                }
                KeyCode::Up if selected_tab == 1 && !history.is_empty() => {
                    let next = selected_history.saturating_sub(1);
                    if next != selected_history {
                        selected_history = next;
                        dirty = true;
                    }
                }
                _ => {}
            }
            continue;
        }

        if last_db_check.elapsed() >= DB_POLL_INTERVAL {
            let db_token = database_change_token(tracker.db_path())?;
            last_db_check = Instant::now();
            if db_token != last_db_token {
                today = Some(tracker.today_view_with_service_status(service_status.clone())?);
                history = tracker.history_days(30)?;
                doctor = Some(tracker.doctor_snapshot()?);
                if selected_history >= history.len() && !history.is_empty() {
                    selected_history = history.len() - 1;
                }
                last_db_token = db_token;
                last_data_refresh = Instant::now();
                dirty = true;
            }
        }

        if last_service_refresh.elapsed() >= SERVICE_STATUS_REFRESH {
            service_status = tracker.service_status()?;
            last_service_refresh = Instant::now();
            if let Some(today_view) = &mut today {
                today_view.service_status = service_status.clone();
                dirty = true;
            }
        }

        if last_data_refresh.elapsed() >= VIEW_HEARTBEAT {
            today = Some(tracker.today_view_with_service_status(service_status.clone())?);
            history = tracker.history_days(30)?;
            doctor = Some(tracker.doctor_snapshot()?);
            if selected_history >= history.len() && !history.is_empty() {
                selected_history = history.len() - 1;
            }
            last_data_refresh = Instant::now();
            dirty = true;
        }
    }

    Ok(())
}

fn database_change_token(db_path: &Path) -> Result<u128> {
    let mut token = 0_u128;
    for candidate in [db_path.to_path_buf(), wal_path(db_path)] {
        token = token.max(path_change_token(&candidate)?);
    }
    Ok(token)
}

fn wal_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", db_path.to_string_lossy()))
}

fn path_change_token(path: &Path) -> Result<u128> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(0);
    };
    let modified = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(modified.saturating_add(metadata.len() as u128))
}

// ── Layout helpers ────────────────────────────────────────────────

fn themed_block<'a>() -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(C_BORDER))
        .padding(Padding::horizontal(1))
}

// ── Root UI ───────────────────────────────────────────────────────

fn draw_ui(
    frame: &mut Frame<'_>,
    selected_tab: usize,
    selected_history: usize,
    today: Option<&TodayView>,
    history: &[HistoryDay],
    doctor: Option<&DoctorReport>,
) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    // ── Tab bar ─────────────────────────────────────────────
    let titles: Vec<Line> = [" Today ", " History ", " Diagnostics "]
        .into_iter()
        .map(|t| Line::from(Span::raw(t)))
        .collect();
    let tabs = Tabs::new(titles)
        .select(selected_tab)
        .block(
            Block::default()
                .title(Span::styled(
                    " Code Daily Quest ",
                    Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(C_BORDER)),
        )
        .style(Style::default().fg(C_DIM))
        .highlight_style(Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD))
        .divider(Span::styled("│", Style::default().fg(C_BORDER)));
    frame.render_widget(tabs, layout[0]);

    // ── Content ─────────────────────────────────────────────
    match selected_tab {
        0 => draw_today(frame, layout[1], today),
        1 => draw_history(frame, layout[1], history, selected_history),
        _ => draw_diagnostics(frame, layout[1], doctor),
    }

    // ── Footer ──────────────────────────────────────────────
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("q ", Style::default().fg(C_ACCENT)),
        Span::styled("quit   ", Style::default().fg(C_DIM)),
        Span::styled("←→ ", Style::default().fg(C_ACCENT)),
        Span::styled("tabs   ", Style::default().fg(C_DIM)),
        Span::styled("↑↓ ", Style::default().fg(C_ACCENT)),
        Span::styled("history   ", Style::default().fg(C_DIM)),
        Span::styled("r ", Style::default().fg(C_ACCENT)),
        Span::styled("refresh", Style::default().fg(C_DIM)),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(footer, layout[2]);
}

// ── Today tab ─────────────────────────────────────────────────────

fn draw_today(frame: &mut Frame<'_>, area: Rect, today: Option<&TodayView>) {
    let Some(today) = today else {
        frame.render_widget(Clear, area);
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // ── Overview bar ────────────────────────────────────────
    let (check, check_color) = if today.record.all_completed {
        ("✓", C_SUCCESS)
    } else {
        ("○", C_WARNING)
    };
    let overview = Paragraph::new(Line::from(vec![
        Span::styled(
            today.today.to_string(),
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  │  ", Style::default().fg(C_BORDER)),
        Span::styled(format!("{check} "), Style::default().fg(check_color)),
        Span::styled(
            format!(
                "{}/{}",
                today.record.completed_quests, today.record.total_quests
            ),
            completion_style(today.record.all_completed),
        ),
        Span::styled("  │  ", Style::default().fg(C_BORDER)),
        Span::styled(
            format!("Streak {}", today.record.closing_streak),
            Style::default().fg(C_STREAK).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  │  ", Style::default().fg(C_BORDER)),
        Span::styled(
            today.service_status.clone(),
            service_status_style(&today.service_status),
        ),
    ]))
    .block(themed_block().title(Line::from(Span::styled(
        " Overview ",
        Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
    ))));
    frame.render_widget(overview, chunks[0]);

    // ── Quest list ──────────────────────────────────────────
    let quest_items: Vec<ListItem> = today.quests.iter().map(render_today_quest).collect();
    let title = if today.record.all_completed {
        Line::from(vec![
            Span::styled(
                " Quests ",
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("── ", Style::default().fg(C_BORDER)),
            Span::styled(
                " ✓ ALL CLEAR ",
                Style::default()
                    .fg(Color::Black)
                    .bg(C_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(Span::styled(
            " Quests ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ))
    };
    let list = List::new(quest_items).block(themed_block().title(title));
    frame.render_widget(list, chunks[1]);
}

// ── History tab ───────────────────────────────────────────────────

fn draw_history(
    frame: &mut Frame<'_>,
    area: Rect,
    history: &[HistoryDay],
    selected_history: usize,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    // ── Day list ────────────────────────────────────────────
    let items: Vec<ListItem> = history.iter().map(render_history_day_item).collect();
    let mut state = ListState::default();
    if !history.is_empty() {
        state.select(Some(selected_history));
    }
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 45, 60))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ")
        .block(themed_block().title(Line::from(Span::styled(
            " Days ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ))));
    frame.render_stateful_widget(list, chunks[0], &mut state);

    // ── Details panel ───────────────────────────────────────
    let details = history
        .get(selected_history)
        .map(render_history_detail)
        .unwrap_or_else(|| {
            vec![Line::from(Span::styled(
                "No history",
                Style::default().fg(C_DIM),
            ))]
        });
    let paragraph = Paragraph::new(details)
        .block(themed_block().title(Line::from(Span::styled(
            " Details ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ))))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, chunks[1]);
}

fn render_history_detail(day: &HistoryDay) -> Vec<Line<'static>> {
    let (check, check_color) = if day.record.all_completed {
        ("✓", C_SUCCESS)
    } else {
        ("○", C_WARNING)
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                day.record.day.to_string(),
                Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  │  ", Style::default().fg(C_BORDER)),
            Span::styled(format!("{check} "), Style::default().fg(check_color)),
            Span::styled(
                format!(
                    "{}/{}",
                    day.record.completed_quests, day.record.total_quests
                ),
                completion_style(day.record.all_completed),
            ),
            Span::styled("  │  ", Style::default().fg(C_BORDER)),
            Span::styled(
                format!("Streak {}", day.record.closing_streak),
                Style::default().fg(C_STREAK).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];
    for quest in &day.quests {
        lines.extend(render_quest_lines(quest));
        lines.push(Line::from(""));
    }
    lines
}

// ── Diagnostics tab ───────────────────────────────────────────────

fn draw_diagnostics(frame: &mut Frame<'_>, area: Rect, doctor: Option<&DoctorReport>) {
    let Some(doctor) = doctor else {
        frame.render_widget(Clear, area);
        return;
    };

    let label = Style::default().fg(C_DIM);
    let value = Style::default().fg(Color::Rgb(200, 206, 218));

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("Database   ", label),
            Span::styled(doctor.diagnostics.database_path.clone(), value),
        ]),
        Line::from(vec![
            Span::styled("Events     ", label),
            Span::styled(doctor.diagnostics.event_count.to_string(), value),
        ]),
        Line::from(vec![
            Span::styled("Checks     ", label),
            Span::styled(doctor.diagnostics.checkpoint_count.to_string(), value),
        ]),
        Line::from(vec![
            Span::styled("Last sync  ", label),
            Span::styled(
                doctor
                    .diagnostics
                    .last_sync_at
                    .map(|ts| ts.to_rfc3339())
                    .unwrap_or_else(|| "never".to_string()),
                value,
            ),
        ]),
        Line::from(vec![
            Span::styled("Notifier   ", label),
            Span::styled(
                if doctor.notifier_supported {
                    "supported"
                } else {
                    "not supported"
                },
                if doctor.notifier_supported {
                    Style::default().fg(C_SUCCESS)
                } else {
                    Style::default().fg(C_DIM)
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("Service    ", label),
            Span::styled(
                if doctor.service_supported {
                    "supported"
                } else {
                    "not supported"
                },
                if doctor.service_supported {
                    Style::default().fg(C_SUCCESS)
                } else {
                    Style::default().fg(C_DIM)
                },
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "── Adapters ──",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for adapter in &doctor.diagnostics.adapter_sources {
        lines.push(Line::from(vec![
            Span::styled(
                adapter.tool_id.clone(),
                tool_style(&adapter.tool_id).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {} files", adapter.discovered_files),
                Style::default().fg(C_DIM),
            ),
        ]));
        if let Some(error) = &adapter.discovery_error {
            lines.push(Line::from(Span::styled(
                format!("  ! {error}"),
                Style::default().fg(C_DANGER),
            )));
        }
        for root in &adapter.roots {
            lines.push(Line::from(Span::styled(
                format!("  {root}"),
                Style::default().fg(C_DIM),
            )));
        }
        lines.push(Line::from(""));
    }

    let paragraph = Paragraph::new(lines)
        .block(themed_block().title(Line::from(Span::styled(
            " Diagnostics ",
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ))))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

// ── Quest rendering ───────────────────────────────────────────────

fn render_today_quest(quest: &DailyQuest) -> ListItem<'static> {
    let mut lines = render_quest_lines(quest);
    lines.push(Line::from(""));
    ListItem::new(lines)
}

fn render_quest_lines(quest: &DailyQuest) -> Vec<Line<'static>> {
    let (status_icon, status_color) = if quest.is_completed() {
        ("✓", C_SUCCESS)
    } else {
        ("○", C_WARNING)
    };
    let progress_color = if quest.is_completed() {
        C_SUCCESS
    } else {
        C_ACCENT
    };

    // Line 1: status icon + difficulty badge + quest name
    let line1 = Line::from(vec![
        Span::styled(format!("{status_icon} "), Style::default().fg(status_color)),
        Span::styled(
            format!(" {} ", quest.difficulty.label().to_uppercase()),
            difficulty_style(quest.difficulty),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            quest.kind.label(),
            Style::default()
                .fg(Color::Rgb(200, 206, 218))
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    // Line 2: progress numbers + progress bar + percentage
    let mut line2_spans = vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            format_count(quest.progress_total),
            Style::default()
                .fg(progress_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / ", Style::default().fg(C_DIM)),
        Span::styled(format_count(quest.threshold), Style::default().fg(C_DIM)),
        Span::styled(
            format!(" {}  ", quest.kind.unit_label()),
            Style::default().fg(C_DIM),
        ),
    ];
    line2_spans.extend(progress_bar_spans(
        quest.progress_total,
        quest.threshold,
        QUEST_BAR_WIDTH,
    ));
    let line2 = Line::from(line2_spans);

    // Line 3: tool breakdown
    let line3 = render_tool_breakdown(quest);

    vec![line1, line2, line3]
}

fn render_tool_breakdown(quest: &DailyQuest) -> Line<'static> {
    if quest.progress_by_tool.is_empty() {
        return Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("no contributions yet", Style::default().fg(C_DIM)),
        ]);
    }

    let mut spans = vec![Span::styled("  ", Style::default())];
    for (index, (tool, progress)) in quest.progress_by_tool.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(C_DIM)));
        }
        spans.push(Span::styled(
            tool.clone(),
            tool_style(tool).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format_count(*progress),
            Style::default().fg(C_DIM),
        ));
    }
    Line::from(spans)
}

fn render_history_day_item(day: &HistoryDay) -> ListItem<'static> {
    let (icon, icon_color) = if day.record.all_completed {
        ("✓", C_SUCCESS)
    } else {
        ("○", C_WARNING)
    };
    ListItem::new(Line::from(vec![
        Span::styled(format!("{icon} "), Style::default().fg(icon_color)),
        Span::styled(day.record.day.to_string(), Style::default().fg(C_ACCENT)),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!(
                "{}/{}",
                day.record.completed_quests, day.record.total_quests
            ),
            completion_style(day.record.all_completed),
        ),
        Span::styled("  streak ", Style::default().fg(C_DIM)),
        Span::styled(
            day.record.closing_streak.to_string(),
            Style::default().fg(C_STREAK),
        ),
    ]))
}

// ── Progress bar ──────────────────────────────────────────────────

fn progress_bar_spans(progress: u64, threshold: u64, width: usize) -> Vec<Span<'static>> {
    if width == 0 {
        return vec![];
    }
    let filled = if threshold == 0 {
        width
    } else {
        ((progress.min(threshold) as f64 / threshold as f64) * width as f64).round() as usize
    }
    .min(width);
    let empty = width.saturating_sub(filled);
    let pct = if threshold == 0 {
        100
    } else {
        ((progress.min(threshold) as f64 / threshold as f64) * 100.0).round() as u64
    };
    let bar_color = if progress >= threshold {
        C_SUCCESS
    } else {
        C_ACCENT
    };

    vec![
        Span::styled("█".repeat(filled), Style::default().fg(bar_color)),
        Span::styled("░".repeat(empty), Style::default().fg(C_BAR_EMPTY)),
        Span::styled(format!(" {pct}%"), Style::default().fg(C_DIM)),
    ]
}

// ── Style helpers ─────────────────────────────────────────────────

fn difficulty_style(difficulty: QuestDifficulty) -> Style {
    match difficulty {
        QuestDifficulty::Easy => Style::default()
            .fg(Color::Black)
            .bg(C_SUCCESS)
            .add_modifier(Modifier::BOLD),
        QuestDifficulty::Normal => Style::default()
            .fg(Color::Black)
            .bg(C_WARNING)
            .add_modifier(Modifier::BOLD),
        QuestDifficulty::Hard => Style::default()
            .fg(Color::White)
            .bg(C_DANGER)
            .add_modifier(Modifier::BOLD),
    }
}

fn completion_style(completed: bool) -> Style {
    if completed {
        Style::default().fg(C_SUCCESS).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_WARNING).add_modifier(Modifier::BOLD)
    }
}

fn service_status_style(status: &str) -> Style {
    if status.contains("loaded") {
        Style::default().fg(C_SUCCESS).add_modifier(Modifier::BOLD)
    } else if status.contains("installed") {
        Style::default().fg(C_WARNING)
    } else {
        Style::default().fg(C_DIM)
    }
}

fn tool_style(tool: &str) -> Style {
    match tool {
        "codex" => Style::default().fg(C_TOOL_CODEX),
        "claude-code" => Style::default().fg(C_TOOL_CLAUDE),
        _ => Style::default().fg(C_ACCENT),
    }
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(ch);
    }
    formatted.chars().rev().collect()
}

// ── Terminal session ──────────────────────────────────────────────

struct TuiSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TuiSession {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;
        Ok(Self { terminal })
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TuiSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}
