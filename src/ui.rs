use crate::app::{App, NoteSource, Screen};
use crate::markdown;
use crate::tree::NodeKind;
use ratatui::prelude::*;
use ratatui::widgets::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

pub fn draw(f: &mut Frame, app: &mut App) {
    match &mut app.screen {
        Screen::SourceSelect { selected, options, error_message } => {
            draw_source_select(f, *selected, options, error_message.as_deref());
        }
        Screen::FolderSelect {
            current_path,
            entries,
            selected,
            scroll_offset,
            message,
            found_vaults,
            throbber_tick,
            vault_selected,
            focus_folders,
            scan_progress,
            ..
        } => {
            draw_folder_select(
                f,
                &current_path.to_string_lossy(),
                entries,
                *selected,
                *scroll_offset,
                message.as_deref(),
                found_vaults.as_deref(),
                *throbber_tick,
                *vault_selected,
                *focus_folders,
                scan_progress,
            );
        }
        Screen::NotesBrowser {
            source,
            flat_items,
            selected,
            tree_scroll,
            note_content,
            note_scroll,
            stats,
            focus_tree,
            integrity_popup,
            config_popup,
            error_message,
            ..
        } => {
            draw_notes_browser(
                f,
                source,
                flat_items,
                *selected,
                tree_scroll,
                note_content.as_deref(),
                note_scroll,
                stats,
                *focus_tree,
                integrity_popup,
                config_popup,
                error_message.as_deref(),
            );
        }
        Screen::ExportVaultSelect {
            vaults,
            selected,
            scroll_offset,
            new_name_input,
            ..
        } => {
            draw_export_vault_select(
                f,
                vaults.as_deref(),
                *selected,
                *scroll_offset,
                new_name_input.as_deref(),
            );
        }
        Screen::ExportFolderSelect {
            current_path,
            entries,
            selected,
            scroll_offset,
            new_name_input,
            ..
        } => {
            draw_export_folder_select(
                f,
                &current_path.to_string_lossy(),
                entries,
                *selected,
                *scroll_offset,
                new_name_input.as_deref(),
            );
        }
        Screen::ExportResults { shared_log, scroll } => {
            let log = shared_log.lock().unwrap();
            draw_export_results(f, &log, *scroll);
        }
    }
}

fn draw_source_select(
    f: &mut Frame,
    selected: usize,
    options: &[(&str, NoteSource)],
    error_message: Option<&str>,
) {
    let area = f.area();

    // Background
    let block = Block::default()
        .title(" Notes Inspector ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(block, area);

    // Center the menu
    let has_error = error_message.is_some();
    let options_chunk_height = options.len() as u16 + 3; // +3 for border (2) + padding (1)
    let menu_height = 2 + 2 + options_chunk_height + if has_error { 3 } else { 0 } + 2;
    let menu_width: u16 = 30;

    // Center vertically with absolute height, horizontally with percentage
    let v_center = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(menu_height),
            Constraint::Fill(1),
        ])
        .split(area);
    let menu_area = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - menu_width) / 2),
            Constraint::Percentage(menu_width),
            Constraint::Percentage((100 - menu_width) / 2),
        ])
        .split(v_center[1])[1];

    let mut constraints = vec![
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(options_chunk_height),
    ];
    if has_error {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Length(2));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(menu_area);

    // Title
    let title = Paragraph::new("Select a notes source:")
        .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center);
    f.render_widget(title, chunks[0]);

    // Options
    let items: Vec<ListItem> = options
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let style = if i == selected {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if i == selected { " ► " } else { "   " };
            ListItem::new(format!("{prefix}{name}")).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Cyan)),
        );
    f.render_widget(list, chunks[2]);

    // Error message (if any)
    if let Some(err) = error_message {
        let err_idx = 3;
        let err_widget = Paragraph::new(err.to_string())
            .style(Style::default().fg(Color::Red))
            .alignment(Alignment::Center)
            .wrap(ratatui::widgets::Wrap { trim: true });
        f.render_widget(err_widget, chunks[err_idx]);
    }

    // Help text (last chunk)
    let help_idx = chunks.len() - 1;
    let help = Paragraph::new("↑↓/jk: Navigate  Enter: Select  q: Quit")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(help, chunks[help_idx]);
}

