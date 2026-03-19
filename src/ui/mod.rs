use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::core::runtime::{App, MIN_HEIGHT, MIN_WIDTH, PeerStatus};

mod theme;
use theme::palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayoutMode {
    Wide,
    Medium,
    Narrow,
}

fn layout_mode(width: u16, height: u16) -> LayoutMode {
    if width < MIN_WIDTH || height < MIN_HEIGHT {
        LayoutMode::Narrow
    } else if width < 160 {
        LayoutMode::Medium
    } else {
        LayoutMode::Wide
    }
}

pub fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let p = palette(app.supports_truecolor);
    frame.render_widget(
        Block::default().style(Style::default().bg(p.bg).fg(p.tx)),
        frame.area(),
    );

    if layout_mode(frame.area().width, frame.area().height) == LayoutMode::Narrow {
        let msg = Paragraph::new("terminal too small — resize to 120×30")
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().fg(p.tx2));
        frame.render_widget(msg, centered_rect(frame.area(), 42, 3));
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(frame.area());

    render_title_bar(frame, rows[0], app, p);

    match layout_mode(frame.area().width, frame.area().height) {
        LayoutMode::Wide => {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(28),
                    Constraint::Min(30),
                    Constraint::Length(40),
                    Constraint::Length(24),
                ])
                .split(rows[1]);
            render_pet_column(frame, cols[0], app, p);
            render_chat_column(frame, cols[1], app, p);
            render_gossip_column(frame, cols[2], app, p);
            render_peers_column(frame, cols[3], app, p);
        }
        LayoutMode::Medium => {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(28),
                    Constraint::Min(30),
                    Constraint::Length(24),
                ])
                .split(rows[1]);
            render_pet_column(frame, cols[0], app, p);
            render_chat_column(frame, cols[1], app, p);
            render_peers_column(frame, cols[2], app, p);
        }
        LayoutMode::Narrow => {}
    }
}

fn render_title_bar(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    frame.render_widget(
        Block::default()
            .style(Style::default().bg(p.bg1))
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(p.ln)),
        area,
    );

    let parts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(8),
            Constraint::Min(0),
            Constraint::Length(5),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("● ", Style::default().fg(p.coral)),
            Span::styled("● ", Style::default().fg(p.amber)),
            Span::styled("●", Style::default().fg(p.green)),
        ]))
        .style(Style::default().bg(p.bg1)),
        parts[0],
    );
    frame.render_widget(
        Paragraph::new(format!(
            "critter · {} · @{}",
            app.pet_name(),
            app.user_name()
        ))
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(p.tx2).bg(p.bg1)),
        parts[1],
    );
    frame.render_widget(
        Paragraph::new(chrono::Local::now().format("%H:%M").to_string())
            .alignment(ratatui::layout::Alignment::Right)
            .style(Style::default().fg(p.tx3).bg(p.bg1)),
        parts[2],
    );
}

fn render_pet_column(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    frame.render_widget(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(p.ln)),
        area,
    );

    let sections = if app.show_debug_pane {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Length(5),
                Constraint::Length(8),
                Constraint::Min(4),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Length(5),
                Constraint::Min(8),
            ])
            .split(area)
    };

    let mut blob_lines: Vec<Line<'_>> = app
        .active_emotion_frame()
        .lines()
        .map(|s| Line::from(Span::styled(s.to_string(), Style::default().fg(p.green))))
        .collect();
    blob_lines.push(Line::from(Span::raw("")));
    blob_lines.push(Line::from(vec![
        Span::styled(format!("{}: ", app.pet_name()), Style::default().fg(p.tx2)),
        Span::styled(
            app.active_emotion_name().to_ascii_lowercase(),
            mood_style(app.active_emotion_color(), p),
        ),
    ]));

    frame.render_widget(
        Paragraph::new(blob_lines).alignment(ratatui::layout::Alignment::Center),
        sections[0],
    );

    render_stat_rows(frame, sections[1], app, p);
    render_os_signals(frame, sections[2], app, p);

    if app.show_debug_pane && sections.len() > 3 {
        render_debug_events(frame, sections[3], app, p);
    }
}

