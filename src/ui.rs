//! UI rendering module.
//!
//! This module contains functions for rendering the user interface,
//! including the tree view, status bar, help screen, and various dialogs.

use std::time::SystemTime;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::explorer::FileExplorer;
use crate::types::{CachedFile, CreationType, FuzzyMatch, TreeLine, UIMode};

/// Get icon for a file based on name, type, and permissions.
pub fn get_file_icon(name: &str, is_dir: bool, permissions: u32) -> &'static str {
    // Directories
    if is_dir {
        return "";
    }

    // Check if executable (any execute bit set)
    let is_executable = permissions & 0o111 != 0;

    // Get file extension
    let extension = if let Some(pos) = name.rfind('.') {
        &name[pos + 1..]
    } else {
        ""
    };

    // Check for specific filenames first
    match name {
        ".git" | ".gitignore" | ".gitmodules" | ".gitattributes" => return "",
        "Cargo.toml" | "Cargo.lock" => return "",
        "package.json" | "package-lock.json" => return "",
        "README.md" | "readme.md" => return "",
        "Makefile" | "makefile" => return "",
        "Dockerfile" | "docker-compose.yml" => return "",
        _ => {}
    }

    // Check by extension
    match extension.to_lowercase().as_str() {
        // Programming languages
        "rs" => "",
        "py" => "",
        "js" | "jsx" | "mjs" => "",
        "ts" | "tsx" => "",
        "go" => "",
        "c" | "h" => "",
        "cpp" | "cc" | "cxx" | "hpp" => "",
        "java" => "",
        "rb" => "",
        "php" => "",
        "sh" | "bash" | "zsh" | "fish" => "",

        // Markup/Data
        "html" | "htm" => "",
        "css" | "scss" | "sass" | "less" => "",
        "json" => "",
        "xml" => "",
        "yaml" | "yml" => "",
        "toml" => "",
        "md" | "markdown" => "",

        // Archives
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => "",

        // Images
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "ico" => "",

        // Documents
        "pdf" => "",
        "txt" => "",

        // Executables/binaries
        "exe" | "bin" | "out" => "",

        // Default for unknown extensions
        _ => {
            if is_executable {
                ""  // Executable file
            } else {
                ""  // Regular file
            }
        }
    }
}

/// Format Unix permissions as a string like "drwxr-xr-x".
pub fn format_permissions(mode: u32, is_dir: bool) -> String {
    let file_type = if is_dir { 'd' } else { '-' };

    let user_r = if mode & 0o400 != 0 { 'r' } else { '-' };
    let user_w = if mode & 0o200 != 0 { 'w' } else { '-' };
    let user_x = if mode & 0o100 != 0 { 'x' } else { '-' };

    let group_r = if mode & 0o040 != 0 { 'r' } else { '-' };
    let group_w = if mode & 0o020 != 0 { 'w' } else { '-' };
    let group_x = if mode & 0o010 != 0 { 'x' } else { '-' };

    let other_r = if mode & 0o004 != 0 { 'r' } else { '-' };
    let other_w = if mode & 0o002 != 0 { 'w' } else { '-' };
    let other_x = if mode & 0o001 != 0 { 'x' } else { '-' };

    format!("{}{}{}{}{}{}{}{}{}{}",
        file_type, user_r, user_w, user_x,
        group_r, group_w, group_x,
        other_r, other_w, other_x)
}

/// Format a file size in human-readable format.
pub fn format_file_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if size >= GB {
        format!("{:.2} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.2} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.2} KB", size as f64 / KB as f64)
    } else {
        format!("{} B", size)
    }
}

/// Format a timestamp for display.
pub fn format_date(time: SystemTime) -> String {
    if let Ok(duration) = time.duration_since(SystemTime::UNIX_EPOCH) {
        let secs = duration.as_secs();

        let days = (secs / 86400) as i64;
        let remaining_secs = secs % 86400;
        let hours = remaining_secs / 3600;
        let minutes = (remaining_secs % 3600) / 60;

        let mut year = 1970;
        let mut remaining_days = days;

        loop {
            let days_in_year = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                366
            } else {
                365
            };

            if remaining_days >= days_in_year {
                remaining_days -= days_in_year;
                year += 1;
            } else {
                break;
            }
        }

        let days_per_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut month = 1;
        let mut day_of_month = remaining_days + 1;

        for (i, &days_in_month) in days_per_month.iter().enumerate() {
            let days_this_month = if i == 1 && year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                days_in_month
            };

            if day_of_month > days_this_month {
                day_of_month -= days_this_month;
                month += 1;
            } else {
                break;
            }
        }

        return format!("{:04}-{:02}-{:02} {:02}:{:02}", year, month, day_of_month, hours, minutes);
    }

    "Unknown         ".to_string()
}

