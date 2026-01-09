//! UI rendering module.
//!
//! This module contains functions for rendering the user interface,
//! including the tree view, status bar, help screen, and various dialogs.

use std::path::Path;
use std::time::SystemTime;

use fs2::statvfs;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::explorer::FileExplorer;
use crate::types::{CachedFile, CreationType, FuzzyMatch, QuickNavLocation, StatusType, TreeLine, UIMode};

/// Get icon for a file based on name, type, and permissions.
pub fn get_file_icon(name: &str, is_dir: bool, permissions: u32) -> &'static str {
    // Directories
    if is_dir {
        return "\u{f07b}";  //
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
        ".git" | ".gitignore" | ".gitmodules" | ".gitattributes" => return "\u{f1d3}",  //
        "Cargo.toml" | "Cargo.lock" => return "\u{e7a8}",  //
        "package.json" | "package-lock.json" => return "\u{e718}",  //
        "README.md" | "readme.md" => return "\u{f48a}",  //
        "Makefile" | "makefile" => return "\u{f489}",  //
        "Dockerfile" | "docker-compose.yml" => return "\u{f308}",  //
        _ => {}
    }

    // Check by extension
    match extension.to_lowercase().as_str() {
        // Programming languages
        "rs" => "\u{e7a8}",  //
        "py" => "\u{e73c}",  //
        "js" | "jsx" | "mjs" => "\u{e74e}",  //
        "ts" | "tsx" => "\u{e628}",  //
        "go" => "\u{e626}",  //
        "c" | "h" => "\u{e61e}",  //
        "cpp" | "cc" | "cxx" | "hpp" => "\u{e61d}",  //
        "java" => "\u{e738}",  //
        "rb" => "\u{e21e}",  //
        "php" => "\u{e73d}",  //
        "sh" | "bash" | "zsh" | "fish" => "\u{e795}",  //

        // Markup/Data
        "html" | "htm" => "\u{e736}",  //
        "css" | "scss" | "sass" | "less" => "\u{e749}",  //
        "json" => "\u{e60b}",  //
        "xml" => "\u{e619}",  //
        "yaml" | "yml" => "\u{e6a8}",  //
        "toml" => "\u{e6b2}",  //
        "md" | "markdown" => "\u{e73e}",  //

        // Archives
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => "\u{f410}",  //

        // Images
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "ico" => "\u{f03e}",  //

        // Documents
        "pdf" => "\u{f1c1}",  //
        "txt" => "\u{f15c}",  //

        // Executables/binaries
        "exe" | "bin" | "out" => "\u{f489}",  //

        // Default for unknown extensions
        _ => {
            if is_executable {
                "\u{f489}"  //  Executable file
            } else {
                "\u{f15b}"  //  Regular file
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

/// Format a file size in human-readable format (right-aligned, fixed width).
pub fn format_file_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;
    const PB: u64 = TB * 1024;

    if size >= PB {
        format!("{:>6.1} PB", size as f64 / PB as f64)
    } else if size >= TB {
        format!("{:>6.1} TB", size as f64 / TB as f64)
    } else if size >= GB {
        format!("{:>6.1} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:>6.1} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:>6.1} KB", size as f64 / KB as f64)
    } else {
        format!("{:>6}  B", size)
    }
}

/// Format a file size compactly (no padding).
pub fn format_size_compact(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;
    const PB: u64 = TB * 1024;

    if size >= PB {
        format!("{:.1} PB", size as f64 / PB as f64)
    } else if size >= TB {
        format!("{:.1} TB", size as f64 / TB as f64)
    } else if size >= GB {
        format!("{:.1} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else {
        format!("{} B", size)
    }
}

/// Get disk space info for the given path.
/// Returns (total_size, available_size) or None if unavailable.
pub fn get_disk_space(path: &Path) -> Option<(u64, u64)> {
    statvfs(path).ok().map(|stat| {
        let total = stat.total_space();
        let available = stat.available_space();
        (total, available)
    })
}

/// Get the drive/mount point for a path.
fn get_drive_label(_path: &Path) -> String {
    #[cfg(windows)]
    {
        // On Windows, get the drive letter (e.g., "C:")
        if let Some(prefix) = _path.components().next() {
            let prefix_str = prefix.as_os_str().to_string_lossy();
            if prefix_str.len() >= 2 && prefix_str.chars().nth(1) == Some(':') {
                return prefix_str.chars().take(2).collect();
            }
        }
        String::new()
    }
    #[cfg(not(windows))]
    {
        // On Unix, use "/" or the mount point
        "/".to_string()
    }
}

/// Format disk space info as a string for display in header.
pub fn format_disk_info(path: &Path) -> String {
    if let Some((total, available)) = get_disk_space(path) {
        let drive = get_drive_label(path);
        let percent_used = if total > 0 {
            ((total.saturating_sub(available)) as f64 / total as f64 * 100.0) as u8
        } else {
            0
        };
        if drive.is_empty() {
            format!(
                "{} free of {} ({}% used)",
                format_size_compact(available),
                format_size_compact(total),
                percent_used
            )
        } else {
            format!(
                "{}: {} free of {} ({}% used)",
                drive,
                format_size_compact(available),
                format_size_compact(total),
                percent_used
            )
        }
    } else {
        String::new()
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
                Span::styled(tree_line.icon.clone(), text_style),
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
    // Determine status text and type
    let (status_text, status_type) = if let Some((ref msg, ref stype)) = explorer.status_message {
        (msg.clone(), stype.clone())
    } else {
        match &explorer.ui_mode {
            UIMode::PasswordPrompt { prompt, password, .. } => {
                let masked_password = "*".repeat(password.len());
                (format!("{} {}", prompt, masked_password), StatusType::Prompt)
            }
            UIMode::ConfirmDelete { items } => {
                (format!(" \u{f071} Delete {} item(s)? (y/n)", items.len()), StatusType::Prompt)
            }
            UIMode::FuzzyFind { search_term, matches, file_cache, .. } => {
                (format!("Find: {} ({} matches | {} files cached)", search_term, matches.len(), file_cache.len()), StatusType::Info)
            }
            _ => {
                let total_items = explorer.entries.len();
                let selected_count = explorer.selected_indices.len();
                let text = if selected_count > 0 {
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
                };
                (text, StatusType::Info)
            }
        }
    };

    // Choose style based on status type
    let style = match status_type {
        StatusType::Info => {
            Style::default()
                .fg(Color::Rgb(185, 177, 160))
                .bg(Color::Rgb(50, 50, 50))
        }
        StatusType::Prompt => {
            Style::default()
                .fg(Color::Rgb(30, 30, 30))
                .bg(Color::Rgb(220, 180, 80))
                .add_modifier(Modifier::BOLD)
        }
        StatusType::Error => {
            Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .bg(Color::Rgb(180, 60, 60))
                .add_modifier(Modifier::BOLD)
        }
    };

    let status_bar = Paragraph::new(status_text)
        .style(style)
        .alignment(Alignment::Left);
    f.render_widget(status_bar, area);
}

/// Render the quick navigation popup.
pub fn render_quick_nav_popup(
    f: &mut Frame,
    locations: &[QuickNavLocation],
    selected_index: usize,
) {
    let area = f.area();

    // Calculate popup size - make it centered and appropriately sized
    let popup_width = 50.min(area.width.saturating_sub(4));
    let popup_height = (locations.len() as u16 + 4).min(area.height.saturating_sub(4));

    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;

    let popup_area = ratatui::layout::Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area behind the popup
    f.render_widget(Clear, popup_area);

    // Build the list items
    let items: Vec<ListItem> = locations
        .iter()
        .enumerate()
        .map(|(i, loc)| {
            let path_str = if loc.is_virtual {
                "(virtual)".to_string()
            } else if let Some(ref path) = loc.path {
                path.display().to_string()
            } else {
                String::new()
            };

            // Truncate path if too long
            let max_path_len = popup_width.saturating_sub(10) as usize;
            let display_path = if path_str.len() > max_path_len {
                format!("...{}", &path_str[path_str.len().saturating_sub(max_path_len - 3)..])
            } else {
                path_str
            };

            let style = if i == selected_index {
                Style::default()
                    .fg(Color::Rgb(30, 30, 30))
                    .bg(Color::Rgb(140, 180, 120))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(185, 177, 160))
            };

            let line = Line::from(vec![
                Span::styled(format!(" {} ", loc.icon), style),
                Span::styled(format!("{:<12}", loc.name), style.add_modifier(Modifier::BOLD)),
                Span::styled(display_path, style),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = ratatui::widgets::List::new(items)
        .block(
            Block::bordered()
                .title(" Quick Nav (G) ")
                .title_style(Style::default().fg(Color::Rgb(140, 180, 120)).add_modifier(Modifier::BOLD))
                .border_style(Style::default().fg(Color::Rgb(100, 100, 100)))
                .style(Style::default().bg(Color::Rgb(45, 45, 45)))
        )
        .highlight_style(Style::default());

    f.render_widget(list, popup_area);
}