fn render_debug_events(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    let max = area.height as usize;
    let start = app.debug_events.len().saturating_sub(max);
    let lines: Vec<Line<'_>> = app.debug_events[start..]
        .iter()
        .map(|m| {
            Line::from(Span::styled(
                truncate_for_chat(m, area.width as usize),
                Style::default().fg(p.tx3),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_stat_rows(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
    render_stat_row(frame, rows[0], "H", app.hunger as usize, p.amber, p);
    render_stat_row(frame, rows[1], "E", app.energy as usize, p.sky, p);
    render_stat_row(frame, rows[2], "S", app.social as usize, p.rose, p);
    render_stat_row(frame, rows[3], "F", app.focus as usize, p.violet, p);
}

fn render_stat_row(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    label: &str,
    value: usize,
    base: Color,
    p: theme::Palette,
) {
    let bar_w = area.width.saturating_sub(7) as usize;
    let fill = ((value.min(100) * bar_w) / 100).min(bar_w);
    let empty = bar_w.saturating_sub(fill);
    let color = if value < 25 {
        p.coral
    } else if value > 75 {
        p.green
    } else {
        base
    };
    let line = Line::from(vec![
        Span::styled(format!("{label} "), Style::default().fg(base)),
        Span::styled("█".repeat(fill), Style::default().fg(color)),
        Span::styled("░".repeat(empty), Style::default().fg(p.bg3)),
        Span::styled(
            format!(" {:>3}", value.min(100)),
            Style::default().fg(color),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_os_signals(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    let snap = app.last_snapshot.as_ref();
    let wifi = snap
        .and_then(|s| s.wifi_rssi.map(|r| format!("{r} dBm")))
        .unwrap_or_else(|| "disconnected".to_string());
    let batt = snap
        .and_then(|s| {
            s.battery_pct
                .map(|v| format!("{v:.0}%{}", if s.charging { " ⚡" } else { "" }))
        })
        .unwrap_or_else(|| "--".to_string());
    let (cpu, cpu_dot) = match snap {
        Some(s) => {
            if let Some(temp_c) = s.cpu_temp_c {
                (format!("{temp_c:.0}°C"), signal_color_cpu(Some(temp_c), p))
            } else {
                let pct = s.cpu_pct.clamp(0.0, 100.0);
                (format!("{pct:.0}%"), signal_color_cpu_load(pct, p))
            }
        }
        None => ("--".to_string(), p.tx3),
    };
    let ram = snap
        .map(|s| format!("{:.0}%", s.mem_pct.clamp(0.0, 100.0)))
        .unwrap_or_else(|| "--".to_string());
    let app_name = snap
        .map(|s| truncate_for_chat(&s.active_app, area.width.saturating_sub(6) as usize))
        .unwrap_or_else(|| "-".to_string());
    let net = snap
        .map(|s| format!("{} kb/s", s.net_tx_kbps))
        .unwrap_or_else(|| "0 kb/s".to_string());
    let ssid = snap
        .and_then(|s| s.wifi_ssid.clone())
        .unwrap_or_else(|| "-".to_string());
    let ssid_short = truncate_for_chat(&ssid, area.width.saturating_sub(6) as usize);
    let idle = snap
        .map(|s| format_idle(s.idle_secs))
        .unwrap_or_else(|| "0s".to_string());

    let lines = vec![
        signal_line(
            "wifi",
            &wifi,
            signal_color_wifi(snap.and_then(|s| s.wifi_rssi), p),
            p,
        ),
        signal_line(
            "batt",
            &batt,
            signal_color_batt(snap.and_then(|s| s.battery_pct), p),
            p,
        ),
        signal_line("cpu", &cpu, cpu_dot, p),
        signal_line(
            "ram",
            &ram,
            signal_color_ram(snap.map(|s| s.mem_pct).unwrap_or(0.0), p),
            p,
        ),
        plain_signal_line("app", &app_name, p),
        plain_signal_line("net↑", &net, p),
        plain_signal_line("ssid", &ssid_short, p),
        plain_signal_line("idle", &idle, p),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_chat_column(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    frame.render_widget(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(p.ln)),
        area,
    );
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);
    render_tab_bar(frame, rows[0], app, p);

    let view_messages = app.active_tab_messages();
    let mut all_lines: Vec<Line<'_>> = Vec::new();
    for message in &view_messages {
        all_lines.extend(chat_lines(
            message,
            rows[1].width as usize,
            app.pet_name(),
            p,
        ));
    }
    let max_lines = rows[1].height as usize;
    let end = if app.chat_auto_scroll {
        all_lines.len()
    } else {
        all_lines.len().saturating_sub(app.chat_scroll)
    };
    let start = end.saturating_sub(max_lines);
    let lines: Vec<Line<'_>> = all_lines[start..end].to_vec();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), rows[1]);
    render_input(frame, rows[2], app, p);
}

fn render_tab_bar(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    let mut spans: Vec<Span<'_>> = Vec::new();
    for (idx, tab) in app.tabs.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(p.tx3)));
        }
        let mut title = if idx == 0 {
            format!("● {}", tab.label)
        } else {
            tab.label.clone()
        };
        if tab.unread > 0 && idx != app.active_tab {
            title.push_str(&format!(" [{}]", tab.unread));
        }
        let style = if idx == app.active_tab {
            Style::default()
                .fg(p.tx)
                .bg(p.bg1)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(p.tx2)
        };
        spans.push(Span::styled(title, style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(p.bg1)),
        area,
    );
}

fn render_input(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    let tab = app.tabs.get(app.active_tab);
    let prefix = tab.map(|t| t.prefix).unwrap_or('>');
    let placeholder = tab
        .map(|t| t.placeholder.as_str())
        .unwrap_or("message pet...");
    let line = if app.is_waiting_for_reply {
        format!("{prefix} {}", "thinking...")
    } else if app.input.is_empty() {
        format!("{prefix} {placeholder}")
    } else {
        format!("{prefix} {}", app.input)
    };
    let style = if app.is_waiting_for_reply {
        Style::default().fg(p.tx3)
    } else if app.input.is_empty() {
        Style::default().fg(p.tx2)
    } else {
        Style::default().fg(p.tx)
    };
    frame.render_widget(Paragraph::new(line).style(style), area);
    if app.is_waiting_for_reply {
        return;
    }
    let cursor_x = area
        .x
        .saturating_add(2)
        .saturating_add(app.input.chars().count() as u16);
    frame.set_cursor_position((cursor_x.min(area.x + area.width.saturating_sub(1)), area.y));
}

fn render_gossip_column(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    frame.render_widget(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(p.ln)),
        area,
    );
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);
    let live = if app.gossip_live && (app.frame_idx / 8).is_multiple_of(2) {
        Span::styled("live", Style::default().fg(p.violet))
    } else {
        Span::styled("live", Style::default().fg(p.tx3))
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("● pet gossip", Style::default().fg(p.violet)),
            Span::raw(" "),
            Span::raw(" ".repeat(rows[0].width.saturating_sub(16) as usize)),
            live,
        ])),
        rows[0],
    );

    let max_lines = rows[1].height as usize;
    let start = app.gossip_lines.len().saturating_sub(max_lines);
    let lines: Vec<Line<'_>> = app.gossip_lines[start..]
        .iter()
        .map(|m| render_gossip_line(m, app, p))
        .collect();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), rows[1]);

    let cool_total = 300_u32;
    let remain = app.gossip_rate_remaining_secs.min(cool_total as u64) as u32;
    let elapsed = cool_total.saturating_sub(remain);
    let bar_w = rows[2].width.saturating_sub(14) as usize;
    let fill = ((elapsed as usize) * bar_w) / cool_total as usize;
    let line = Line::from(vec![
        Span::styled("next msg ", Style::default().fg(p.tx2)),
        Span::styled("▓".repeat(fill), Style::default().fg(p.violet)),
        Span::styled(
            "░".repeat(bar_w.saturating_sub(fill)),
            Style::default().fg(p.bg3),
        ),
        Span::styled(format!(" {}s", remain), Style::default().fg(p.tx2)),
    ]);
    frame.render_widget(Paragraph::new(line), rows[2]);
}