#[allow(clippy::too_many_arguments)]
fn draw_folder_select(
    f: &mut Frame,
    current_path: &str,
    entries: &[std::path::PathBuf],
    selected: usize,
    scroll_offset: usize,
    message: Option<&str>,
    found_vaults: Option<&[std::path::PathBuf]>,
    throbber_tick: usize,
    vault_selected: usize,
    focus_folders: bool,
    scan_progress: &crate::obsidian::SharedScanProgress,
) {
    let area = f.area();

    // Determine vault discovery frame height
    let vault_frame_height: u16 = match found_vaults {
        None => 3,      // "Searching..." single line
        Some(v) if v.is_empty() => 3, // "No vaults found"
        Some(v) => (v.len() as u16 + 2).min(8), // vaults + border, capped
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(vault_frame_height),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    // Vault discovery frame
    let vault_border_color = if !focus_folders && found_vaults.is_some_and(|v| !v.is_empty()) {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    match found_vaults {
        None => {
            // Still scanning - show throbber with live progress
            let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let spinner = spinner_chars[throbber_tick % spinner_chars.len()];
            let (folders_searched, scan_path) = {
                let prog = scan_progress.lock().unwrap();
                (prog.folders_searched, prog.current_path.clone())
            };
            let vault_block = Block::default()
                .title(" Obsidian Vaults ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Yellow));
            // Truncate path to fit in available width
            let inner_width = chunks[0].width.saturating_sub(2) as usize;
            let prefix = format!(" {spinner} Searching ({folders_searched}) ");
            let max_path_len = inner_width.saturating_sub(prefix.len());
            let display_path = if scan_path.len() > max_path_len {
                let start = scan_path.len() - max_path_len.saturating_sub(1);
                format!("…{}", &scan_path[start..])
            } else {
                scan_path
            };
            let searching = Paragraph::new(format!("{prefix}{display_path}"))
                .style(Style::default().fg(Color::Yellow))
                .block(vault_block);
            f.render_widget(searching, chunks[0]);
        }
        Some(vaults) if vaults.is_empty() => {
            let vault_block = Block::default()
                .title(" Obsidian Vaults ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::DarkGray));
            let none_found = Paragraph::new(" No vaults found")
                .style(Style::default().fg(Color::DarkGray))
                .block(vault_block);
            f.render_widget(none_found, chunks[0]);
        }
        Some(vaults) => {
            let vault_block = Block::default()
                .title(format!(" {} Obsidian Vault{} Found ",
                    vaults.len(),
                    if vaults.len() == 1 { "" } else { "s" }))
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(vault_border_color));
            let inner_height = vault_frame_height.saturating_sub(2) as usize;
            let vault_scroll = if vault_selected >= inner_height {
                vault_selected - inner_height + 1
            } else {
                0
            };
            let items: Vec<ListItem> = vaults
                .iter()
                .enumerate()
                .skip(vault_scroll)
                .take(inner_height)
                .map(|(i, path)| {
                    let name = path.to_string_lossy().to_string();
                    let style = if i == vault_selected && !focus_folders {
                        Style::default()
                            .fg(Color::White)
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Green)
                    };
                    ListItem::new(format!(" ♦ {name}")).style(style)
                })
                .collect();
            let list = List::new(items).block(vault_block);
            f.render_widget(list, chunks[0]);
        }
    }

    // Path header
    let path_border_color = if focus_folders { Color::Cyan } else { Color::DarkGray };
    let path_block = Block::default()
        .title(" Select Obsidian Vault ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(path_border_color));
    let path_text = Paragraph::new(current_path)
        .style(Style::default().fg(Color::Yellow))
        .block(path_block);
    f.render_widget(path_text, chunks[1]);

    // Directory list
    let visible_height = chunks[2].height.saturating_sub(2) as usize;
    let scroll = if selected >= scroll_offset + visible_height {
        selected.saturating_sub(visible_height) + 1
    } else {
        scroll_offset
    };

    let items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, path)| {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_vault = crate::obsidian::is_obsidian_vault(path);
            let icon = if is_vault { "♦ " } else { "▪ " };
            let style = if i == selected && focus_folders {
                Style::default()
                    .fg(Color::Black)
                    .bg(if is_vault { Color::Green } else { Color::Cyan })
                    .add_modifier(Modifier::BOLD)
            } else if is_vault {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(format!(" {icon}{name}")).style(style)
        })
        .collect();

    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focus_folders { Color::DarkGray } else { Color::Rgb(40, 40, 40) }))
        .title(if let Some(msg) = message {
            format!(" {msg} ")
        } else {
            format!(" {} items ", entries.len())
        });
    let list = List::new(items).block(list_block);
    f.render_widget(list, chunks[2]);

    // Help footer
    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let help_text = if found_vaults.is_some_and(|v| !v.is_empty()) {
        "↑↓/jk: Navigate  Enter: Open  Tab: Switch pane  Backspace/←: Parent  Esc: Back"
    } else {
        "↑↓/jk: Navigate  Enter: Open  Backspace/←: Parent  Esc: Back  Green = Vault"
    };
    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(help_block);
    f.render_widget(help, chunks[3]);
}