/// Render fuzzy find results as list items.
#[allow(dead_code)]
pub fn render_fuzzy_results(
    matches: &[FuzzyMatch],
    selected_index: usize,
    file_cache: &[CachedFile],
    terminal_width: usize,
) -> (Vec<ListItem<'static>>, ListState, String) {
    let cache_building = file_cache.is_empty();

    let fuzzy_items: Vec<ListItem> = if cache_building {
        vec![ListItem::new(Line::from(vec![
            Span::styled("Building file cache, please wait...", Style::default().fg(Color::Rgb(140, 180, 120)))
        ]))]
    } else {
        matches
            .iter()
            .enumerate()
            .rev()
            .map(|(idx, fuzzy_match)| {
                let is_selected = idx == selected_index;
                let icon = get_file_icon(&fuzzy_match.name, fuzzy_match.is_dir, fuzzy_match.permissions);

                let mut spans = vec![Span::raw(format!("{} ", icon))];

                let grey_color = Color::Rgb(120, 120, 117);
                let green_color = Color::Rgb(140, 180, 120);
                let bg_color = if is_selected { Some(Color::Rgb(50, 50, 50)) } else { None };

                let chars: Vec<char> = fuzzy_match.display_path.chars().collect();
                let mut last_pos = 0;

                for &match_pos in &fuzzy_match.matched_positions {
                    if match_pos > last_pos {
                        let non_matched: String = chars[last_pos..match_pos].iter().collect();
                        let mut style = Style::default().fg(grey_color);
                        if let Some(bg) = bg_color {
                            style = style.bg(bg);
                        }
                        spans.push(Span::styled(non_matched, style));
                    }

                    if match_pos < chars.len() {
                        let matched_char = chars[match_pos].to_string();
                        let mut style = Style::default().fg(green_color);
                        if let Some(bg) = bg_color {
                            style = style.bg(bg);
                        }
                        if is_selected {
                            style = style.add_modifier(Modifier::BOLD);
                        }
                        spans.push(Span::styled(matched_char, style));
                        last_pos = match_pos + 1;
                    }
                }

                if last_pos < chars.len() {
                    let remaining: String = chars[last_pos..].iter().collect();
                    let mut style = Style::default().fg(grey_color);
                    if let Some(bg) = bg_color {
                        style = style.bg(bg);
                    }
                    spans.push(Span::styled(remaining, style));
                }

                let icon_width = 2;
                let path_width = fuzzy_match.display_path.chars().count();
                let perms_width = 10;
                let buffer = 1;

                let used_width = icon_width + path_width + perms_width + buffer;
                let padding_needed = if terminal_width > used_width {
                    terminal_width - used_width
                } else {
                    1
                };

                let mut padding_style = Style::default();
                if let Some(bg) = bg_color {
                    padding_style = padding_style.bg(bg);
                }
                spans.push(Span::styled(" ".repeat(padding_needed), padding_style));

                let perms_str = format_permissions(fuzzy_match.permissions, fuzzy_match.is_dir);
                let perm_color = Color::Rgb(120, 120, 117);
                let mut perm_style = Style::default().fg(perm_color);
                if let Some(bg) = bg_color {
                    perm_style = perm_style.bg(bg);
                }
                spans.push(Span::styled(perms_str, perm_style));

                ListItem::new(Line::from(spans))
            })
            .collect()
    };

    let visual_selected = if cache_building || matches.is_empty() {
        None
    } else {
        Some(matches.len() - 1 - selected_index)
    };

    let list_state = ListState::default()
        .with_selected(visual_selected)
        .with_offset(0);

    let title = if cache_building {
        "Fuzzy Find: (building cache...)".to_string()
    } else {
        format!("Fuzzy Find: ({} matches)", matches.len())
    };

    (fuzzy_items, list_state, title)
}