fn render_gossip_line<'a>(line: &'a str, app: &App, p: theme::Palette) -> Line<'a> {
    if line.contains("began talking") {
        return Line::from(Span::styled(line, Style::default().fg(p.tx3)));
    }
    let Some((head, body)) = line.split_once(" | ") else {
        return Line::from(Span::styled(
            line,
            Style::default().fg(p.tx2).add_modifier(Modifier::ITALIC),
        ));
    };
    let Some((from, rest)) = head.split_once(" -> ") else {
        return Line::from(Span::styled(
            line,
            Style::default().fg(p.tx2).add_modifier(Modifier::ITALIC),
        ));
    };
    let (to, time) = if let Some((to_side, time_side)) = rest.rsplit_once(" [") {
        (to_side, time_side.trim_end_matches(']'))
    } else {
        (rest, "")
    };

    let from_color = if from.contains(app.pet_name()) {
        p.green
    } else {
        p.violet
    };
    let to_color = if to.contains(app.pet_name()) {
        p.green
    } else {
        p.sky
    };

    let mut spans = vec![
        Span::styled(from.to_string(), Style::default().fg(from_color)),
        Span::styled(" -> ", Style::default().fg(p.tx3)),
        Span::styled(to.to_string(), Style::default().fg(to_color)),
    ];
    if !time.is_empty() {
        spans.push(Span::styled(
            format!(" [{time}] "),
            Style::default().fg(p.tx3),
        ));
    } else {
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        body.to_string(),
        Style::default().fg(p.tx2).add_modifier(Modifier::ITALIC),
    ));
    Line::from(spans)
}