#[allow(clippy::too_many_arguments)]
fn draw_notes_browser(
    f: &mut Frame,
    source: &NoteSource,
    flat_items: &[crate::tree::FlatItem],
    selected: usize,
    tree_scroll: &mut usize,
    note_content: Option<&str>,
    note_scroll: &mut usize,
    stats: &crate::app::NoteStats,
    focus_tree: bool,
    integrity_popup: &mut Option<crate::app::IntegrityPopup>,
    config_popup: &mut Option<crate::app::ConfigPopup>,
    error_message: Option<&str>,
) {
    let area = f.area();

    // Main layout: stats bar on top, content below, footer at bottom
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    // Stats bar
    draw_stats_bar(f, main_chunks[0], stats);

    // Content: tree on left, preview on right
    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(main_chunks[1]);

    // Tree view
    draw_tree(f, content_chunks[0], flat_items, selected, tree_scroll, focus_tree);

    // Note preview — pass selected item for metadata
    let selected_item = flat_items.get(selected);
    draw_note_preview(f, content_chunks[1], note_content, note_scroll, !focus_tree, selected_item);

    // Footer
    draw_footer(f, main_chunks[2], source, focus_tree, error_message);

    // Integrity check popup overlay
    if let Some(popup) = integrity_popup {
        draw_integrity_popup(f, area, popup);
    }


    // Config viewer popup overlay
    if let Some(popup) = config_popup {
        draw_config_popup(f, area, popup);
    }
}

fn draw_stats_bar(f: &mut Frame, area: Rect, stats: &crate::app::NoteStats) {
    let block = Block::default()
        .title(format!(" {} ", stats.vault_name))
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    let stats_text = format!(
        "Notes: {}  |  Folders: {}  |  Attachments: {}",
        stats.total_notes, stats.total_folders, stats.total_attachments
    );

    let paragraph = Paragraph::new(stats_text)
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Center)
        .block(block);
    f.render_widget(paragraph, area);
}

fn draw_tree(
    f: &mut Frame,
    area: Rect,
    flat_items: &[crate::tree::FlatItem],
    selected: usize,
    tree_scroll: &mut usize,
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(" Tree ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    let visible_height = inner.height as usize;
    let scroll = if selected >= *tree_scroll + visible_height {
        // Cursor moved below viewport — scroll down
        selected.saturating_sub(visible_height) + 1
    } else if selected < *tree_scroll {
        // Cursor moved above viewport — scroll up
        selected
    } else {
        *tree_scroll
    };
    // Persist the computed scroll back to app state
    *tree_scroll = scroll;

    let items: Vec<ListItem> = flat_items
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, item)| {
            if item.kind == NodeKind::Divider {
                let indent = "  ".repeat(item.depth);
                let rule_width = inner.width as usize - (item.depth * 2) - 2;
                let rule = "─".repeat(rule_width);
                return ListItem::new(format!("{indent}  {rule}"))
                    .style(Style::default().fg(Color::DarkGray));
            }

            let indent = "  ".repeat(item.depth);
            let icon = match item.kind {
                NodeKind::Folder => {
                    if item.expanded { "▼ " } else { "▶ " }
                }
                NodeKind::Note | NodeKind::Attachment | NodeKind::Divider => "  ",
            };
            let kind_icon = match item.kind {
                NodeKind::Folder => "■ ",
                NodeKind::Note => {
                    if item.is_pinned { "★ " } else { "◇ " }
                }
                NodeKind::Attachment => "· ",
                NodeKind::Divider => "",
            };

            let prefix = format!("{indent}{icon}{kind_icon}");
            let safe_name = sanitize_for_display(&item.name);
            let max_name = (inner.width as usize).saturating_sub(display_width(&prefix));
            let name = truncate_to_width(&safe_name, max_name);

            let style = if i == selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                match item.kind {
                    NodeKind::Folder => Style::default().fg(Color::Yellow),
                    NodeKind::Note => Style::default().fg(Color::White),
                    NodeKind::Attachment => Style::default().fg(Color::DarkGray),
                    NodeKind::Divider => Style::default().fg(Color::DarkGray),
                }
            };

            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(name, style),
            ]))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

