//! Rusty Files - A terminal-based file explorer.
//!
//! This application provides a feature-rich file explorer with fuzzy finding,
//! tree view navigation, file operations, and more.

mod clipboard_file;
mod explorer;
mod file_operations;
mod fuzzy_find;
mod types;
mod ui;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind, EnableMouseCapture, DisableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListState, Paragraph},
    Terminal,
};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crate::explorer::FileExplorer;
use crate::file_operations::{
    perform_delete_sudo, perform_file_operation_sudo, perform_rename_sudo, perform_undo_sudo,
};
use crate::fuzzy_find::build_file_cache_static;
use crate::types::{CachedFile, CreationType, OperationType, UIMode, UndoAction};
use crate::ui::{
    build_tree_items, format_disk_info, get_file_icon, format_permissions,
    render_create_dialog, render_delete_dialog, render_help_screen,
    render_password_dialog, render_quick_nav_popup, render_rename_dialog, render_setup_guide,
    render_status_bar,
};

/// Run the main application loop.
fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    mut explorer: FileExplorer,
) -> io::Result<PathBuf> {
    // Debouncing for fuzzy find search
    let mut last_search_update: Option<Instant> = None;
    let mut pending_search: bool = false;
    let debounce_ms = 150;

    // Build initial fuzzy cache on startup in background from current directory
    let mut cache_receiver: Option<Receiver<Arc<Vec<CachedFile>>>> = {
        let (sender, receiver) = mpsc::channel();
        let cache_dir = explorer.current_dir.clone();
        let show_hidden = explorer.show_hidden;

        thread::spawn(move || {
            let mut file_cache = Vec::new();
            build_file_cache_static(&cache_dir, Some(8), 0, &mut file_cache, show_hidden, Some(sender));
        });

        Some(receiver)
    };
    let mut cache_complete = false;
    let mut needs_redraw = true;

    loop {
        // Check if cut operation has been completed in another instance
        if explorer.has_active_cut() && explorer.check_cut_completion() {
            needs_redraw = true;
            if let Err(e) = explorer.load_directory() {
                explorer.show_error(format!("Failed to refresh directory: {}", e));
            } else {
                explorer.show_status("Directory refreshed after cut operation".to_string());
            }
        }

        // Check if cache is ready from background thread
        if let Some(ref receiver) = cache_receiver {
            match receiver.try_recv() {
                Ok(file_cache) => {
                    // Received incremental update
                    needs_redraw = true;
                    explorer.fuzzy_cache = file_cache.clone();

                    let (search_term_clone, should_search) = if let UIMode::FuzzyFind { search_term, file_cache: cache, .. } = &mut explorer.ui_mode {
                        *cache = file_cache.clone();
                        (Some(search_term.clone()), true)
                    } else {
                        (None, false)
                    };

                    if should_search {
                        if let Some(term) = search_term_clone {
                            if !term.is_empty() {
                                // Use the just-updated fuzzy_cache (file_cache variable points to it)
                                let new_matches = explorer.perform_fuzzy_search(&term, &explorer.fuzzy_cache);
                                if let UIMode::FuzzyFind { matches, selected_index, .. } = &mut explorer.ui_mode {
                                    *matches = new_matches;
                                    // Clamp selected_index to valid range
                                    if matches.is_empty() {
                                        *selected_index = 0;
                                    } else if *selected_index >= matches.len() {
                                        *selected_index = matches.len() - 1;
                                    }
                                }
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // Channel closed, cache building complete
                    needs_redraw = true;
                    cache_receiver = None;
                    cache_complete = true;

                    // Trigger one final search with the completed cache
                    if let UIMode::FuzzyFind { search_term, .. } = &explorer.ui_mode {
                        if !search_term.is_empty() {
                            pending_search = true;
                            last_search_update = Some(Instant::now());
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // No update yet, keep waiting
                }
            }
        }

        // Debounced fuzzy find search
        if pending_search {
            if let Some(last_update) = last_search_update {
                let elapsed = last_update.elapsed().as_millis();
                if elapsed >= debounce_ms as u128 {
                    let search_term_clone = if let UIMode::FuzzyFind { search_term, .. } = &explorer.ui_mode {
                        Some(search_term.clone())
                    } else {
                        None
                    };

                    if let Some(term) = search_term_clone {
                        if !term.is_empty() {
                            // Always use the latest global cache, not the stale UIMode cache
                            let new_matches = explorer.perform_fuzzy_search(&term, &explorer.fuzzy_cache);
                            if let UIMode::FuzzyFind { matches, selected_index, .. } = &mut explorer.ui_mode {
                                *matches = new_matches;
                                // Clamp selected_index to valid range
                                if matches.is_empty() {
                                    *selected_index = 0;
                                } else if *selected_index >= matches.len() {
                                    *selected_index = matches.len() - 1;
                                }
                            }
                        }
                    }

                    pending_search = false;
                    last_search_update = None;
                    needs_redraw = true;
                }
            }
        }

        // Force full redraw if needed (e.g., after returning from editor)
        if explorer.needs_full_redraw {
            terminal.clear()?;
            explorer.needs_full_redraw = false;
            needs_redraw = true;
        }

        if needs_redraw {
        needs_redraw = false;
        terminal.draw(|f| {
            let area = f.area();

            let chunks = match &explorer.ui_mode {
                UIMode::Normal | UIMode::StatusMessage { .. } | UIMode::PasswordPrompt { .. } | UIMode::ConfirmDelete { .. } => Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(3),
                        Constraint::Length(1),
                    ])
                    .split(area)
                    .to_vec(),
                _ => Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(3),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .split(area)
                    .to_vec(),
            };

            let main_area = chunks[0];
            let status_bar_area = chunks[1];
            let visible_height = main_area.height.saturating_sub(2) as usize;
            let terminal_width = main_area.width as usize;

            explorer.terminal_width = terminal_width;

            let (tree_items, list_state, title) = if let UIMode::FuzzyFind { search_term, matches, selected_index, file_cache: _ } = &mut explorer.ui_mode {
                // Clamp selected_index BEFORE rendering to prevent crashes
                if !matches.is_empty() && *selected_index >= matches.len() {
                    *selected_index = matches.len() - 1;
                }

                let cache_building = !cache_complete;
                let cache_count = explorer.fuzzy_cache.len();

                let fuzzy_items: Vec<ratatui::widgets::ListItem> = if cache_building && cache_count == 0 {
                    vec![ratatui::widgets::ListItem::new(Line::from(vec![
                        Span::styled("Building file cache, please wait...", Style::default().fg(Color::Rgb(140, 180, 120)))
                    ]))]
                } else if cache_building {
                    vec![ratatui::widgets::ListItem::new(Line::from(vec![
                        Span::styled(format!("Scanning... {} files found", cache_count), Style::default().fg(Color::Rgb(140, 180, 120)))
                    ]))]
                } else if search_term.is_empty() {
                    vec![ratatui::widgets::ListItem::new(Line::from(vec![
                        Span::styled(format!("Start typing to search {} files...", cache_count), Style::default().fg(Color::Rgb(140, 180, 120)))
                    ]))]
                } else if matches.is_empty() {
                    vec![ratatui::widgets::ListItem::new(Line::from(vec![
                        Span::styled(format!("No matches for '{}' (searched {} files)", search_term, cache_count), Style::default().fg(Color::Rgb(140, 180, 120)))
                    ]))]
                } else {
                    matches
                        .iter()
                        .enumerate()
                        .rev()
                        .map(|(idx, fuzzy_match)| {
                            let is_selected = idx == *selected_index;
                            let icon = get_file_icon(&fuzzy_match.name, fuzzy_match.is_dir, fuzzy_match.permissions);

                            let mut spans = vec![Span::raw(format!("{} ", icon))];

                            let grey_color = Color::Rgb(120, 120, 117);
                            let green_color = Color::Rgb(140, 180, 120);
                            let bg_color = if is_selected { Some(Color::Rgb(50, 50, 50)) } else { None };

                            let chars: Vec<char> = fuzzy_match.display_path.chars().collect();
                            let mut last_pos = 0;

                            for &match_pos in &fuzzy_match.matched_positions {
                                // Bounds check to prevent panics
                                if match_pos >= chars.len() {
                                    continue;
                                }

                                if match_pos > last_pos {
                                    let non_matched: String = chars[last_pos..match_pos].iter().collect();
                                    let mut style = Style::default().fg(grey_color);
                                    if let Some(bg) = bg_color {
                                        style = style.bg(bg);
                                    }
                                    spans.push(Span::styled(non_matched, style));
                                }

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

                            ratatui::widgets::ListItem::new(Line::from(spans))
                        })
                        .collect()
                };

                let visual_selected = if cache_building || matches.is_empty() {
                    None
                } else if *selected_index >= matches.len() {
                    // Safety check: clamp to valid range
                    Some(0)
                } else {
                    Some(matches.len() - 1 - *selected_index)
                };

                let scroll_offset = if let Some(visual_pos) = visual_selected {
                    let scrolloff = 3;

                    if visible_height > 0 && matches.len() > visible_height {
                        let max_offset = matches.len().saturating_sub(visible_height);

                        if visual_pos < scrolloff {
                            0
                        } else if visual_pos + scrolloff >= matches.len() {
                            max_offset
                        } else {
                            let ideal_offset = visual_pos.saturating_sub(scrolloff);
                            ideal_offset.min(max_offset)
                        }
                    } else {
                        0
                    }
                } else {
                    0
                };

                let list_state = ListState::default()
                    .with_selected(visual_selected)
                    .with_offset(scroll_offset);

                let title = if cache_building {
                    format!("Fuzzy Find: {} (scanning... {} files)", search_term, cache_count)
                } else {
                    format!("Fuzzy Find: {} ({} matches from {} files)", search_term, matches.len(), cache_count)
                };
                (fuzzy_items, list_state, title)
            } else {
                let tree_lines = explorer.build_tree_lines(terminal_width);
                explorer.calculate_scroll_offset(visible_height, &tree_lines);

                let tree_items = build_tree_items(&tree_lines);

                let cursor_line_idx = explorer.get_cursor_line_index(terminal_width);
                let list_state = ListState::default()
                    .with_selected(Some(cursor_line_idx))
                    .with_offset(explorer.scroll_offset);

                let disk_info = format_disk_info(&explorer.current_dir);
                let title = format!(" {}", disk_info);
                (tree_items, list_state, title)
            };

            let title_style = Style::default()
                .fg(Color::Rgb(65, 65, 65))
                .add_modifier(Modifier::BOLD);

            let tree_list = List::new(tree_items)
                .block(
                    Block::default()
                        .title(Span::styled(title, title_style))
                );

            let mut list_state = list_state;
            f.render_stateful_widget(tree_list, main_area, &mut list_state);

            render_status_bar(f, status_bar_area, &explorer);

            if chunks.len() > 2 {
                match &explorer.ui_mode {
                    UIMode::PasswordPrompt { prompt, password, .. } => {
                        render_password_dialog(f, chunks[2], prompt, password);
                    }
                    UIMode::StatusMessage { message } => {
                        let para = Paragraph::new(message.as_str())
                            .block(Block::default().title("Status"))
                            .style(Style::default().fg(Color::Rgb(170, 160, 145)))
                            .alignment(Alignment::Left);
                        f.render_widget(para, chunks[2]);
                    }
                    UIMode::ConfirmDelete { items } => {
                        render_delete_dialog(f, chunks[2], items.len());
                    }
                    UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } => {
                        render_rename_dialog(f, chunks[2], new_name, *cursor_pos, *selection_start);
                    }
                    UIMode::CreateNew { creation_type, name } => {
                        render_create_dialog(f, chunks[2], creation_type, name);
                    }
                    _ => {}
                }
            }

            if matches!(explorer.ui_mode, UIMode::Help) {
                render_help_screen(f, area, explorer.help_scroll_offset);
            }

            // Render quick nav popup
            if let UIMode::QuickNav { ref locations, selected_index } = explorer.ui_mode {
                render_quick_nav_popup(f, locations, selected_index);
            }

            // Render setup guide popup
            if let UIMode::SetupGuide { ref message } = explorer.ui_mode {
                render_setup_guide(f, message);
            }
        })?;
        } // needs_redraw

        // Auto-dismiss status messages after 3 seconds
        if let Some(time) = explorer.status_message_time {
            if time.elapsed() > std::time::Duration::from_secs(3) {
                explorer.clear_status();
                needs_redraw = true;
            }
        }

        if event::poll(std::time::Duration::from_millis(100))? {
            needs_redraw = true;
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if explorer.status_message.is_some() {
                        explorer.status_message = None;
                        explorer.status_message_time = None;
                    }
                    if matches!(explorer.ui_mode, UIMode::StatusMessage { .. }) {
                        explorer.clear_status();
                    }

                    match &explorer.ui_mode.clone() {
                        UIMode::PasswordPrompt { prompt: _, password, pending_operation } => {
                            match key.code {
                                KeyCode::Char(c) => {
                                    if let UIMode::PasswordPrompt { password, .. } = &mut explorer.ui_mode {
                                        password.push(c);
                                    }
                                }
                                KeyCode::Backspace => {
                                    if let UIMode::PasswordPrompt { password, .. } = &mut explorer.ui_mode {
                                        password.pop();
                                    }
                                }
                                KeyCode::Enter => {
                                    let op = pending_operation.clone();
                                    let pwd = password.clone();
                                    explorer.ui_mode = UIMode::Normal;

                                    match &op.operation {
                                        OperationType::Copy | OperationType::Move => {
                                            let is_move = matches!(op.operation, OperationType::Move);
                                            if let Some(dest) = &op.destination {
                                                let is_rename = op.items.len() == 1
                                                    && op.items[0].parent() == dest.parent();

                                                if is_rename {
                                                    let original_path = &op.items[0];
                                                    let new_name = dest.file_name()
                                                        .and_then(|n| n.to_str())
                                                        .unwrap_or("")
                                                        .to_string();

                                                    match perform_rename_sudo(original_path, dest, &pwd) {
                                                        Ok(_) => {
                                                            explorer.show_status(format!("Renamed to '{}' with sudo", new_name));
                                                            explorer.undo_stack.push(UndoAction::Rename {
                                                                original_path: original_path.clone(),
                                                                new_path: dest.clone(),
                                                            });
                                                            explorer.size_cache.remove(original_path);
                                                            explorer.load_directory()?;
                                                            explorer.select_items_by_name(&[new_name]);
                                                        }
                                                        Err(e) => {
                                                            explorer.show_error(format!("Error: {}", e));
                                                        }
                                                    }
                                                } else {
                                                    let pasted_names: Vec<String> = op.items.iter()
                                                        .filter_map(|p| p.file_name())
                                                        .filter_map(|n| n.to_str())
                                                        .map(|s| s.to_string())
                                                        .collect();

                                                    match perform_file_operation_sudo(&op.items, dest, is_move, &pwd) {
                                                        Ok(count) => {
                                                            if is_move {
                                                                explorer.clipboard = None;
                                                            }
                                                            explorer.show_status(format!("Pasted {} item(s) with sudo", count));
                                                            explorer.load_directory()?;
                                                            explorer.select_items_by_name(&pasted_names);
                                                        }
                                                        Err(e) => {
                                                            explorer.show_error(format!("Error: {}", e));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        OperationType::Delete => {
                                            match perform_delete_sudo(&op.items, &pwd) {
                                                Ok(deleted_files) => {
                                                    let count = deleted_files.len();
                                                    explorer.undo_stack.push(UndoAction::Delete { deleted_files });
                                                    explorer.show_status(format!("Deleted {} item(s) with elevated privileges", count));
                                                    explorer.selected_indices.clear();
                                                    explorer.selection_anchor = None;
                                                    explorer.load_directory()?;
                                                }
                                                Err(e) => {
                                                    explorer.show_error(format!("Error: {}", e));
                                                }
                                            }
                                        }
                                        OperationType::Undo => {
                                            if let Some(undo_action) = &op.undo_action {
                                                match perform_undo_sudo(undo_action, &pwd) {
                                                    Ok(count) => {
                                                        explorer.undo_stack.pop();
                                                        let msg = match undo_action {
                                                            UndoAction::Copy { .. } => format!("Undone copy: removed {} item(s) with sudo", count),
                                                            UndoAction::Move { .. } => format!("Undone move: restored {} item(s) with sudo", count),
                                                            UndoAction::Delete { .. } => format!("Undone delete: restored {} item(s) with sudo", count),
                                                            UndoAction::Rename { original_path, .. } => {
                                                                let name = original_path.file_name()
                                                                    .and_then(|n| n.to_str())
                                                                    .unwrap_or("");
                                                                format!("Undone rename: restored to '{}' with sudo", name)
                                                            }
                                                        };
                                                        explorer.show_status(msg);
                                                        explorer.load_directory()?;
                                                    }
                                                    Err(e) => {
                                                        explorer.show_error(format!("Error: {}", e));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                }
                                _ => {}
                            }
                        }
                        UIMode::ConfirmDelete { items } => {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    let items_to_delete = items.clone();
                                    explorer.ui_mode = UIMode::Normal;

                                    match explorer.perform_delete(&items_to_delete) {
                                        Ok(_) => {}
                                        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                                            explorer.ui_mode = UIMode::PasswordPrompt {
                                                prompt: "Permission denied. Enter sudo password:".to_string(),
                                                password: String::new(),
                                                pending_operation: Box::new(types::PendingOperation {
                                                    items: items_to_delete,
                                                    destination: None,
                                                    operation: OperationType::Delete,
                                                    undo_action: None,
                                                }),
                                            };
                                        }
                                        Err(e) => {
                                            explorer.show_error(format!("Error: {}", e));
                                        }
                                    }
                                }
                                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                }
                                _ => {}
                            }
                        }
                        UIMode::RenameItem { original_path, new_name, .. } => {
                            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

                            match key.code {
                                KeyCode::Char(c) if !ctrl => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if let Some(sel_start) = selection_start {
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            new_name.replace_range(start..end, "");
                                            *cursor_pos = start;
                                            *selection_start = None;
                                        }
                                        new_name.insert(*cursor_pos, c);
                                        *cursor_pos += 1;
                                    }
                                }
                                KeyCode::Char('a') if ctrl => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        *selection_start = Some(0);
                                        *cursor_pos = new_name.len();
                                    }
                                }
                                KeyCode::Char('c') if ctrl => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &explorer.ui_mode {
                                        if let Some(sel_start) = selection_start {
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            if start < end {
                                                let selected_text = new_name[start..end].to_string();
                                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                                    let _ = clipboard.set_text(selected_text);
                                                }
                                            }
                                        }
                                    }
                                }
                                KeyCode::Char('v') if ctrl => {
                                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                        if let Ok(clipboard_text) = clipboard.get_text() {
                                            if !clipboard_text.is_empty() {
                                                if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                                    if let Some(sel_start) = selection_start {
                                                        let start = (*sel_start).min(*cursor_pos);
                                                        let end = (*sel_start).max(*cursor_pos);
                                                        new_name.replace_range(start..end, "");
                                                        *cursor_pos = start;
                                                        *selection_start = None;
                                                    }
                                                    new_name.insert_str(*cursor_pos, &clipboard_text);
                                                    *cursor_pos += clipboard_text.len();
                                                }
                                            }
                                        }
                                    }
                                }
                                KeyCode::Char('x') if ctrl => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if let Some(sel_start) = selection_start {
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            if start < end {
                                                let selected_text = new_name[start..end].to_string();
                                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                                    let _ = clipboard.set_text(selected_text);
                                                }
                                                new_name.replace_range(start..end, "");
                                                *cursor_pos = start;
                                                *selection_start = None;
                                            }
                                        }
                                    }
                                }
                                KeyCode::Left => {
                                    if let UIMode::RenameItem { cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if shift {
                                            if selection_start.is_none() {
                                                *selection_start = Some(*cursor_pos);
                                            }
                                            if *cursor_pos > 0 {
                                                *cursor_pos -= 1;
                                            }
                                        } else {
                                            *selection_start = None;
                                            if *cursor_pos > 0 {
                                                *cursor_pos -= 1;
                                            }
                                        }
                                    }
                                }
                                KeyCode::Right => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if shift {
                                            if selection_start.is_none() {
                                                *selection_start = Some(*cursor_pos);
                                            }
                                            if *cursor_pos < new_name.len() {
                                                *cursor_pos += 1;
                                            }
                                        } else {
                                            *selection_start = None;
                                            if *cursor_pos < new_name.len() {
                                                *cursor_pos += 1;
                                            }
                                        }
                                    }
                                }
                                KeyCode::Home => {
                                    if let UIMode::RenameItem { cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if shift {
                                            if selection_start.is_none() {
                                                *selection_start = Some(*cursor_pos);
                                            }
                                        } else {
                                            *selection_start = None;
                                        }
                                        *cursor_pos = 0;
                                    }
                                }
                                KeyCode::End => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if shift {
                                            if selection_start.is_none() {
                                                *selection_start = Some(*cursor_pos);
                                            }
                                        } else {
                                            *selection_start = None;
                                        }
                                        *cursor_pos = new_name.len();
                                    }
                                }
                                KeyCode::Backspace => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if let Some(sel_start) = selection_start {
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            new_name.replace_range(start..end, "");
                                            *cursor_pos = start;
                                            *selection_start = None;
                                        } else if *cursor_pos > 0 {
                                            new_name.remove(*cursor_pos - 1);
                                            *cursor_pos -= 1;
                                        }
                                    }
                                }
                                KeyCode::Delete => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if let Some(sel_start) = selection_start {
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            new_name.replace_range(start..end, "");
                                            *cursor_pos = start;
                                            *selection_start = None;
                                        } else if *cursor_pos < new_name.len() {
                                            new_name.remove(*cursor_pos);
                                        }
                                    }
                                }
                                KeyCode::Enter => {
                                    let path = original_path.clone();
                                    let name = new_name.clone();
                                    explorer.ui_mode = UIMode::Normal;

                                    if let Err(e) = explorer.rename_item(path, name) {
                                        explorer.show_error(format!("Error: {}", e));
                                    }
                                }
                                KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                }
                                _ => {}
                            }
                        }
                        UIMode::CreateNew { creation_type, name } => {
                            match key.code {
                                KeyCode::Char(c) if creation_type.is_none() => {
                                    match c {
                                        'f' | 'F' => {
                                            if let UIMode::CreateNew { creation_type, .. } = &mut explorer.ui_mode {
                                                *creation_type = Some(CreationType::File);
                                            }
                                        }
                                        'd' | 'D' => {
                                            if let UIMode::CreateNew { creation_type, .. } = &mut explorer.ui_mode {
                                                *creation_type = Some(CreationType::Directory);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                KeyCode::Char(c) if creation_type.is_some() => {
                                    if let UIMode::CreateNew { name, .. } = &mut explorer.ui_mode {
                                        name.push(c);
                                    }
                                }
                                KeyCode::Backspace if creation_type.is_some() => {
                                    if let UIMode::CreateNew { name, .. } = &mut explorer.ui_mode {
                                        name.pop();
                                    }
                                }
                                KeyCode::Enter if creation_type.is_some() => {
                                    let ctype = creation_type.clone().unwrap();
                                    let item_name = name.clone();
                                    explorer.ui_mode = UIMode::Normal;

                                    if let Err(e) = explorer.create_new_item(ctype, item_name) {
                                        explorer.show_error(format!("Error: {}", e));
                                    }
                                }
                                KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                }
                                _ => {}
                            }
                        }
                        UIMode::Help => {
                            match key.code {
                                KeyCode::F(1) | KeyCode::Esc => {
                                    explorer.toggle_help();
                                }
                                KeyCode::Up => {
                                    explorer.scroll_help_up();
                                }
                                KeyCode::Down => {
                                    explorer.scroll_help_down();
                                }
                                _ => {}
                            }
                        }
                        UIMode::FuzzyFind { .. } => {
                            match key.code {
                                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    return Ok(explorer.current_dir.clone());
                                }
                                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    if let UIMode::FuzzyFind { matches, selected_index, .. } = &explorer.ui_mode {
                                        if let Some(selected) = matches.get(*selected_index) {
                                            let full_path = selected.path.display().to_string();
                                            match arboard::Clipboard::new() {
                                                Ok(mut clipboard) => {
                                                    match clipboard.set_text(&full_path) {
                                                        Ok(_) => {
                                                            drop(clipboard);
                                                            explorer.show_status(format!("Copied to clipboard: {}", full_path));
                                                        }
                                                        Err(e) => {
                                                            explorer.show_error(format!("Failed to set clipboard: {}", e));
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    explorer.show_status(format!("Clipboard error: {}. Install xsel, xclip, or wl-clipboard", e));
                                                }
                                            }
                                        }
                                    }
                                }
                                KeyCode::Char(c) => {
                                    if let UIMode::FuzzyFind { search_term, .. } = &mut explorer.ui_mode {
                                        search_term.push(c);
                                        pending_search = true;
                                        last_search_update = Some(Instant::now());
                                    }
                                }
                                KeyCode::Backspace => {
                                    if let UIMode::FuzzyFind { search_term, .. } = &mut explorer.ui_mode {
                                        search_term.pop();
                                        pending_search = true;
                                        last_search_update = Some(Instant::now());
                                    }
                                }
                                KeyCode::Up => {
                                    if let UIMode::FuzzyFind { matches, selected_index, .. } = &mut explorer.ui_mode {
                                        if *selected_index + 1 < matches.len() {
                                            *selected_index += 1;
                                        }
                                    }
                                }
                                KeyCode::Down => {
                                    if let UIMode::FuzzyFind { selected_index, .. } = &mut explorer.ui_mode {
                                        if *selected_index > 0 {
                                            *selected_index -= 1;
                                        }
                                    }
                                }
                                KeyCode::Enter => {
                                    if let UIMode::FuzzyFind { matches, selected_index, .. } = &explorer.ui_mode {
                                        if let Some(selected) = matches.get(*selected_index) {
                                            let path = selected.path.clone();
                                            let is_dir = selected.is_dir;
                                            explorer.ui_mode = UIMode::Normal;
                                            pending_search = false;
                                            last_search_update = None;

                                            if is_dir {
                                                explorer.current_dir = path;
                                                explorer.load_directory()?;
                                            } else {
                                                if let Some(parent) = path.parent() {
                                                    explorer.current_dir = parent.to_path_buf();
                                                    explorer.load_directory()?;

                                                    if let Some(file_name) = path.file_name() {
                                                        if let Some(name_str) = file_name.to_str() {
                                                            for (i, entry) in explorer.entries.iter().enumerate() {
                                                                if entry.name == name_str {
                                                                    explorer.cursor_index = i;
                                                                    break;
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                    pending_search = false;
                                    last_search_update = None;
                                }
                                _ => {}
                            }
                        }
                        UIMode::QuickNav { locations, selected_index } => {
                            let selected_index = *selected_index;
                            match key.code {
                                KeyCode::Up => {
                                    if selected_index > 0 {
                                        explorer.ui_mode = UIMode::QuickNav {
                                            locations: locations.clone(),
                                            selected_index: selected_index - 1,
                                        };
                                    }
                                }
                                KeyCode::Down => {
                                    if selected_index < locations.len().saturating_sub(1) {
                                        explorer.ui_mode = UIMode::QuickNav {
                                            locations: locations.clone(),
                                            selected_index: selected_index + 1,
                                        };
                                    }
                                }
                                KeyCode::Enter => {
                                    if let Some(location) = locations.get(selected_index) {
                                        if location.is_virtual {
                                            // Trash is virtual - show status message
                                            explorer.ui_mode = UIMode::Normal;
                                            explorer.show_status("Trash view not yet implemented".to_string());
                                        } else if let Some(ref path) = location.path {
                                            if path.exists() {
                                                let nav_path = path.clone();
                                                explorer.ui_mode = UIMode::Normal;
                                                explorer.current_dir = nav_path;
                                                explorer.load_directory()?;
                                            } else {
                                                explorer.ui_mode = UIMode::Normal;
                                                explorer.show_status(format!("Path does not exist: {}", path.display()));
                                            }
                                        } else {
                                            explorer.ui_mode = UIMode::Normal;
                                            explorer.show_status("No path available".to_string());
                                        }
                                    }
                                }
                                KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                }
                                _ => {}
                            }
                        }
                        UIMode::SetupGuide { .. } => {
                            // Dismiss on any key
                            explorer.ui_mode = UIMode::Normal;
                        }
                        UIMode::Normal | UIMode::StatusMessage { .. } => {
                            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

                            match key.code {
                                KeyCode::F(1) => {
                                    explorer.toggle_help();
                                }
                                KeyCode::Char('q') if ctrl => return Ok(explorer.current_dir.clone()),
                                KeyCode::Char('l') if ctrl => {
                                    terminal.clear()?;
                                }
                                KeyCode::Char('t') if ctrl => {
                                    let dir = explorer.current_dir.clone();

                                    let dir_str = match dir.to_str() {
                                        Some(s) => s,
                                        None => {
                                            explorer.show_status("Invalid directory path".to_string());
                                            continue;
                                        }
                                    };

                                    let result = if cfg!(target_os = "windows") {
                                        // Use -p to specify profile (for colors) and Set-Location (for directory)
                                        // Note: Use windowingBehavior setting in WT to control new window vs tab
                                        let ps_escaped = dir_str.replace("'", "''");
                                        let wt_result = std::process::Command::new("wt")
                                            .args([
                                                "-p",
                                                "PowerShell",
                                                "--",
                                                "powershell",
                                                "-NoExit",
                                                "-NoLogo",
                                                "-Command",
                                                &format!("Set-Location -LiteralPath '{}'", ps_escaped),
                                            ])
                                            .stdin(std::process::Stdio::null())
                                            .stdout(std::process::Stdio::null())
                                            .stderr(std::process::Stdio::null())
                                            .spawn();

                                        if wt_result.is_err() {
                                            // Fall back to cmd.exe - start new window in directory
                                            // Escape special cmd characters
                                            let cmd_escaped = dir_str.replace("&", "^&")
                                                .replace("|", "^|")
                                                .replace("<", "^<")
                                                .replace(">", "^>");
                                            std::process::Command::new("cmd")
                                                .arg("/c")
                                                .arg("start")
                                                .arg("")
                                                .arg("cmd")
                                                .arg("/k")
                                                .arg(format!("cd /d \"{}\"", cmd_escaped))
                                                .stdin(std::process::Stdio::null())
                                                .stdout(std::process::Stdio::null())
                                                .stderr(std::process::Stdio::null())
                                                .spawn()
                                        } else {
                                            wt_result
                                        }
                                    } else if cfg!(target_os = "macos") {
                                        let terminal_app = std::env::var("TERMINAL")
                                            .unwrap_or_else(|_| "Terminal".to_string());

                                        if terminal_app == "Terminal" {
                                            let script = format!(
                                                "tell application \"Terminal\" to do script \"cd '{}' && clear\"",
                                                dir_str.replace("'", "'\\''")
                                            );
                                            std::process::Command::new("osascript")
                                                .arg("-e")
                                                .arg(&script)
                                                .stdin(std::process::Stdio::null())
                                                .stdout(std::process::Stdio::null())
                                                .stderr(std::process::Stdio::null())
                                                .spawn()
                                        } else if terminal_app.to_lowercase().contains("iterm") {
                                            let script = format!(
                                                "tell application \"iTerm\" to create window with default profile command \"cd '{}' && clear\"",
                                                dir_str.replace("'", "'\\''")
                                            );
                                            std::process::Command::new("osascript")
                                                .arg("-e")
                                                .arg(&script)
                                                .stdin(std::process::Stdio::null())
                                                .stdout(std::process::Stdio::null())
                                                .stderr(std::process::Stdio::null())
                                                .spawn()
                                        } else {
                                            std::process::Command::new("open")
                                                .arg("-n")
                                                .arg("-a")
                                                .arg(&terminal_app)
                                                .arg("--args")
                                                .arg(dir_str)
                                                .stdin(std::process::Stdio::null())
                                                .stdout(std::process::Stdio::null())
                                                .stderr(std::process::Stdio::null())
                                                .spawn()
                                        }
                                    } else {
                                        // Linux/Unix
                                        let terminal_cmd = std::env::var("TERMINAL")
                                            .unwrap_or_else(|_| "kitty".to_string());

                                        let command = format!("cd '{}' && setsid -f {} >/dev/null 2>&1", dir_str, terminal_cmd);
                                        std::process::Command::new("sh")
                                            .arg("-c")
                                            .arg(&command)
                                            .stdin(std::process::Stdio::null())
                                            .stdout(std::process::Stdio::null())
                                            .stderr(std::process::Stdio::null())
                                            .spawn()
                                    };

                                    match result {
                                        Ok(_) => {
                                            explorer.show_status("Opened terminal".to_string());
                                        }
                                        Err(e) => {
                                            explorer.show_error(format!("Failed to open terminal: {}", e));
                                        }
                                    }
                                }
                                KeyCode::Char('e') if ctrl => {
                                    let dir = explorer.current_dir.clone();

                                    let dir_str = match dir.to_str() {
                                        Some(s) => s,
                                        None => {
                                            explorer.show_status("Invalid directory path".to_string());
                                            continue;
                                        }
                                    };

                                    let result = if cfg!(target_os = "windows") {
                                        // Windows: Use explorer.exe with proper path format
                                        // Convert forward slashes to backslashes for Windows
                                        let windows_path = dir_str.replace("/", "\\");
                                        std::process::Command::new("explorer")
                                            .arg(&windows_path)
                                            .stdin(std::process::Stdio::null())
                                            .stdout(std::process::Stdio::null())
                                            .stderr(std::process::Stdio::null())
                                            .spawn()
                                    } else if cfg!(target_os = "macos") {
                                        // macOS: Use open -a Finder
                                        std::process::Command::new("open")
                                            .arg("-a")
                                            .arg("Finder")
                                            .arg(dir_str)
                                            .stdin(std::process::Stdio::null())
                                            .stdout(std::process::Stdio::null())
                                            .stderr(std::process::Stdio::null())
                                            .spawn()
                                    } else {
                                        // Linux/Unix: Use xdg-open
                                        std::process::Command::new("xdg-open")
                                            .arg(dir_str)
                                            .stdin(std::process::Stdio::null())
                                            .stdout(std::process::Stdio::null())
                                            .stderr(std::process::Stdio::null())
                                            .spawn()
                                    };

                                    match result {
                                        Ok(_) => {
                                            explorer.show_status("Opened file explorer".to_string());
                                        }
                                        Err(e) => {
                                            explorer.show_error(format!("Failed to open file explorer: {}", e));
                                        }
                                    }
                                }
                                KeyCode::Up => explorer.move_up(shift),
                                KeyCode::Down => explorer.move_down(shift),
                                KeyCode::Enter => explorer.open_or_enter()?,
                                KeyCode::Right => explorer.enter_directory()?,
                                KeyCode::Left => explorer.go_to_parent()?,
                                KeyCode::Char(' ') if ctrl => {
                                    explorer.toggle_selection();
                                }
                                KeyCode::Char('c') if ctrl => {
                                    explorer.copy_selected();
                                }
                                KeyCode::Char('x') if ctrl => {
                                    explorer.cut_selected();
                                }
                                KeyCode::Char('v') if ctrl => {
                                    explorer.paste()?;
                                }
                                KeyCode::Char('n') if ctrl => {
                                    explorer.start_create_new();
                                }
                                KeyCode::Char('r') if ctrl => {
                                    explorer.start_rename();
                                }
                                KeyCode::Delete => {
                                    explorer.delete_selected();
                                }
                                KeyCode::Char('d') if ctrl => {
                                    if let Some(entry) = explorer.entries.get(explorer.cursor_index) {
                                        let full_path = entry.path.display().to_string();
                                        match arboard::Clipboard::new() {
                                            Ok(mut clipboard) => {
                                                match clipboard.set_text(&full_path) {
                                                    Ok(_) => {
                                                        drop(clipboard);
                                                        explorer.show_status(format!("Copied to clipboard: {}", full_path));
                                                    }
                                                    Err(e) => {
                                                        explorer.show_error(format!("Failed to set clipboard: {}", e));
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                explorer.show_status(format!("Clipboard error: {}. Install xsel, xclip, or wl-clipboard", e));
                                            }
                                        }
                                    }
                                }
                                KeyCode::Char('z') if ctrl => {
                                    explorer.undo()?;
                                }
                                KeyCode::Char('s') if ctrl => {
                                    explorer.toggle_sort_mode()?;
                                }
                                KeyCode::Char('h') if ctrl => {
                                    explorer.toggle_hidden()?;
                                }
                                KeyCode::Char('f') if ctrl => {
                                    // Start fuzzy find mode with current cached files filtered to current dir
                                    let filtered_cache: Vec<CachedFile> = explorer.fuzzy_cache
                                        .iter()
                                        .filter(|f| f.path.starts_with(&explorer.current_dir))
                                        .cloned()
                                        .collect();

                                    explorer.ui_mode = UIMode::FuzzyFind {
                                        search_term: String::new(),
                                        matches: Vec::new(),
                                        selected_index: 0,
                                        file_cache: Arc::new(filtered_cache),
                                    };

                                    // Rebuild cache for current directory in background
                                    let (sender, receiver) = mpsc::channel();
                                    let current_dir = explorer.current_dir.clone();
                                    let show_hidden = explorer.show_hidden;
                                    thread::spawn(move || {
                                        let mut fresh_cache = Vec::new();
                                        build_file_cache_static(&current_dir, Some(8), 0, &mut fresh_cache, show_hidden, Some(sender));
                                    });
                                    cache_receiver = Some(receiver);
                                    cache_complete = false;
                                }
                                KeyCode::Char('g') if ctrl => {
                                    explorer.open_quick_nav();
                                }
                                KeyCode::Esc => {
                                    if explorer.has_active_cut() {
                                        explorer.cancel_cut_operation();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    if matches!(explorer.ui_mode, UIMode::Help) {
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                explorer.scroll_help_up();
                            }
                            MouseEventKind::ScrollDown => {
                                explorer.scroll_help_down();
                            }
                            _ => {}
                        }
                    } else if matches!(explorer.ui_mode, UIMode::Normal) {
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                explorer.move_up(false);
                            }
                            MouseEventKind::ScrollDown => {
                                explorer.move_down(false);
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                explorer.handle_mouse_down(
                                    mouse.row,
                                    mouse.column,
                                    mouse.modifiers,
                                    0,
                                );
                            }
                            MouseEventKind::Drag(MouseButton::Left) => {
                                explorer.handle_mouse_drag(
                                    mouse.row,
                                    mouse.column,
                                    0,
                                );
                            }
                            MouseEventKind::Up(MouseButton::Left) => {
                                explorer.handle_mouse_up();
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Create shell wrapper scripts next to the executable if they don't already exist.
fn create_wrapper_scripts() {
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let exe_dir = match exe_path.parent() {
        Some(d) => d,
        None => return,
    };
    let lastdir = {
        let temp = std::fs::canonicalize(std::env::temp_dir())
            .unwrap_or_else(|_| std::env::temp_dir());
        // Strip \\?\ prefix that Windows canonicalize adds — cmd.exe can't handle it
        let s = temp.display().to_string();
        PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
    }.join("rusty_files_lastdir");

    if cfg!(target_os = "windows") {
        // rf.bat for cmd.exe / PowerShell
        let bat_path = exe_dir.join("rf.bat");
        if !bat_path.exists() {
            let bat_content = format!(
                "@echo off\r\n\
                 set RF_WRAPPER=1\r\n\
                 \"{exe}\" %*\r\n\
                 if exist \"{lastdir}\" (\r\n\
                     set /p RF_DIR=<\"{lastdir}\"\r\n\
                     cd /d \"%RF_DIR%\"\r\n\
                 )\r\n",
                exe = exe_path.display(),
                lastdir = lastdir.display(),
            );
            let _ = std::fs::write(&bat_path, bat_content);
        }
    } else {
        // rf shell script for bash/zsh
        let sh_path = exe_dir.join("rf");
        if !sh_path.exists() {
            let sh_content = format!(
                "#!/bin/sh\n\
                 export RF_WRAPPER=1\n\
                 \"{exe}\" \"$@\"\n\
                 lastdir=\"{lastdir}\"\n\
                 if [ -f \"$lastdir\" ]; then\n\
                     dir=\"$(cat \"$lastdir\")\"\n\
                     [ -d \"$dir\" ] && cd \"$dir\"\n\
                 fi\n",
                exe = exe_path.display(),
                lastdir = lastdir.display(),
            );
            let _ = std::fs::write(&sh_path, sh_content);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&sh_path, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
}

fn main() -> io::Result<()> {
    create_wrapper_scripts();

    // Prevent Ctrl+C from terminating the program on Windows
    // This allows crossterm to capture it as a key event instead
    ctrlc::set_handler(|| {}).expect("Error setting Ctrl-C handler");

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, crossterm::terminal::SetTitle("rusty_files"))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut explorer = FileExplorer::new()?;

    // Show setup guide if not launched from wrapper script
    if std::env::var("RF_WRAPPER").is_err() {
        let exe_path = std::env::current_exe().unwrap_or_default();
        let exe_dir = exe_path.parent().map(|p| p.display().to_string()).unwrap_or_default();

        let mut lines = vec![
            "rusty_files can cd your shell on exit,".to_string(),
            "but it needs to be launched via a wrapper.".to_string(),
            String::new(),
        ];

        if cfg!(target_os = "windows") {
            lines.extend([
                "For PowerShell, add this function to $PROFILE:".to_string(),
                String::new(),
                format!("  function rf {{"),
                format!("    $env:RF_WRAPPER = \"1\""),
                format!("    & \"{}\" @args", exe_path.display()),
                format!("    $f = \"$env:TEMP\\rusty_files_lastdir\""),
                format!("    if (Test-Path $f) {{"),
                format!("      $d = Get-Content $f"),
                format!("      if (Test-Path $d) {{ Set-Location $d }}"),
                format!("    }}"),
                format!("  }}"),
                String::new(),
                format!("For cmd.exe, use: {}\\rf.bat", exe_dir),
            ]);
        } else {
            lines.extend([
                "Add this function to your shell profile:".to_string(),
                String::new(),
                format!("  rf() {{"),
                format!("    export RF_WRAPPER=1"),
                format!("    \"{}\" \"$@\"", exe_path.display()),
                format!("    f=\"/tmp/rusty_files_lastdir\""),
                format!("    [ -f \"$f\" ] && cd \"$(cat \"$f\")\""),
                format!("  }}"),
            ]);
        }

        lines.push(String::new());
        lines.push("Press any key to continue...".to_string());

        explorer.ui_mode = UIMode::SetupGuide { message: lines };
    }

    let res = run_app(&mut terminal, explorer);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    match res {
        Ok(last_dir) => {
            // Write last directory to temp file so a shell wrapper can cd there
            let lastdir_path = {
                let temp = std::fs::canonicalize(std::env::temp_dir())
                    .unwrap_or_else(|_| std::env::temp_dir());
                let s = temp.display().to_string();
                PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
            }.join("rusty_files_lastdir");
            let _ = std::fs::write(&lastdir_path, last_dir.display().to_string());
        }
        Err(err) => {
            println!("Error: {:?}", err);
        }
    }

    Ok(())
}