fn render_peers_column(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, p: theme::Palette) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(6),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("● ", Style::default().fg(p.green)),
            Span::styled("peers", Style::default().fg(p.tx)),
        ])),
        rows[0],
    );
    let mut peers: Vec<Line<'_>> = Vec::new();
    if let Some(node_id) = app.self_node_id.as_deref() {
        peers.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(p.sky)),
            Span::styled("you", Style::default().fg(p.tx)),
            Span::styled(
                format!(
                    " {}",
                    truncate_for_chat(node_id, rows[1].width.saturating_sub(8) as usize)
                ),
                Style::default().fg(p.tx3),
            ),
        ]));
        peers.push(Line::from(Span::styled(
            "  local node",
            Style::default().fg(p.tx3),
        )));
        peers.push(Line::from(""));
    }

    if app.peers.is_empty() {
        peers.push(Line::from(Span::styled(
            "no peers discovered yet",
            Style::default().fg(p.tx3),
        )));
    } else {
        for peer in &app.peers {
            let dot = match peer.status {
                PeerStatus::Online => p.green,
                PeerStatus::Away => p.amber,
                PeerStatus::Offline => p.tx3,
            };
            let mood = match peer.status {
                PeerStatus::Online => "online",
                PeerStatus::Away => "away",
                PeerStatus::Offline => "offline",
            };
            peers.push(Line::from(vec![
                Span::styled("● ", Style::default().fg(dot)),
                Span::styled(
                    truncate_for_chat(&peer.pet_name, rows[1].width.saturating_sub(12) as usize),
                    Style::default().fg(p.tx),
                ),
                Span::styled(format!("  {mood}"), Style::default().fg(p.tx3)),
            ]));
            peers.push(Line::from(Span::styled(
                format!(
                    "  {}",
                    truncate_for_chat(&peer.activity, rows[1].width.saturating_sub(2) as usize)
                ),
                Style::default().fg(p.tx2),
            )));
            peers.push(Line::from(Span::styled(
                format!(
                    "  {}",
                    truncate_for_chat(&peer.node_id, rows[1].width.saturating_sub(2) as usize)
                ),
                Style::default().fg(p.tx3),
            )));
            peers.push(Line::from(""));
        }
    }
    frame.render_widget(
        Paragraph::new(peers).style(Style::default().fg(p.tx2)),
        rows[1],
    );
    let cmds = vec![
        Line::from(vec![
            Span::styled("/feed", Style::default().fg(p.green)),
            Span::styled(" · ", Style::default().fg(p.tx3)),
            Span::styled("/sleep", Style::default().fg(p.green)),
            Span::styled(" · ", Style::default().fg(p.tx3)),
            Span::styled("/play", Style::default().fg(p.green)),
        ]),
        Line::from(vec![
            Span::styled("/poke", Style::default().fg(p.green)),
            Span::styled(" @name", Style::default().fg(p.tx2)),
        ]),
        Line::from(vec![
            Span::styled("/dm", Style::default().fg(p.green)),
            Span::styled(" @name", Style::default().fg(p.tx2)),
        ]),
        Line::from(vec![
            Span::styled("/friend", Style::default().fg(p.green)),
            Span::styled(" add/accept/list", Style::default().fg(p.tx2)),
        ]),
        Line::from(vec![
            Span::styled("/connect", Style::default().fg(p.green)),
            Span::styled(" <nodeid>", Style::default().fg(p.tx2)),
        ]),
        Line::from(vec![
            Span::styled("/group", Style::default().fg(p.green)),
            Span::styled(" #name", Style::default().fg(p.tx2)),
        ]),
        Line::from(vec![
            Span::styled("/gossip", Style::default().fg(p.green)),
            Span::styled(" · ", Style::default().fg(p.tx3)),
            Span::styled("/config", Style::default().fg(p.green)),
        ]),
        Line::from(vec![
            Span::styled("/setup", Style::default().fg(p.green)),
            Span::styled(" · ", Style::default().fg(p.tx3)),
            Span::styled("/help", Style::default().fg(p.green)),
        ]),
    ];
    frame.render_widget(Paragraph::new(cmds), rows[2]);
}