fn draw_note_preview(
    f: &mut Frame,
    area: Rect,
    content: Option<&str>,
    scroll: &mut usize,
    focused: bool,
    selected_item: Option<&crate::tree::FlatItem>,
) {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Build title with modification date if a note is selected
    let title = if let Some(item) = selected_item {
        if item.kind == NodeKind::Note {
            if let Some(cocoa_ts) = item.modified_date {
                let unix_ts = (cocoa_ts + 978_307_200.0) as i64;
                let formatted = format_unix_timestamp(unix_ts);
                format!(" Preview — Last Modified: {} ", formatted)
            } else {
                " Preview ".to_string()
            }
        } else {
            " Preview ".to_string()
        }
    } else {
        " Preview ".to_string()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    match content {
        Some(text) if text.starts_with("__IMAGE__:") => {
            // Render image as ANSI art
            let path_str = &text["__IMAGE__:".len()..];
            let path = std::path::Path::new(path_str);
            let lines = markdown::image_to_ansi_lines(path, inner.width as usize);
            let visible_height = inner.height as usize;
            let visible_lines: Vec<Line> = lines.into_iter().take(visible_height).collect();
            let paragraph = Paragraph::new(visible_lines);
            f.render_widget(paragraph, inner);
        }
        Some(text) if text.contains("__INLINE_IMAGE__:") => {
            // Mixed text + inline images: split on image markers,
            // render text sections as markdown and image sections as ANSI art.
            let width = inner.width as usize;
            let mut all_lines: Vec<Line> = Vec::new();

            for segment in text.split("__INLINE_IMAGE__:") {
                if segment.is_empty() {
                    continue;
                }
                // Extract path: everything up to the first \n, or the whole
                // segment if there's no \n (last image at end of text).
                let newline_pos = segment.find('\n');
                let path_candidate = match newline_pos {
                    Some(p) => segment[..p].trim(),
                    None => segment.trim(),
                };
                if !path_candidate.is_empty()
                    && std::path::Path::new(path_candidate).exists()
                {
                    let is_image = ["png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "heic"]
                        .iter()
                        .any(|ext| path_candidate.to_lowercase().ends_with(ext));
                    if is_image {
                        let path = std::path::Path::new(path_candidate);
                        let img_lines = markdown::image_to_ansi_lines(path, width);
                        all_lines.extend(img_lines);
                    } else {
                        all_lines.push(Line::styled(
                            format!("[Attachment: {}]", std::path::Path::new(path_candidate)
                                .file_name().unwrap_or_default().to_string_lossy()),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                    // Render remaining text after the path (if any)
                    if let Some(p) = newline_pos {
                        let rest = &segment[p..];
                        if !rest.trim().is_empty() {
                            all_lines.extend(render_text_with_callouts(rest, width));
                        }
                    }
                    continue;
                }
                // Plain text segment
                all_lines.extend(render_text_with_callouts(segment, width));
            }

            let visible_height = inner.height as usize;
            let max_scroll = all_lines.len().saturating_sub(visible_height);
            *scroll = (*scroll).min(max_scroll);
            let visible_lines: Vec<Line> = all_lines
                .into_iter()
                .skip(*scroll)
                .take(visible_height)
                .collect();
            let paragraph = Paragraph::new(visible_lines);
            f.render_widget(paragraph, inner);
        }
        Some(text) => {
            let width = inner.width as usize;
            let lines = render_text_with_callouts(text, width);

            let visible_height = inner.height as usize;
            let max_scroll = lines.len().saturating_sub(visible_height);
            *scroll = (*scroll).min(max_scroll);

            let visible_lines: Vec<Line> = lines
                .into_iter()
                .skip(*scroll)
                .take(visible_height)
                .collect();

            let paragraph = Paragraph::new(visible_lines);
            f.render_widget(paragraph, inner);
        }
        None => {
            let help = Paragraph::new("Select a note to preview")
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center);
            // Center vertically
            let y_offset = inner.height / 2;
            let centered = Rect::new(inner.x, inner.y + y_offset, inner.width, 1);
            f.render_widget(help, centered);
        }
    }
}

fn draw_footer(
    f: &mut Frame,
    area: Rect,
    source: &NoteSource,
    focus_tree: bool,
    error_message: Option<&str>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));

    let focus_indicator = if focus_tree { "[Tree]" } else { "[Preview]" };

    let base_help = match source {
        NoteSource::Obsidian => {
            format!(
                "{focus_indicator}  ↑↓/jk: Navigate  ←→/hl: Collapse/Expand  Tab: Switch pane  i: Integrity  c: Config  Esc: Back  q: Quit"
            )
        }
        NoteSource::AppleNotes => {
            format!(
                "{focus_indicator}  ↑↓/jk: Navigate  ←→/hl: Collapse/Expand  Tab: Switch pane  d: Debug Attachments  2: Debug Text  e: Export  Esc: Back  q: Quit"
            )
        }
    };

    let text = if let Some(err) = error_message {
        format!("{err}  |  {base_help}")
    } else {
        base_help
    };

    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(block);
    f.render_widget(paragraph, area);
}

fn draw_integrity_popup(f: &mut Frame, area: Rect, popup: &mut crate::app::IntegrityPopup) {
    use crate::obsidian::IntegrityIssue;

    // Dim background
    let dim = Block::default().style(Style::default().bg(Color::Black));
    f.render_widget(dim, area);

    // Popup centered, max 50% width and height
    let popup_area = centered_rect(50, 50, area);

    let issue_count = popup.result.issues.len();
    let title = if issue_count == 0 {
        " Integrity Check — All Clear ".to_string()
    } else {
        format!(" Integrity Check — {} Issue{} ", issue_count, if issue_count == 1 { "" } else { "s" })
    };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(popup_area);
    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(2)])
        .split(inner);

    // Summary
    let summary = format!(
        "Notes: {}  |  Attachments: {}  |  Broken links: {}  |  Unlinked: {}",
        popup.result.notes_scanned,
        popup.result.attachments_scanned,
        popup.result.broken_links,
        popup.result.unlinked_attachments,
    );
    let summary_widget = Paragraph::new(summary)
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Center);
    f.render_widget(summary_widget, chunks[0]);

    // Issues list
    if issue_count == 0 {
        let ok = Paragraph::new("No issues found.")
            .style(Style::default().fg(Color::Green))
            .alignment(Alignment::Center);
        f.render_widget(ok, chunks[1]);
    } else {
        let visible_height = chunks[1].height.saturating_sub(1) as usize; // -1 for border
        popup.visible_height = visible_height;
        let scroll = popup.scroll;

        let items: Vec<ListItem> = popup
            .result
            .issues
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible_height)
            .map(|(i, issue)| {
                let (icon, text, base_color) = match issue {
                    IntegrityIssue::BrokenLink { source_note, link_target } => {
                        let note_name = source_note
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy();
                        ("⚠", format!("{link_target}  ← {note_name}"), Color::Red)
                    }
                    IntegrityIssue::UnlinkedAttachment { path } => {
                        let fname = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy();
                        ("●", format!("{fname}  (unlinked)"), Color::Yellow)
                    }
                };
                let style = if i == popup.selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(base_color)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(base_color)
                };
                ListItem::new(format!("  {icon} {text}")).style(style)
            })
            .collect();

        let list_block = Block::default()
            .title(" Issues ")
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray));
        let list = List::new(items).block(list_block);
        f.render_widget(list, chunks[1]);
    }

    // Help — show delete option when an unlinked attachment is selected
    let help_text = if issue_count > 0 {
        let is_unlinked = matches!(
            popup.result.issues.get(popup.selected),
            Some(IntegrityIssue::UnlinkedAttachment { .. })
        );
        if is_unlinked {
            "↑↓/jk: Navigate  d: Delete attachment  Esc/q: Close"
        } else {
            "↑↓/jk: Navigate  Esc/q: Close"
        }
    } else {
        "Esc/q: Close"
    };
    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(help, chunks[2]);
}

