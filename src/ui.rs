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
        } => {
            draw_folder_select(
                f,
                &current_path.to_string_lossy(),
                entries,
                *selected,
                *scroll_offset,
                message.as_deref(),
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
            attachment_popup,
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
                attachment_popup,
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
    let menu_height = options.len() as u16 + if has_error { 10 } else { 6 };
    let menu_width = 60;
    let menu_area = centered_rect(menu_width, menu_height, area);

    let mut constraints = vec![
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(options.len() as u16 + 1),
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
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if i == selected { " ► " } else { "   " };
            ListItem::new(format!("{prefix}{name}")).style(style)
        })
        .collect();

    let list = List::new(items);
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

fn draw_folder_select(
    f: &mut Frame,
    current_path: &str,
    entries: &[std::path::PathBuf],
    selected: usize,
    scroll_offset: usize,
    message: Option<&str>,
) {
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    // Path header
    let path_block = Block::default()
        .title(" Select Obsidian Vault ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));
    let path_text = Paragraph::new(current_path)
        .style(Style::default().fg(Color::Yellow))
        .block(path_block);
    f.render_widget(path_text, chunks[0]);

    // Directory list
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
        .border_style(Style::default().fg(Color::DarkGray))
        .title(if let Some(msg) = message {
            format!(" {msg} ")
        } else {
            format!(" {} items ", entries.len())
        });
    let list = List::new(items).block(list_block);
    f.render_widget(list, chunks[1]);

    // Help footer
    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let help = Paragraph::new(
        "↑↓/jk: Navigate  Enter: Open  Backspace/←: Parent  Esc: Back  Green = Vault",
    )
    .style(Style::default().fg(Color::DarkGray))
    .alignment(Alignment::Center)
    .block(help_block);
    f.render_widget(help, chunks[2]);
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
    attachment_popup: &Option<crate::app::AttachmentPopup>,
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

    // Attachment popup overlay
    if let Some(popup) = attachment_popup {
        draw_attachment_popup(f, area, popup);
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
                "{focus_indicator}  ↑↓/jk: Navigate  ←→/hl: Collapse/Expand  Tab: Switch pane  a: Attachments  Esc: Back  q: Quit"
            )
        }
        NoteSource::AppleNotes => {
            format!(
                "{focus_indicator}  ↑↓/jk: Navigate  ←→/hl: Collapse/Expand  Tab: Switch pane  d: Debug  e: Export  Esc: Back  q: Quit"
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

fn draw_attachment_popup(f: &mut Frame, area: Rect, popup: &crate::app::AttachmentPopup) {
    // Dim background
    let dim = Block::default().style(Style::default().bg(Color::Black));
    f.render_widget(dim, area);

    // Popup centered
    let popup_area = centered_rect(70, 80, area);

    let block = Block::default()
        .title(" Attachment Analysis ")
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
        "Total: {}  |  Linked: {}  |  Unlinked: {}",
        popup.analysis.total_attachments,
        popup.analysis.linked_attachments,
        popup.analysis.unlinked.len()
    );
    let summary_widget = Paragraph::new(summary)
        .style(Style::default().fg(Color::White))
        .alignment(Alignment::Center);
    f.render_widget(summary_widget, chunks[0]);

    // Unlinked files list
    let visible_height = chunks[1].height as usize;
    let scroll = if popup.selected >= popup.scroll + visible_height {
        popup.selected.saturating_sub(visible_height) + 1
    } else {
        popup.scroll
    };

    let items: Vec<ListItem> = popup
        .analysis
        .unlinked
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, path)| {
            let name = path.to_string_lossy().to_string();
            let style = if i == popup.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(format!("  · {name}")).style(style)
        })
        .collect();

    let list_block = Block::default()
        .title(" Unlinked Attachments (candidates for pruning) ")
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let list = List::new(items).block(list_block);
    f.render_widget(list, chunks[1]);

    // Help
    let help = Paragraph::new("↑↓/jk: Navigate  Esc/q: Close")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(help, chunks[2]);
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

    for line in text.split('\n') {
        let trimmed = line.trim();

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

    // Flush remaining regular text
    if !regular_buf.is_empty() {
        all_lines.extend(markdown::markdown_to_lines(&regular_buf, width));
    }

    all_lines
}