fn truncate_for_chat(message: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }

    let char_count = message.chars().count();
    if char_count <= max_cols {
        return message.to_string();
    }

    if max_cols <= 3 {
        return ".".repeat(max_cols);
    }

    let keep = max_cols - 3;
    let mut out = String::new();
    for c in message.chars().take(keep) {
        out.push(c);
    }
    out.push_str("...");
    out
}

fn mood_style(m: crate::pet::emotions::EmotionColor, p: theme::Palette) -> Style {
    let c = match m {
        crate::pet::emotions::EmotionColor::Green => p.green,
        crate::pet::emotions::EmotionColor::Amber => p.amber,
        crate::pet::emotions::EmotionColor::Coral => p.coral,
        crate::pet::emotions::EmotionColor::Sky => p.sky,
        crate::pet::emotions::EmotionColor::Violet => p.violet,
        crate::pet::emotions::EmotionColor::Rose => p.rose,
        crate::pet::emotions::EmotionColor::Teal => p.green,
        crate::pet::emotions::EmotionColor::Neutral => p.tx2,
    };
    Style::default().fg(c)
}

fn signal_line<'a>(key: &'a str, val: &'a str, dot: Color, p: theme::Palette) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{key:<4} "), Style::default().fg(p.tx2)),
        Span::styled(val.to_string(), Style::default().fg(p.tx)),
        Span::styled("  ●", Style::default().fg(dot)),
    ])
}

fn plain_signal_line<'a>(key: &'a str, val: &'a str, p: theme::Palette) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{key:<4} "), Style::default().fg(p.tx2)),
        Span::styled(val.to_string(), Style::default().fg(p.tx)),
    ])
}

fn signal_color_wifi(rssi: Option<i32>, p: theme::Palette) -> Color {
    match rssi {
        Some(v) if v > -65 => p.green,
        Some(v) if v >= -75 => p.amber,
        Some(_) => p.coral,
        None => p.coral,
    }
}

fn signal_color_batt(b: Option<f32>, p: theme::Palette) -> Color {
    match b {
        Some(v) if v > 50.0 => p.green,
        Some(v) if v >= 20.0 => p.amber,
        Some(_) => p.coral,
        None => p.tx3,
    }
}

fn signal_color_cpu(c: Option<f32>, p: theme::Palette) -> Color {
    match c {
        Some(v) if v < 70.0 => p.green,
        Some(v) if v <= 85.0 => p.amber,
        Some(_) => p.coral,
        None => p.tx3,
    }
}

fn signal_color_cpu_load(v: f32, p: theme::Palette) -> Color {
    if v < 65.0 {
        p.green
    } else if v <= 85.0 {
        p.amber
    } else {
        p.coral
    }
}

fn signal_color_ram(v: f32, p: theme::Palette) -> Color {
    if v < 70.0 {
        p.green
    } else if v <= 85.0 {
        p.amber
    } else {
        p.coral
    }
}

fn format_idle(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m {s}s")
    }
}

fn chat_lines(msg: &str, width: usize, pet_name: &str, p: theme::Palette) -> Vec<Line<'static>> {
    let style = chat_style(msg, pet_name, p);
    let wrapped = wrap_text(msg, width.saturating_sub(1).max(8));
    wrapped
        .into_iter()
        .map(|part| Line::from(Span::styled(part, style)))
        .collect()
}

fn chat_style(msg: &str, pet_name: &str, p: theme::Palette) -> Style {
    let lower = msg.to_ascii_lowercase();
    let pet_prefix = format!("{}:", pet_name.to_ascii_lowercase());
    if lower.starts_with(&pet_prefix) || lower.starts_with("blob") {
        Style::default().fg(p.green).add_modifier(Modifier::ITALIC)
    } else if lower.starts_with("you:") {
        Style::default().fg(p.sky)
    } else if lower.starts_with("system:") {
        Style::default().fg(p.tx3)
    } else {
        Style::default().fg(p.tx2)
    }
}

fn wrap_text(msg: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![msg.to_string()];
    }
    let mut out = Vec::new();
    for raw_line in msg.split('\n') {
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
                continue;
            }
            let next_len = current.chars().count() + 1 + word.chars().count();
            if next_len <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(current);
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            out.push(current);
        } else if raw_line.is_empty() {
            out.push(String::new());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(height)) / 2),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(width)) / 2),
            Constraint::Length(width.min(area.width)),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}