fn draw_config_popup(f: &mut Frame, area: Rect, popup: &mut crate::app::ConfigPopup) {
    // Dim background
    let dim = Block::default().style(Style::default().bg(Color::Black));
    f.render_widget(dim, area);

    // Popup centered, max 50% width and height
    let popup_area = centered_rect(50, 50, area);

    let title = format!(
        " Vault Config — {} file{} ",
        popup.files.len(),
        if popup.files.len() == 1 { "" } else { "s" }
    );
    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(popup_area);
    f.render_widget(Clear, popup_area);
    f.render_widget(block, popup_area);

    if popup.files.is_empty() {
        let empty = Paragraph::new("No config files found.")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(empty, inner);
        return;
    }

    // Split into file list (left) and content (right)
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(0)])
        .split(inner);

    // Left: file list (full height minus help line)
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(panes[0]);

    let visible_height = left_chunks[0].height as usize;
    let file_scroll = if popup.selected >= visible_height {
        popup.selected - visible_height + 1
    } else {
        0
    };
    let items: Vec<ListItem> = popup
        .files
        .iter()
        .enumerate()
        .skip(file_scroll)
        .take(visible_height)
        .map(|(i, cf)| {
            let style = if i == popup.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(format!(" {}", cf.name)).style(style)
        })
        .collect();

    let list_border_color = if popup.focus_content { Color::DarkGray } else { Color::Cyan };
    let list_block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(list_border_color));
    let list = List::new(items).block(list_block);
    f.render_widget(list, left_chunks[0]);

    // Help in bottom-left
    let left_help = if popup.focus_content { " ←: Files" } else { " ↑↓ →: View  Esc/q" };
    let help = Paragraph::new(left_help)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(help, left_chunks[1]);

    // Right: file content
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(panes[1]);

    if let Some(cf) = popup.files.get(popup.selected) {
        let content_lines: Vec<&str> = cf.content.lines().collect();
        let content_height = right_chunks[0].height as usize;
        let max_scroll = content_lines.len().saturating_sub(content_height);
        popup.max_scroll = max_scroll;
        popup.content_scroll = popup.content_scroll.min(max_scroll);
        let scroll = popup.content_scroll;

        let visible: String = content_lines
            .iter()
            .skip(scroll)
            .take(content_height)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");

        let content_color = if popup.focus_content { Color::White } else { Color::DarkGray };
        let content_widget = Paragraph::new(visible)
            .style(Style::default().fg(content_color));
        f.render_widget(content_widget, right_chunks[0]);

        // Scroll indicator
        let indicator = if content_lines.len() > content_height {
            format!(
                " {}-{}/{}  ↑↓: Scroll  ←: Files",
                scroll + 1,
                (scroll + content_height).min(content_lines.len()),
                content_lines.len()
            )
        } else if popup.focus_content {
            " ←: Files".to_string()
        } else {
            String::new()
        };
        let scroll_info = Paragraph::new(indicator)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Right);
        f.render_widget(scroll_info, right_chunks[1]);
    }
}