/// Build tree line items for the file list.
pub fn build_tree_items(tree_lines: &[TreeLine]) -> Vec<ListItem<'static>> {
    tree_lines
        .iter()
        .map(|tree_line| {
            let text_color = if tree_line.is_cursor && tree_line.is_selected {
                Color::Rgb(165, 162, 157)
            } else if tree_line.is_cursor {
                Color::Rgb(165, 162, 157)
            } else if tree_line.is_selected {
                Color::Rgb(190, 182, 165)
            } else if tree_line.is_current_dir {
                Color::Rgb(160, 150, 135)
            } else if tree_line.is_hidden && tree_line.is_dir {
                Color::Rgb(75, 75, 75)
            } else if tree_line.is_hidden {
                Color::Rgb(100, 100, 98)
            } else if tree_line.is_dir {
                Color::Rgb(130, 125, 115)
            } else {
                Color::Rgb(190, 182, 165)
            };

            let (bg_color, modifiers) = if tree_line.is_cursor && tree_line.is_selected {
                (Some(Color::Rgb(60, 60, 60)), Modifier::BOLD)
            } else if tree_line.is_cursor {
                (Some(Color::Rgb(50, 50, 50)), Modifier::BOLD)
            } else if tree_line.is_selected {
                (Some(Color::Rgb(45, 45, 45)), Modifier::empty())
            } else {
                (None, Modifier::empty())
            };

            let mut text_style = Style::default()
                .fg(text_color)
                .add_modifier(modifiers);
            if let Some(bg) = bg_color {
                text_style = text_style.bg(bg);
            }

            let tree_prefix_color = Color::Rgb(65, 65, 65);
            let mut tree_prefix_style = Style::default()
                .fg(tree_prefix_color)
                .add_modifier(modifiers);
            if let Some(bg) = bg_color {
                tree_prefix_style = tree_prefix_style.bg(bg);
            }

            let timestamp_color = if tree_line.is_cursor || tree_line.is_selected {
                Color::Rgb(130, 130, 126)
            } else {
                Color::Rgb(120, 120, 117)
            };

            let mut timestamp_style = Style::default()
                .fg(timestamp_color)
                .add_modifier(modifiers);
            if let Some(bg) = bg_color {
                timestamp_style = timestamp_style.bg(bg);
            }

            let mut spans = vec![
                Span::styled(tree_line.tree_prefix.clone(), tree_prefix_style),
                Span::styled(tree_line.text.clone(), text_style)
            ];
            if let Some(timestamp) = &tree_line.timestamp {
                spans.push(Span::styled(timestamp.clone(), timestamp_style));
            }

            ListItem::new(Line::from(spans))
        })
        .collect()
}

/// Render the help screen overlay.
pub fn render_help_screen(f: &mut Frame, area: ratatui::layout::Rect, scroll_offset: usize) {
    f.render_widget(Clear, area);

    let bg_color = Color::Rgb(40, 40, 40);
    let title_color = Color::Cyan;
    let section_color = Color::Yellow;
    let key_color = Color::Green;
    let desc_color = Color::White;

    let mut help_lines = vec![];

    help_lines.push(Line::from(vec![Span::styled("", Style::default())]));
    help_lines.push(Line::from(vec![
        Span::styled("                    RUSTY FILES - Keyboard Shortcuts",
            Style::default().fg(title_color))
    ]));
    help_lines.push(Line::from(vec![Span::styled("", Style::default())]));

    // Navigation section
    help_lines.push(Line::from(vec![
        Span::styled("  NAVIGATION", Style::default().fg(section_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Up/Down                       ", Style::default().fg(key_color)),
        Span::styled("Move cursor", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Left                          ", Style::default().fg(key_color)),
        Span::styled("Go to parent directory", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Right                         ", Style::default().fg(key_color)),
        Span::styled("Enter directory", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Enter                         ", Style::default().fg(key_color)),
        Span::styled("Open file/directory", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![Span::styled("", Style::default())]));

    // Selection section
    help_lines.push(Line::from(vec![
        Span::styled("  SELECTION", Style::default().fg(section_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Shift+Up/Down                 ", Style::default().fg(key_color)),
        Span::styled("Select range", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+Space                    ", Style::default().fg(key_color)),
        Span::styled("Toggle selection", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Mouse Click+Drag              ", Style::default().fg(key_color)),
        Span::styled("Select multiple", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![Span::styled("", Style::default())]));

    // File Operations section
    help_lines.push(Line::from(vec![
        Span::styled("  FILE OPERATIONS", Style::default().fg(section_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+C                        ", Style::default().fg(key_color)),
        Span::styled("Copy", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+X                        ", Style::default().fg(key_color)),
        Span::styled("Cut", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+V                        ", Style::default().fg(key_color)),
        Span::styled("Paste", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+N                        ", Style::default().fg(key_color)),
        Span::styled("Create new", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+R                        ", Style::default().fg(key_color)),
        Span::styled("Rename", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Delete                        ", Style::default().fg(key_color)),
        Span::styled("Delete", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+D                        ", Style::default().fg(key_color)),
        Span::styled("Copy path to clipboard", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+Z                        ", Style::default().fg(key_color)),
        Span::styled("Undo", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![Span::styled("", Style::default())]));

    // View Options section
    help_lines.push(Line::from(vec![
        Span::styled("  VIEW OPTIONS", Style::default().fg(section_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+S                        ", Style::default().fg(key_color)),
        Span::styled("Toggle sort (Name/Date)", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+H                        ", Style::default().fg(key_color)),
        Span::styled("Toggle hidden files", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+L                        ", Style::default().fg(key_color)),
        Span::styled("Refresh display", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![Span::styled("", Style::default())]));

    // Search section
    help_lines.push(Line::from(vec![
        Span::styled("  SEARCH", Style::default().fg(section_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+F                        ", Style::default().fg(key_color)),
        Span::styled("Fuzzy find files/directories", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![Span::styled("", Style::default())]));

    // Other section
    help_lines.push(Line::from(vec![
        Span::styled("  OTHER", Style::default().fg(section_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+T                        ", Style::default().fg(key_color)),
        Span::styled("Open terminal at current directory", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    F1                            ", Style::default().fg(key_color)),
        Span::styled("Show/hide this help", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![
        Span::styled("    Ctrl+Q                        ", Style::default().fg(key_color)),
        Span::styled("Quit", Style::default().fg(desc_color))
    ]));
    help_lines.push(Line::from(vec![Span::styled("", Style::default())]));
    help_lines.push(Line::from(vec![Span::styled("", Style::default())]));

    let scrolled_lines: Vec<Line> = help_lines.into_iter()
        .skip(scroll_offset)
        .collect();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area);

    let para = Paragraph::new(scrolled_lines)
        .block(Block::default())
        .style(Style::default().bg(bg_color))
        .alignment(Alignment::Left);
    f.render_widget(para, chunks[0]);

    let footer_lines = vec![
        Line::from(vec![Span::styled("", Style::default())]),
        Line::from(vec![
            Span::styled("           Use Up/Down arrows to scroll | Press F1 or Esc to close",
                Style::default().fg(title_color))
        ]),
    ];
    let footer_para = Paragraph::new(footer_lines)
        .style(Style::default().bg(bg_color))
        .alignment(Alignment::Left);
    f.render_widget(footer_para, chunks[1]);
}

/// Render the rename dialog.
pub fn render_rename_dialog(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    new_name: &str,
    cursor_pos: usize,
    selection_start: Option<usize>,
) {
    let mut spans = vec![Span::raw("Rename to: ")];

    let sel_range = selection_start.map(|sel_start| {
        let start = sel_start.min(cursor_pos);
        let end = sel_start.max(cursor_pos);
        (start, end)
    });

    for (i, ch) in new_name.chars().enumerate() {
        let is_selected = sel_range.map_or(false, |(start, end)| i >= start && i < end);
        let is_cursor = i == cursor_pos;

        let style = if is_cursor && is_selected {
            Style::default().bg(Color::Rgb(165, 162, 157)).fg(Color::Rgb(160, 150, 135))
        } else if is_cursor {
            Style::default().bg(Color::Rgb(175, 167, 150)).fg(Color::Rgb(30, 30, 30))
        } else if is_selected {
            Style::default().bg(Color::Rgb(160, 150, 135)).fg(Color::Rgb(165, 162, 157))
        } else {
            Style::default()
        };

        spans.push(Span::styled(ch.to_string(), style));
    }

    if cursor_pos >= new_name.len() {
        spans.push(Span::styled("", Style::default().bg(Color::Rgb(175, 167, 150)).fg(Color::Rgb(30, 30, 30))));
    }

    let text = Line::from(spans);
    let para = Paragraph::new(text)
        .block(Block::default().title("Rename"))
        .style(Style::default().fg(Color::Rgb(175, 167, 150)))
        .alignment(Alignment::Left);
    f.render_widget(para, area);
}

/// Render the create new item dialog.
pub fn render_create_dialog(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    creation_type: &Option<CreationType>,
    name: &str,
) {
    let text = if creation_type.is_none() {
        "Create new: (f)ile or (d)irectory?".to_string()
    } else {
        let type_str = match creation_type {
            Some(CreationType::File) => "file",
            Some(CreationType::Directory) => "directory",
            None => unreachable!(),
        };
        format!("Enter {} name: {}", type_str, name)
    };
    let para = Paragraph::new(text)
        .block(Block::default().title("Create New"))
        .style(Style::default().fg(Color::Rgb(175, 167, 150)))
        .alignment(Alignment::Left);
    f.render_widget(para, area);
}

/// Render the password prompt dialog.
pub fn render_password_dialog(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    prompt: &str,
    password: &str,
) {
    let masked_password = "*".repeat(password.len());
    let text = format!("{}\n{}", prompt, masked_password);
    let para = Paragraph::new(text)
        .block(Block::default().title("Password Required"))
        .style(Style::default().fg(Color::Rgb(175, 167, 150)))
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

/// Render the delete confirmation dialog.
pub fn render_delete_dialog(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    item_count: usize,
) {
    let text = format!("Delete {} item(s)? (y/n)", item_count);
    let para = Paragraph::new(text)
        .block(Block::default().title("Confirm Delete"))
        .style(Style::default().fg(Color::Rgb(145, 135, 125)))
        .alignment(Alignment::Left);
    f.render_widget(para, area);
}

/// Render the status bar.
pub fn render_status_bar(
    f: &mut Frame,
    area: ratatui::layout::Rect,
    explorer: &FileExplorer,
) {
    let status_text = if let Some(ref msg) = explorer.status_message {
        msg.clone()
    } else {
        match &explorer.ui_mode {
            UIMode::PasswordPrompt { prompt, password, .. } => {
                let masked_password = "*".repeat(password.len());
                format!("{} {}", prompt, masked_password)
            }
            UIMode::ConfirmDelete { items } => {
                format!("Delete {} item(s)? (y/n)", items.len())
            }
            UIMode::FuzzyFind { search_term, matches, file_cache, .. } => {
                format!("Find: {} ({} matches | {} files cached)", search_term, matches.len(), file_cache.len())
            }
            _ => {
                let total_items = explorer.entries.len();
                let selected_count = explorer.selected_indices.len();
                if selected_count > 0 {
                    let total_size = explorer.get_selected_total_size();
                    let size_str = format_file_size(total_size);
                    format!("{} items | {} selected | {}", total_items, selected_count, size_str)
                } else if let Some(entry) = explorer.entries.get(explorer.cursor_index) {
                    if entry.is_dir {
                        format!("{} items | Directory: {}", total_items, entry.name)
                    } else {
                        let item_size = explorer.current_item_size.unwrap_or(0);
                        let size_str = format_file_size(item_size);
                        format!("{} items | File: {} | {}", total_items, entry.name, size_str)
                    }
                } else {
                    format!("{} items", total_items)
                }
            }
        }
    };

    let status_bar = Paragraph::new(status_text)
        .style(Style::default().fg(Color::Rgb(185, 177, 160)).bg(Color::Rgb(90, 90, 90)))
        .alignment(Alignment::Left);
    f.render_widget(status_bar, area);
}