/// Create a centered rect using percentage of parent area.
fn draw_export_vault_select(
    f: &mut Frame,
    vaults: Option<&[std::path::PathBuf]>,
    selected: usize,
    scroll_offset: usize,
    new_name_input: Option<&str>,
) {
    let area = f.area();

    // If still scanning, show a simple loading screen
    let Some(vaults) = vaults else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(area);
        let header_block = Block::default()
            .title(" Export to Obsidian — Select Destination ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow));
        let header = Paragraph::new("Searching for Obsidian vaults...")
            .style(Style::default().fg(Color::Yellow))
            .alignment(Alignment::Center)
            .block(header_block);
        f.render_widget(header, chunks[0]);

        let body_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray));
        let body = Paragraph::new("Scanning ~/  (this may take a moment)")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(body_block);
        f.render_widget(body, chunks[1]);

        let footer_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray));
        let footer = Paragraph::new("Esc: Cancel")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(footer_block);
        f.render_widget(footer, chunks[2]);
        return;
    };

    let has_input = new_name_input.is_some();
    let mut constraints = vec![
        Constraint::Length(3),
        Constraint::Min(0),
    ];
    if has_input {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Length(3));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // Header
    let header_block = Block::default()
        .title(" Export to Obsidian — Select Destination ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));
    let vault_summary = if vaults.is_empty() {
        "No Obsidian vaults found".to_string()
    } else {
        format!("Found {} Obsidian vault{}", vaults.len(), if vaults.len() == 1 { "" } else { "s" })
    };
    let header = Paragraph::new(vault_summary)
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Center)
        .block(header_block);
    f.render_widget(header, chunks[0]);

    // Build the item list: vaults, divider, browse, create
    let vault_count = vaults.len();
    let browse_idx = vault_count;
    let create_idx = vault_count + 1;
    let inner_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = inner_block.inner(chunks[1]);
    f.render_widget(inner_block, chunks[1]);

    let visible_height = inner.height as usize;
    let scroll = if selected >= scroll_offset + visible_height {
        selected.saturating_sub(visible_height) + 1
    } else {
        scroll_offset
    };

    // We need to render: vaults + divider + 2 action items
    // The divider is visual-only, not selectable (handled in key logic)
    let mut items: Vec<ListItem> = Vec::new();
    let mut item_idx = 0;

    // Vaults
    for (i, vault_path) in vaults.iter().enumerate() {
        if item_idx >= scroll + visible_height {
            break;
        }
        if item_idx >= scroll {
            let name = vault_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let path_display = vault_path.to_string_lossy();
            let style = if i == selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Green)
            };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(format!(" ♦ {name}"), style),
                Span::styled(
                    format!("  {path_display}"),
                    if i == selected {
                        style
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
            ])));
        }
        item_idx += 1;
    }

    // Divider between vaults and actions
    if item_idx >= scroll && item_idx < scroll + visible_height && vault_count > 0 {
        let rule_width = inner.width.saturating_sub(4) as usize;
        let rule = "─".repeat(rule_width);
        items.push(ListItem::new(format!("  {rule}"))
            .style(Style::default().fg(Color::DarkGray)));
    }
    if vault_count > 0 {
        item_idx += 1; // divider takes a visual slot but isn't in total_items
    }

    // "Browse folders..."
    if item_idx >= scroll && item_idx < scroll + visible_height {
        let style = if selected == browse_idx {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };
        items.push(ListItem::new(" ▸ Browse folders...").style(style));
    }
    item_idx += 1;

    // "Create new vault..."
    if item_idx >= scroll && item_idx < scroll + visible_height {
        let style = if selected == create_idx {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        items.push(ListItem::new(" ▸ Create new vault...").style(style));
    }

    let list = List::new(items);
    f.render_widget(list, inner);

    // Text input for new vault name (if active)
    if let Some(input_text) = new_name_input {
        let input_block = Block::default()
            .title(" New vault name (Enter to create, Esc to cancel) ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));
        let home = dirs::home_dir().unwrap_or_default();
        let preview = home.join(if input_text.is_empty() { "..." } else { input_text });
        let display = format!("{}▏", preview.display());
        let input_widget = Paragraph::new(display)
            .style(Style::default().fg(Color::White))
            .block(input_block);
        f.render_widget(input_widget, chunks[2]);
    }

    // Footer
    let footer_idx = chunks.len() - 1;
    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let help = Paragraph::new("↑↓/jk: Navigate  Enter: Select  Esc: Cancel")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(help_block);
    f.render_widget(help, chunks[footer_idx]);
}

fn draw_export_folder_select(
    f: &mut Frame,
    current_path: &str,
    entries: &[std::path::PathBuf],
    selected: usize,
    scroll_offset: usize,
    new_name_input: Option<&str>,
) {
    let area = f.area();

    let has_input = new_name_input.is_some();
    let mut constraints = vec![
        Constraint::Length(3),
        Constraint::Min(0),
    ];
    if has_input {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Length(3));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    // Path header
    let path_block = Block::default()
        .title(" Export to Obsidian — Browse Folders ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));
    let path_text = Paragraph::new(current_path)
        .style(Style::default().fg(Color::Yellow))
        .block(path_block);
    f.render_widget(path_text, chunks[0]);

    // Directory list with vault indicators
    let visible_height = chunks[1].height.saturating_sub(2) as usize;
    let scroll = if selected >= scroll_offset + visible_height {
        selected.saturating_sub(visible_height) + 1
    } else {
        scroll_offset
    };

    let items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, path)| {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_vault = crate::obsidian::is_obsidian_vault(path);
            let icon = if is_vault { "♦ " } else { "▪ " };
            let style = if i == selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(if is_vault { Color::Green } else { Color::Yellow })
                    .add_modifier(Modifier::BOLD)
            } else if is_vault {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(format!(" {icon}{name}")).style(style)
        })
        .collect();

    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" {} items ", entries.len()));
    let list = List::new(items).block(list_block);
    f.render_widget(list, chunks[1]);

    // Text input for new directory name (if active)
    if let Some(input_text) = new_name_input {
        let input_block = Block::default()
            .title(" New vault name (Enter to create, Esc to cancel) ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan));
        let display = format!("{input_text}▏");
        let input_widget = Paragraph::new(display)
            .style(Style::default().fg(Color::White))
            .block(input_block);
        f.render_widget(input_widget, chunks[2]);
    }

    // Help footer
    let footer_idx = chunks.len() - 1;
    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let help = Paragraph::new(
        "↑↓/jk: Navigate  Enter: Open  ←/Backspace: Parent  x: Export here  n: New vault  Esc: Back",
    )
    .style(Style::default().fg(Color::DarkGray))
    .alignment(Alignment::Center)
    .block(help_block);
    f.render_widget(help, chunks[footer_idx]);
}

fn draw_export_results(f: &mut Frame, log: &crate::export::ExportLog, scroll: usize) {
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    // Header with stats
    let status = if log.is_complete { "Complete" } else { "Running..." };
    let header_block = Block::default()
        .title(format!(" Export {status} "))
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if log.errors > 0 {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        });

    let stats_text = format!(
        "Notes: {}  |  Attachments: {}  |  Folders: {}  |  Errors: {}",
        log.notes_exported, log.attachments_copied, log.folders_created, log.errors
    );
    let header = Paragraph::new(stats_text)
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Center)
        .block(header_block);
    f.render_widget(header, chunks[0]);

    // Log output
    let log_block = Block::default()
        .title(" Log ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = log_block.inner(chunks[1]);
    f.render_widget(log_block, chunks[1]);

    let visible_height = inner.height as usize;
    let max_scroll = log.lines.len().saturating_sub(visible_height);
    let actual_scroll = scroll.min(max_scroll);

    let lines: Vec<Line> = log
        .lines
        .iter()
        .skip(actual_scroll)
        .take(visible_height)
        .map(|line| {
            if line.starts_with("ERROR:") {
                Line::styled(line.clone(), Style::default().fg(Color::Red))
            } else if line.starts_with("  Exported:") {
                Line::styled(line.clone(), Style::default().fg(Color::Green))
            } else if line.starts_with("Folder:") {
                Line::styled(
                    line.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else if line.contains("complete") || line.contains("═") {
                Line::styled(
                    line.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Line::raw(line.clone())
            }
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);

    // Footer
    let footer_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let footer = Paragraph::new("↑↓/jk: Scroll  PageUp/PageDown  Esc/q: Done")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(footer_block);
    f.render_widget(footer, chunks[2]);
}

/// Sanitize a string for display in ratatui.
///
/// Ratatui processes characters one codepoint at a time using `unicode-width`.
/// Multi-codepoint emoji sequences (flags, ZWJ families, skin tones) corrupt
/// the buffer because the terminal combines them into a single glyph but
/// ratatui allocated separate cells for each codepoint.
///
/// This function replaces only the problematic multi-codepoint clusters:
///   - Flag emoji (Regional Indicator pairs) → two-letter country code
///   - ZWJ / skin tone / variation sequences → base emoji (first codepoint)
/// Single-codepoint emoji (✅, 🧠, etc.) pass through unchanged.
fn sanitize_for_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for g in s.graphemes(true) {
        let mut chars = g.chars();
        let first = match chars.next() {
            Some(c) => c,
            None => continue,
        };
        if chars.next().is_none() {
            // Single codepoint — always safe for ratatui
            out.push(first);
            continue;
        }
        // Multi-codepoint grapheme cluster
        // Replace any multi-codepoint sequence with a single safe symbol
        out.push('⚑');
    }
    out
}

/// Measure display width using `unicode-width` per character.
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// Truncate a string to fit within `max_cols` terminal columns.
fn truncate_to_width(s: &str, max_cols: usize) -> String {
    let w = display_width(s);
    if w <= max_cols {
        return s.to_string();
    }
    let mut width = 0usize;
    let mut result = String::new();
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw + 1 > max_cols {
            result.push('…');
            break;
        }
        result.push(c);
        width += cw;
    }
    result
}

/// Format a Unix timestamp as "YYYY-MM-DD HH:MM".
fn format_unix_timestamp(ts: i64) -> String {
    // Manual UTC conversion (no chrono dependency)
    let secs_per_min = 60i64;
    let secs_per_hour = 3600i64;
    let secs_per_day = 86400i64;

    let mut days = ts / secs_per_day;
    let day_secs = ts % secs_per_day;
    let hour = day_secs / secs_per_hour;
    let minute = (day_secs % secs_per_hour) / secs_per_min;

    // Days since 1970-01-01
    let mut year = 1970i64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let month_days: [i64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    let day = days + 1;

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Render text to styled lines, detecting Obsidian `> [!info]` callout blocks
/// and rendering them with dark blue background and `│` prefix.
fn render_text_with_callouts(text: &str, width: usize) -> Vec<Line<'static>> {
    let callout_bg = Color::Rgb(15, 23, 42);
    let prefix_style = Style::default().fg(Color::Cyan).bg(callout_bg);
    let header_style = Style::default()
        .fg(Color::Cyan)
        .bg(callout_bg)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().bg(callout_bg);
    let pad_style = Style::default().bg(callout_bg);

    // Build a callout line padded with spaces to fill the full width
    let make_callout_line = |spans: Vec<Span<'static>>| -> Line<'static> {
        let content_width: usize = spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        let mut padded = spans;
        if content_width < width {
            padded.push(Span::styled(
                " ".repeat(width - content_width),
                pad_style,
            ));
        }
        Line::from(padded)
    };

    // Split text into segments: regular markdown and callout blocks.
    // A callout starts with `> [!info]` (or similar) and continues while lines start with `>`.
    let mut all_lines: Vec<Line<'static>> = Vec::new();
    let mut regular_buf = String::new();
    let mut in_callout = false;
    let mut consecutive_blanks: usize = 0;

    for line in text.split('\n') {
        let trimmed = line.trim();

        // Track consecutive blank lines outside callouts so we can preserve
        // the original spacing (pulldown-cmark would normalize them away).
        if trimmed.is_empty() && !in_callout {
            consecutive_blanks += 1;
            if consecutive_blanks <= 1 {
                // Buffer the first blank line for markdown paragraph breaks
                regular_buf.push('\n');
            }
            continue;
        }

        // When we hit non-blank content after 2+ blank lines, flush the
        // buffer and emit the extra blank lines directly.
        if consecutive_blanks > 1 {
            if !regular_buf.is_empty() {
                all_lines.extend(markdown::markdown_to_lines(&regular_buf, width));
                regular_buf.clear();
            }
            for _ in 1..consecutive_blanks {
                all_lines.push(Line::raw(""));
            }
        }
        consecutive_blanks = 0;

        if !in_callout && trimmed.starts_with("> [!") {
            // Flush any regular text accumulated so far
            if !regular_buf.is_empty() {
                all_lines.extend(markdown::markdown_to_lines(&regular_buf, width));
                regular_buf.clear();
            }
            // Emit callout header line
            let callout_type = trimmed
                .trim_start_matches('>')
                .trim()
                .trim_start_matches("[!")
                .trim_end_matches(']')
                .to_uppercase();
            all_lines.push(make_callout_line(vec![Span::styled(
                format!("│ ℹ  {callout_type}"),
                header_style,
            )]));
            in_callout = true;
        } else if in_callout && (trimmed.starts_with('>') || trimmed == ">") {
            // Callout content line — strip the `> ` prefix
            let content = trimmed.strip_prefix('>')
                .unwrap_or("")
                .strip_prefix(' ')
                .unwrap_or("");
            if content.is_empty() {
                // Blank line inside callout — keep the │ prefix
                all_lines.push(make_callout_line(vec![
                    Span::styled("│ ", prefix_style),
                ]));
            } else {
                all_lines.push(make_callout_line(vec![
                    Span::styled("│ ", prefix_style),
                    Span::styled(content.to_string(), text_style),
                ]));
            }
        } else if in_callout {
            // Line doesn't start with `>` — callout ended
            in_callout = false;
            // Add spacing after the callout
            all_lines.push(Line::raw(""));
            all_lines.push(Line::raw(""));
            // Process this line as regular text
            regular_buf.push_str(line);
            regular_buf.push('\n');
        } else {
            // Regular markdown line
            regular_buf.push_str(line);
            regular_buf.push('\n');
        }
    }

    // If the text ended while still in a callout, add spacing
    if in_callout {
        all_lines.push(Line::raw(""));
        all_lines.push(Line::raw(""));
    }

    // Handle trailing consecutive blanks
    if consecutive_blanks > 1 && !regular_buf.is_empty() {
        all_lines.extend(markdown::markdown_to_lines(&regular_buf, width));
        regular_buf.clear();
        for _ in 1..consecutive_blanks {
            all_lines.push(Line::raw(""));
        }
    }

    // Flush remaining regular text
    if !regular_buf.is_empty() {
        all_lines.extend(markdown::markdown_to_lines(&regular_buf, width));
    }

    all_lines
}
