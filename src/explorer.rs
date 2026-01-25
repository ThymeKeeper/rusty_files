//! File Explorer module.
//!
//! This module contains the main FileExplorer struct and its implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    event::{DisableMouseCapture, EnableMouseCapture},
};

use crate::clipboard_file;
use crate::file_operations::{get_default_file_content, perform_file_operation_tracked};
use crate::fuzzy_find::{build_file_cache_static, perform_fuzzy_search};
use crate::types::{
    CachedFile, Clipboard, ClipboardOp, CreationType, DirEntry, DirState, FuzzyMatch,
    OperationType, PendingOperation, QuickNavLocation, SortMode, StatusType, TreeLine, UIMode, UndoAction,
};
use crate::ui::{format_date, format_file_size, format_permissions, get_file_icon};

/// The main file explorer state.
pub struct FileExplorer {
    pub current_dir: PathBuf,
    pub entries: Vec<DirEntry>,
    pub cursor_index: usize,
    pub selected_indices: HashSet<usize>,
    pub selection_anchor: Option<usize>,
    pub scroll_offset: usize,
    pub dir_memory: HashMap<PathBuf, DirState>,
    pub clipboard: Option<Clipboard>,
    pub ui_mode: UIMode,
    pub undo_stack: Vec<UndoAction>,
    pub drag_selection: Option<usize>,
    pub size_cache: HashMap<PathBuf, u64>,
    pub current_item_size: Option<u64>,
    pub sort_mode: SortMode,
    pub terminal_width: usize,
    pub show_hidden: bool,
    pub status_message: Option<(String, StatusType)>,
    pub status_message_time: Option<std::time::Instant>,
    pub fuzzy_cache: Arc<Vec<CachedFile>>,
    pub help_scroll_offset: usize,
    pub cut_operation_time: Option<std::time::Instant>,
    pub cut_items: Vec<PathBuf>,
    pub last_cut_poll_time: Option<std::time::Instant>,
}

impl FileExplorer {
    /// Create a new FileExplorer instance.
    pub fn new() -> io::Result<Self> {
        let current_dir = std::env::current_dir()?;

        let mut explorer = FileExplorer {
            current_dir: current_dir.clone(),
            entries: Vec::new(),
            cursor_index: 0,
            selected_indices: HashSet::new(),
            selection_anchor: None,
            scroll_offset: 0,
            dir_memory: HashMap::new(),
            clipboard: None,
            ui_mode: UIMode::Normal,
            undo_stack: Vec::new(),
            drag_selection: None,
            size_cache: HashMap::new(),
            current_item_size: None,
            sort_mode: SortMode::Name,
            terminal_width: 100,
            show_hidden: false,
            status_message: None,
            status_message_time: None,
            fuzzy_cache: Arc::new(Vec::new()),
            help_scroll_offset: 0,
            cut_operation_time: None,
            cut_items: Vec::new(),
            last_cut_poll_time: None,
        };
        explorer.load_directory()?;
        Ok(explorer)
    }

    /// Load the current directory.
    pub fn load_directory(&mut self) -> io::Result<()> {
        self.entries.clear();

        let mut entries = Vec::new();
        if let Ok(read_dir) = fs::read_dir(&self.current_dir) {
            for entry in read_dir.flatten() {
                if let (Ok(name), Ok(metadata)) = (
                    entry.file_name().into_string(),
                    entry.metadata()
                ) {
                    // Check for hidden files (Unix: dot prefix, Windows: hidden attribute)
                    let is_hidden_unix = name.starts_with('.');
                    #[cfg(windows)]
                    let is_hidden_windows = {
                        use std::os::windows::fs::MetadataExt;
                        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
                        const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
                        let attrs = metadata.file_attributes();
                        (attrs & FILE_ATTRIBUTE_HIDDEN != 0) || (attrs & FILE_ATTRIBUTE_SYSTEM != 0)
                    };
                    #[cfg(not(windows))]
                    let is_hidden_windows = false;

                    if !self.show_hidden && (is_hidden_unix || is_hidden_windows) {
                        continue;
                    }

                    let path = entry.path();
                    let is_dir = metadata.is_dir();

                    let modified = if is_dir {
                        Self::get_dir_max_modified(&path, 1)
                    } else {
                        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)
                    };

                    let size = if is_dir { 0 } else { metadata.len() };

                    #[cfg(unix)]
                    let permissions = metadata.permissions().mode();
                    #[cfg(not(unix))]
                    let permissions = if metadata.permissions().readonly() { 0o444 } else { 0o644 };

                    entries.push(DirEntry {
                        path,
                        name,
                        is_dir,
                        size,
                        modified,
                        permissions,
                    });
                }
            }
        }

        match self.sort_mode {
            SortMode::Name => {
                entries.sort_by(|a, b| {
                    match (a.is_dir, b.is_dir) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    }
                });
            }
            SortMode::Date => {
                entries.sort_by(|a, b| {
                    match (a.is_dir, b.is_dir) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => b.modified.cmp(&a.modified),
                    }
                });
            }
        }

        self.entries = entries;

        if let Some(state) = self.dir_memory.get(&self.current_dir) {
            self.cursor_index = state.cursor_index.min(self.entries.len().saturating_sub(1));
            self.selected_indices = state.selected_indices.clone();
            self.scroll_offset = state.scroll_offset;
        } else {
            self.cursor_index = 0;
            self.selected_indices.clear();
            self.scroll_offset = 0;
        }

        self.selection_anchor = None;
        self.size_cache.clear();
        self.update_current_item_size();

        Ok(())
    }

    /// Build tree lines for display.
    pub fn build_tree_lines(&self, terminal_width: usize) -> Vec<TreeLine> {
        let mut lines = Vec::new();
        let ancestors = self.get_ancestors();

        for (depth, path) in ancestors.iter().enumerate() {
            let indent = "  ".repeat(depth);

            let name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("/")
                .to_string();

            let is_current = path == &self.current_dir;
            let marker = if depth == ancestors.len() - 1 {
                "\u{2570}\u{2500}"
            } else if depth > 0 {
                "\u{2570}\u{2500}"
            } else {
                "\u{2500} "
            };

            lines.push(TreeLine {
                tree_prefix: format!("{}{}", indent, marker),
                icon: String::new(),
                text: name,
                timestamp: None,
                entry_index: None,
                is_selected: false,
                is_cursor: false,
                is_dir: true,
                is_current_dir: is_current,
                is_hidden: false,
            });

            if is_current && !self.entries.is_empty() {
                let child_indent = format!("{}  ", "  ".repeat(depth));

                for (i, entry) in self.entries.iter().enumerate() {
                    let is_last = i == self.entries.len() - 1;
                    let tree_char = if is_last { "\u{2570}\u{2500}" } else { "\u{251c}\u{2500}" };
                    let icon = get_file_icon(&entry.name, entry.is_dir, entry.permissions);
                    let is_hidden = entry.name.starts_with('.');

                    let tree_char_width = 2;
                    let icon_display_width = 2;
                    let prefix_len = child_indent.len() + tree_char_width + icon_display_width;

                    // Metadata column width (size + perms + date)
                    let metadata_width = 42;  // 9 (size) + 2 (spaces) + 10 (perms) + 3 (spaces) + 17 (date) + 1 (buffer)

                    // Minimum width for filename - prioritize showing names over metadata
                    // Metadata will spill off-screen and get clipped naturally by the terminal
                    let min_name_width = 20;
                    let ideal_name_width = terminal_width.saturating_sub(prefix_len + metadata_width);
                    let name_column_width = ideal_name_width.max(min_name_width);

                    let display_name = if entry.name.chars().count() > name_column_width {
                        let truncate_at = name_column_width.saturating_sub(3);
                        let truncated: String = entry.name.chars().take(truncate_at).collect();
                        format!("{}...", truncated)
                    } else {
                        entry.name.clone()
                    };

                    // Always build metadata - let terminal clip what doesn't fit
                    let perms_str = format_permissions(entry.permissions, entry.is_dir);
                    let date_str = format_date(entry.modified);
                    let size_str = if entry.is_dir {
                        "         ".to_string()  // 9 spaces to match size column width
                    } else {
                        format_file_size(entry.size)
                    };
                    let timestamp_str = format!("{}  {}   {}", size_str, perms_str, date_str);

                    let name_len = display_name.chars().count();
                    let padding = " ".repeat(name_column_width.saturating_sub(name_len));

                    lines.push(TreeLine {
                        tree_prefix: format!("{}{} ", child_indent, tree_char),
                        icon: format!("{} ", icon),
                        text: format!("{}{}", display_name, padding),
                        timestamp: Some(timestamp_str),
                        entry_index: Some(i),
                        is_selected: self.selected_indices.contains(&i),
                        is_cursor: i == self.cursor_index,
                        is_dir: entry.is_dir,
                        is_current_dir: false,
                        is_hidden,
                    });
                }
            }
        }

        lines
    }

    /// Get the line index of the cursor in tree lines.
    pub fn get_cursor_line_index(&self, terminal_width: usize) -> usize {
        let tree_lines = self.build_tree_lines(terminal_width);
        for (line_idx, line) in tree_lines.iter().enumerate() {
            if line.is_cursor {
                return line_idx;
            }
        }
        0
    }

    /// Calculate scroll offset to keep cursor visible.
    pub fn calculate_scroll_offset(&mut self, visible_height: usize, tree_lines: &[TreeLine]) {
        let scrolloff = 1;

        if visible_height == 0 {
            return;
        }

        let cursor_line_idx = tree_lines.iter()
            .position(|line| line.is_cursor)
            .unwrap_or(0);

        if cursor_line_idx < self.scroll_offset + scrolloff {
            self.scroll_offset = cursor_line_idx.saturating_sub(scrolloff);
        } else if cursor_line_idx >= self.scroll_offset + visible_height - scrolloff {
            self.scroll_offset = cursor_line_idx + scrolloff + 1 - visible_height.min(tree_lines.len());
        }

        self.scroll_offset = self.scroll_offset.min(tree_lines.len().saturating_sub(visible_height));
    }

    /// Save current directory state.
    pub fn save_state(&mut self) {
        self.dir_memory.insert(
            self.current_dir.clone(),
            DirState {
                cursor_index: self.cursor_index,
                selected_indices: self.selected_indices.clone(),
                scroll_offset: self.scroll_offset,
            },
        );
    }

    /// Move cursor up.
    pub fn move_up(&mut self, shift: bool) {
        if self.cursor_index > 0 {
            if shift {
                if self.selection_anchor.is_none() {
                    self.selection_anchor = Some(self.cursor_index);
                }
            } else {
                self.selected_indices.clear();
                self.selection_anchor = None;
            }

            self.cursor_index -= 1;

            if shift {
                self.update_selection_range();
            }

            self.save_state();
            self.update_current_item_size();
        }
    }

    /// Move cursor down.
    pub fn move_down(&mut self, shift: bool) {
        if self.cursor_index < self.entries.len().saturating_sub(1) {
            if shift {
                if self.selection_anchor.is_none() {
                    self.selection_anchor = Some(self.cursor_index);
                }
            } else {
                self.selected_indices.clear();
                self.selection_anchor = None;
            }

            self.cursor_index += 1;

            if shift {
                self.update_selection_range();
            }

            self.save_state();
            self.update_current_item_size();
        }
    }

    /// Update selection range based on anchor.
    pub fn update_selection_range(&mut self) {
        if let Some(anchor) = self.selection_anchor {
            self.selected_indices.clear();
            let start = anchor.min(self.cursor_index);
            let end = anchor.max(self.cursor_index);
            for i in start..=end {
                self.selected_indices.insert(i);
            }
        }
    }

    /// Toggle selection of current item.
    pub fn toggle_selection(&mut self) {
        if self.selected_indices.contains(&self.cursor_index) {
            self.selected_indices.remove(&self.cursor_index);
        } else {
            self.selected_indices.insert(self.cursor_index);
        }
        self.selection_anchor = None;
        self.save_state();
    }

    /// Enter the directory at cursor.
    pub fn enter_directory(&mut self) -> io::Result<()> {
        if let Some(entry) = self.entries.get(self.cursor_index) {
            if entry.is_dir {
                self.current_dir = entry.path.clone();
                self.load_directory()?;
            }
        }
        Ok(())
    }

    /// Open a file with the default application.
    pub fn open_file(&mut self, path: &PathBuf) -> io::Result<()> {
        let path_str = path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
        })?;

        if cfg!(target_os = "windows") {
            Command::new("cmd")
                .args(&["/c", "start", "", path_str])
                .spawn()?;
        } else if cfg!(target_os = "macos") {
            Command::new("open")
                .arg(path_str)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
        } else {
            // On Linux/Unix, check if we're in a terminal-only environment (SSH, no X11)
            let in_ssh = std::env::var("SSH_CONNECTION").is_ok()
                      || std::env::var("SSH_CLIENT").is_ok()
                      || std::env::var("SSH_TTY").is_ok();
            let no_display = std::env::var("DISPLAY").is_err();
            let has_editor = std::env::var("EDITOR").is_ok() || std::env::var("VISUAL").is_ok();

            // If we're in SSH or no display, and we have an EDITOR set, use it for text files
            if (in_ssh || no_display) && has_editor && Self::is_likely_text_file(path) {
                // Use EDITOR/VISUAL for text files in terminal environment
                let editor = std::env::var("EDITOR")
                    .or_else(|_| std::env::var("VISUAL"))
                    .unwrap_or_else(|_| "vi".to_string());

                // Suspend TUI: disable raw mode, leave alternate screen, disable mouse
                disable_raw_mode()?;
                let mut stdout = io::stdout();
                execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;

                // Launch editor and wait for it to finish
                let status = Command::new(&editor)
                    .arg(path_str)
                    .status();

                // Resume TUI: re-enable raw mode, re-enter alternate screen, re-enable mouse
                enable_raw_mode()?;
                execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

                // Check if editor launched successfully
                status?;
            } else {
                // Use xdg-open for GUI applications
                let command = format!("setsid -f xdg-open '{}' >/dev/null 2>&1 &", path_str);
                Command::new("sh")
                    .arg("-c")
                    .arg(&command)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()?;
            }
        }

        Ok(())
    }

    /// Check if a file is likely a text file based on extension
    fn is_likely_text_file(path: &PathBuf) -> bool {
        if let Some(ext) = path.extension() {
            let ext = ext.to_string_lossy().to_lowercase();
            matches!(
                ext.as_str(),
                "txt" | "md" | "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "json" | "toml" | "yaml" | "yml" |
                "xml" | "html" | "css" | "sh" | "bash" | "zsh" | "fish" | "c" | "cpp" | "h" | "hpp" |
                "java" | "go" | "rb" | "php" | "pl" | "lua" | "vim" | "sql" | "log" | "conf" | "ini" |
                "env" | "gitignore" | "dockerfile" | "makefile" | "cmake" | "gradle" | "properties" |
                "tex" | "r" | "jl" | "swift" | "kt" | "scala" | "hs" | "ml" | "fs" | "clj" | "ex" | "exs"
            )
        } else {
            // Files without extension might be text (like README, Makefile, etc.)
            if let Some(name) = path.file_name() {
                let name = name.to_string_lossy().to_lowercase();
                matches!(
                    name.as_str(),
                    "readme" | "makefile" | "dockerfile" | "license" | "changelog" | "todo" |
                    "authors" | "contributors" | "gemfile" | "rakefile" | "vagrantfile"
                )
            } else {
                false
            }
        }
    }

    /// Open or enter the item at cursor.
    pub fn open_or_enter(&mut self) -> io::Result<()> {
        if let Some(entry) = self.entries.get(self.cursor_index) {
            if entry.is_dir {
                self.current_dir = entry.path.clone();
                self.load_directory()?;
            } else {
                let path = entry.path.clone();
                let name = entry.name.clone();
                if let Err(e) = self.open_file(&path) {
                    self.show_error(format!("Failed to open file: {}", e));
                } else {
                    self.show_status(format!("Opening '{}'", name));
                }
            }
        }
        Ok(())
    }

    /// Go to parent directory.
    pub fn go_to_parent(&mut self) -> io::Result<()> {
        if let Some(parent) = self.current_dir.parent() {
            let current_dir_name = self.current_dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            self.current_dir = parent.to_path_buf();
            self.load_directory()?;

            for (i, entry) in self.entries.iter().enumerate() {
                if entry.name == current_dir_name {
                    self.cursor_index = i;
                    self.save_state();
                    break;
                }
            }
        }
        Ok(())
    }

    /// Handle mouse down event.
    pub fn handle_mouse_down(&mut self, row: u16, _col: u16, modifiers: crossterm::event::KeyModifiers, area_top: u16) {
        let tree_lines = self.build_tree_lines(self.terminal_width);
        let clicked_line = (row as usize).saturating_sub(area_top as usize + 1).saturating_add(self.scroll_offset);

        if clicked_line < tree_lines.len() {
            if let Some(entry_index) = tree_lines[clicked_line].entry_index {
                if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                    self.cursor_index = entry_index;
                    self.toggle_selection();
                    self.update_current_item_size();
                } else {
                    self.cursor_index = entry_index;
                    self.drag_selection = Some(entry_index);
                    self.selected_indices.clear();
                    self.selected_indices.insert(entry_index);
                    self.selection_anchor = Some(entry_index);
                    self.save_state();
                    self.update_current_item_size();
                }
            }
        }
    }

    /// Handle mouse drag event.
    pub fn handle_mouse_drag(&mut self, row: u16, _col: u16, area_top: u16) {
        if self.drag_selection.is_none() {
            return;
        }

        let tree_lines = self.build_tree_lines(self.terminal_width);
        let dragged_line = (row as usize).saturating_sub(area_top as usize + 1).saturating_add(self.scroll_offset);

        if dragged_line < tree_lines.len() {
            if let Some(entry_index) = tree_lines[dragged_line].entry_index {
                self.cursor_index = entry_index;
                self.update_selection_range();
                self.save_state();
                self.update_current_item_size();
            }
        }
    }

    /// Handle mouse up event.
    pub fn handle_mouse_up(&mut self) {
        self.drag_selection = None;
    }

    /// Copy selected items to clipboard.
    pub fn copy_selected(&mut self) {
        let items = self.get_selected_paths();
        if !items.is_empty() {
            let clipboard = Clipboard {
                items,
                operation: ClipboardOp::Copy,
            };

            // Write to shared clipboard file for cross-instance support
            if let Err(e) = clipboard_file::write_clipboard(&clipboard) {
                self.show_status(format!("Warning: Failed to write clipboard file: {}", e));
            }

            let count = clipboard.items.len();
            self.clipboard = Some(clipboard);

            // Clear cut operation tracking (copy replaces cut)
            self.cut_operation_time = None;
            self.cut_items.clear();
            self.last_cut_poll_time = None;

            self.show_status(format!("Copied {} item(s)", count));
        }
    }

    /// Cut selected items to clipboard.
    pub fn cut_selected(&mut self) {
        let items = self.get_selected_paths();
        if !items.is_empty() {
            let clipboard = Clipboard {
                items: items.clone(),
                operation: ClipboardOp::Cut,
            };

            // Write to shared clipboard file for cross-instance support
            if let Err(e) = clipboard_file::write_clipboard(&clipboard) {
                self.show_status(format!("Warning: Failed to write clipboard file: {}", e));
            }

            let count = clipboard.items.len();
            self.clipboard = Some(clipboard);

            // Track cut operation for polling
            self.cut_operation_time = Some(std::time::Instant::now());
            self.cut_items = items;
            self.last_cut_poll_time = Some(std::time::Instant::now());

            self.show_status(format!("Cut {} item(s) - Press Esc to cancel", count));
        }
    }

    /// Paste items from clipboard.
    pub fn paste(&mut self) -> io::Result<()> {
        // Try to read from clipboard file first (for cross-instance support)
        let clipboard = match clipboard_file::read_clipboard() {
            Ok(Some(file_clipboard)) => Some(file_clipboard),
            Ok(None) => self.clipboard.clone(), // Fall back to internal clipboard
            Err(_) => self.clipboard.clone(), // Fall back to internal clipboard on error
        };

        if let Some(clipboard) = clipboard {
            let destination = self.current_dir.clone();
            let items = clipboard.items.clone();
            let is_move = matches!(clipboard.operation, ClipboardOp::Cut);

            match perform_file_operation_tracked(&items, &destination, is_move) {
                Ok((count, undo_action)) => {
                    if is_move {
                        // Clear both internal and file-based clipboard after move
                        self.clipboard = None;

                        // Clear cut operation tracking immediately
                        self.cut_operation_time = None;
                        self.cut_items.clear();
                        self.last_cut_poll_time = None;

                        if let Err(e) = clipboard_file::write_clipboard(&Clipboard {
                            items: Vec::new(),
                            operation: ClipboardOp::Copy,
                        }) {
                            self.show_status(format!("Warning: Failed to clear clipboard file: {}", e));
                        }
                    }

                    let pasted_names: Vec<String> = match &undo_action {
                        UndoAction::Move { moved_files } => {
                            moved_files.iter()
                                .filter_map(|(_, dest)| dest.file_name())
                                .filter_map(|n| n.to_str())
                                .map(|s| s.to_string())
                                .collect()
                        }
                        UndoAction::Copy { copied_files } => {
                            copied_files.iter()
                                .filter_map(|p| p.file_name())
                                .filter_map(|n| n.to_str())
                                .map(|s| s.to_string())
                                .collect()
                        }
                        _ => Vec::new(),
                    };

                    self.undo_stack.push(undo_action);
                    self.show_status(format!("Pasted {} item(s)", count));
                    self.load_directory()?;
                    self.select_items_by_name(&pasted_names);
                }
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    self.ui_mode = UIMode::PasswordPrompt {
                        prompt: "Permission denied. Enter sudo password:".to_string(),
                        password: String::new(),
                        pending_operation: Box::new(PendingOperation {
                            items,
                            destination: Some(destination),
                            operation: if is_move { OperationType::Move } else { OperationType::Copy },
                            undo_action: None,
                        }),
                    };
                }
                Err(e) => {
                    self.show_error(format!("Error: {}", e));
                }
            }
        }
        Ok(())
    }

    /// Start creating a new item.
    pub fn start_create_new(&mut self) {
        self.ui_mode = UIMode::CreateNew {
            creation_type: None,
            name: String::new(),
        };
    }

    /// Create a new file or directory.
    pub fn create_new_item(&mut self, creation_type: CreationType, name: String) -> io::Result<()> {
        if name.is_empty() {
            self.show_status("Name cannot be empty".to_string());
            return Ok(());
        }

        let new_path = self.current_dir.join(&name);

        if new_path.exists() {
            self.show_status(format!("'{}' already exists", name));
            return Ok(());
        }

        match creation_type {
            CreationType::File => {
                let mut file = fs::File::create(&new_path)?;
                let default_content = get_default_file_content(&name);
                if !default_content.is_empty() {
                    file.write_all(default_content.as_bytes())?;
                }
                self.show_status(format!("Created file '{}'", name));
            }
            CreationType::Directory => {
                fs::create_dir(&new_path)?;
                self.show_status(format!("Created directory '{}'", name));
            }
        }

        self.load_directory()?;
        self.select_items_by_name(&[name]);

        Ok(())
    }

    /// Start renaming the current item.
    pub fn start_rename(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_index) {
            let original_path = entry.path.clone();
            let current_name = entry.name.clone();

            let cursor_pos = if let Some(dot_pos) = current_name.rfind('.') {
                if dot_pos > 0 {
                    dot_pos
                } else {
                    current_name.len()
                }
            } else {
                current_name.len()
            };

            self.ui_mode = UIMode::RenameItem {
                original_path,
                new_name: current_name,
                cursor_pos,
                selection_start: Some(0),
            };
        }
    }

    /// Rename an item.
    pub fn rename_item(&mut self, original_path: PathBuf, new_name: String) -> io::Result<()> {
        if new_name.is_empty() {
            self.show_status("Name cannot be empty".to_string());
            return Ok(());
        }

        let parent = original_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
        })?;

        let new_path = parent.join(&new_name);

        if new_path == original_path {
            self.show_status("Name unchanged".to_string());
            return Ok(());
        }

        if new_path.exists() {
            self.show_status(format!("'{}' already exists", new_name));
            return Ok(());
        }

        match fs::rename(&original_path, &new_path) {
            Ok(_) => {
                self.show_status(format!("Renamed to '{}'", new_name));

                self.undo_stack.push(UndoAction::Rename {
                    original_path: original_path.clone(),
                    new_path: new_path.clone(),
                });

                self.size_cache.remove(&original_path);
                self.load_directory()?;
                self.select_items_by_name(&[new_name]);

                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(13) => {
                self.ui_mode = UIMode::PasswordPrompt {
                    prompt: format!("Enter sudo password to rename '{}':", original_path.file_name().unwrap_or_default().to_string_lossy()),
                    password: String::new(),
                    pending_operation: Box::new(PendingOperation {
                        items: vec![original_path.clone()],
                        destination: Some(new_path),
                        operation: OperationType::Move,
                        undo_action: None,
                    }),
                };
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Start deleting selected items.
    pub fn delete_selected(&mut self) {
        let items = self.get_selected_paths();
        if !items.is_empty() {
            self.ui_mode = UIMode::ConfirmDelete { items };
        }
    }

    /// Perform delete operation.
    pub fn perform_delete(&mut self, items: &[PathBuf]) -> io::Result<()> {
        let mut count = 0;
        let mut deleted_files = Vec::new();

        for item in items {
            match trash::delete(item) {
                Ok(_) => {
                    deleted_files.push(item.clone());
                    count += 1;
                }
                Err(e) => {
                    // Check if error is permission denied by looking at the error message
                    let error_msg = e.to_string();
                    if error_msg.contains("Permission denied") || error_msg.contains("PermissionDenied") {
                        return Err(io::Error::new(io::ErrorKind::PermissionDenied, error_msg));
                    } else {
                        return Err(io::Error::new(io::ErrorKind::Other, error_msg));
                    }
                }
            }
        }

        self.undo_stack.push(UndoAction::Delete { deleted_files });
        self.show_status(format!("Deleted {} item(s) (moved to system trash)", count));
        self.selected_indices.clear();
        self.selection_anchor = None;
        self.save_state();
        self.load_directory()?;
        Ok(())
    }

    /// Undo the last action.
    pub fn undo(&mut self) -> io::Result<()> {
        if let Some(action) = self.undo_stack.pop() {
            let action_clone = action.clone();
            let result: io::Result<()> = match action {
                UndoAction::Copy { copied_files } => {
                    let mut count = 0;
                    for file in &copied_files {
                        if file.exists() {
                            if file.is_dir() {
                                if let Err(e) = fs::remove_dir_all(file) {
                                    return self.handle_undo_error(e, action_clone);
                                }
                            } else {
                                if let Err(e) = fs::remove_file(file) {
                                    return self.handle_undo_error(e, action_clone);
                                }
                            }
                            count += 1;
                        }
                    }
                    self.show_status(format!("Undone copy: removed {} item(s)", count));
                    Ok(())
                }
                UndoAction::Move { moved_files } => {
                    let mut count = 0;
                    for (original, moved_to) in &moved_files {
                        if moved_to.exists() {
                            if let Err(e) = fs::rename(moved_to, original) {
                                return self.handle_undo_error(e, action_clone);
                            }
                            count += 1;
                        }
                    }
                    self.show_status(format!("Undone move: restored {} item(s)", count));
                    Ok(())
                }
                UndoAction::Delete { deleted_files } => {
                    // Try to restore files from system trash
                    match trash::os_limited::list() {
                        Ok(trash_items) => {
                            let mut items_to_restore = Vec::new();

                            // Find matching items in trash
                            for original_path in &deleted_files {
                                for item in trash_items.iter() {
                                    if item.original_path() == *original_path {
                                        items_to_restore.push(item.clone());
                                        break;
                                    }
                                }
                            }

                            if items_to_restore.is_empty() {
                                self.show_status("No matching items found in trash (may have been emptied)".to_string());
                            } else {
                                let restore_count = items_to_restore.len();
                                match trash::os_limited::restore_all(items_to_restore) {
                                    Ok(_) => {
                                        self.show_status(format!("Restored {} item(s) from trash", restore_count));
                                    }
                                    Err(e) => {
                                        self.show_error(format!("Failed to restore: {}", e));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            self.show_error(format!("Cannot access trash: {}", e));
                        }
                    }
                    Ok(())
                }
                UndoAction::Rename { original_path, new_path } => {
                    if new_path.exists() {
                        if let Err(e) = fs::rename(&new_path, &original_path) {
                            return self.handle_undo_error(e, action_clone);
                        }
                        let original_name = original_path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();
                        self.show_status(format!("Undone rename: restored to '{}'", original_name));
                    } else {
                        self.show_status("Cannot undo rename: file not found".to_string());
                    }
                    Ok(())
                }
            };

            match result {
                Ok(_) => {
                    if let Err(e) = self.load_directory() {
                        self.show_status(format!("Warning: {}", e));
                    }
                }
                Err(e) => {
                    return self.handle_undo_error(e, action_clone);
                }
            }
        } else {
            self.show_status("Nothing to undo".to_string());
        }
        Ok(())
    }

    /// Handle error during undo operation.
    fn handle_undo_error(&mut self, e: io::Error, action: UndoAction) -> io::Result<()> {
        let is_permission_error = e.kind() == io::ErrorKind::PermissionDenied
            || e.raw_os_error() == Some(13);

        if is_permission_error {
            self.undo_stack.push(action.clone());
            self.ui_mode = UIMode::PasswordPrompt {
                prompt: "Permission denied. Enter sudo password:".to_string(),
                password: String::new(),
                pending_operation: Box::new(PendingOperation {
                    items: Vec::new(),
                    destination: None,
                    operation: OperationType::Undo,
                    undo_action: Some(action),
                }),
            };
            Ok(())
        } else {
            self.show_status(format!("Undo error: {}", e));
            Ok(())
        }
    }

    /// Select items by name.
    pub fn select_items_by_name(&mut self, names: &[String]) {
        self.selected_indices.clear();
        for (i, entry) in self.entries.iter().enumerate() {
            if names.contains(&entry.name) {
                self.selected_indices.insert(i);
            }
        }
        if let Some(&first_idx) = self.selected_indices.iter().next() {
            self.cursor_index = first_idx;
        }
        self.save_state();
    }

    /// Get paths of selected items.
    pub fn get_selected_paths(&self) -> Vec<PathBuf> {
        let indices = if self.selected_indices.is_empty() {
            vec![self.cursor_index]
        } else {
            self.selected_indices.iter().cloned().collect()
        };

        indices.iter()
            .filter_map(|&i| self.entries.get(i))
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// Get file size.
    pub fn get_file_size(path: &PathBuf) -> u64 {
        if let Ok(metadata) = fs::metadata(path) {
            if metadata.is_file() {
                return metadata.len();
            }
        }
        0
    }

    /// Get total size of selected items.
    pub fn get_selected_total_size(&self) -> u64 {
        self.selected_indices
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .map(|entry| Self::get_file_size(&entry.path))
            .sum()
    }

    /// Update size of current item.
    pub fn update_current_item_size(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_index) {
            let path = &entry.path;
            if let Some(&cached_size) = self.size_cache.get(path) {
                self.current_item_size = Some(cached_size);
            } else {
                let size = Self::get_file_size(path);
                self.size_cache.insert(path.clone(), size);
                self.current_item_size = Some(size);
            }
        } else {
            self.current_item_size = None;
        }
    }

    /// Show a status message with a specific type.
    pub fn show_status(&mut self, message: String) {
        self.status_message = Some((message, StatusType::Info));
        self.status_message_time = Some(std::time::Instant::now());
    }

    /// Show an error status message (more prominent styling).
    pub fn show_error(&mut self, message: String) {
        self.status_message = Some((message, StatusType::Error));
        self.status_message_time = Some(std::time::Instant::now());
    }

    /// Clear the status message.
    pub fn clear_status(&mut self) {
        self.status_message = None;
        self.status_message_time = None;
        if matches!(self.ui_mode, UIMode::StatusMessage { .. }) {
            self.ui_mode = UIMode::Normal;
        }
    }

    /// Toggle help screen.
    pub fn toggle_help(&mut self) {
        if matches!(self.ui_mode, UIMode::Help) {
            self.ui_mode = UIMode::Normal;
        } else {
            self.ui_mode = UIMode::Help;
            self.help_scroll_offset = 0;
        }
    }

    /// Scroll help screen up.
    pub fn scroll_help_up(&mut self) {
        self.help_scroll_offset = self.help_scroll_offset.saturating_sub(1);
    }

    /// Scroll help screen down.
    pub fn scroll_help_down(&mut self) {
        self.help_scroll_offset += 1;
    }

    /// Get ancestor directories.
    pub fn get_ancestors(&self) -> Vec<PathBuf> {
        let mut ancestors = Vec::new();
        let mut current = self.current_dir.clone();

        ancestors.push(current.clone());

        while let Some(parent) = current.parent() {
            if parent == current {
                break;
            }
            current = parent.to_path_buf();
            ancestors.insert(0, current.clone());
        }

        ancestors
    }

    /// Get maximum modified time for a directory.
    pub fn get_dir_max_modified(path: &PathBuf, max_depth: usize) -> SystemTime {
        Self::get_dir_max_modified_recursive(path, max_depth, 0)
    }

    fn get_dir_max_modified_recursive(path: &PathBuf, max_depth: usize, current_depth: usize) -> SystemTime {
        let mut max_time = SystemTime::UNIX_EPOCH;

        if let Ok(metadata) = fs::metadata(path) {
            if let Ok(modified) = metadata.modified() {
                max_time = modified;
            }
        }

        if current_depth >= max_depth {
            return max_time;
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        if metadata.is_file() {
                            if modified > max_time {
                                max_time = modified;
                            }
                        } else if metadata.is_dir() && current_depth + 1 <= max_depth {
                            let sub_max = Self::get_dir_max_modified_recursive(&entry.path(), max_depth, current_depth + 1);
                            if sub_max > max_time {
                                max_time = sub_max;
                            }
                        }
                    }
                }
            }
        }

        max_time
    }

    /// Toggle sort mode.
    pub fn toggle_sort_mode(&mut self) -> io::Result<()> {
        self.sort_mode = match self.sort_mode {
            SortMode::Name => SortMode::Date,
            SortMode::Date => SortMode::Name,
        };

        let mode_name = match self.sort_mode {
            SortMode::Name => "Name",
            SortMode::Date => "Date Modified",
        };
        self.show_status(format!("Sorting by: {}", mode_name));

        self.load_directory()?;
        Ok(())
    }

    /// Toggle hidden files visibility.
    pub fn toggle_hidden(&mut self) -> io::Result<()> {
        self.show_hidden = !self.show_hidden;

        let status_msg = if self.show_hidden {
            "Showing hidden files"
        } else {
            "Hiding hidden files"
        };
        self.show_status(status_msg.to_string());

        self.load_directory()?;
        Ok(())
    }

    /// Perform fuzzy search.
    pub fn perform_fuzzy_search(&self, search_term: &str, file_cache: &Arc<Vec<CachedFile>>) -> Vec<FuzzyMatch> {
        perform_fuzzy_search(search_term, file_cache)
    }

    /// Build file cache.
    #[allow(dead_code)]
    pub fn build_file_cache(&self, dir: &PathBuf, max_depth: Option<usize>, current_depth: usize, cache: &mut Vec<CachedFile>) {
        build_file_cache_static(dir, max_depth, current_depth, cache, self.show_hidden, None);
    }

    /// Get quick navigation locations.
    pub fn get_quick_nav_locations(&self) -> Vec<QuickNavLocation> {
        let mut locations = Vec::new();

        // Home directory
        #[cfg(windows)]
        let home = std::env::var("USERPROFILE").ok().map(PathBuf::from);
        #[cfg(not(windows))]
        let home = std::env::var("HOME").ok().map(PathBuf::from);

        if let Some(home_path) = home {
            // Home
            locations.push(QuickNavLocation {
                name: "Home".to_string(),
                path: Some(home_path.clone()),
                icon: "\u{f015}".to_string(),  //
                is_virtual: false,
            });

            // Desktop
            let desktop = home_path.join("Desktop");
            if desktop.exists() {
                locations.push(QuickNavLocation {
                    name: "Desktop".to_string(),
                    path: Some(desktop),
                    icon: "\u{f108}".to_string(),  //
                    is_virtual: false,
                });
            }

            // Downloads
            let downloads = home_path.join("Downloads");
            if downloads.exists() {
                locations.push(QuickNavLocation {
                    name: "Downloads".to_string(),
                    path: Some(downloads),
                    icon: "\u{f019}".to_string(),  //
                    is_virtual: false,
                });
            }

            // Documents
            let documents = home_path.join("Documents");
            if documents.exists() {
                locations.push(QuickNavLocation {
                    name: "Documents".to_string(),
                    path: Some(documents),
                    icon: "\u{f07b}".to_string(),  //
                    is_virtual: false,
                });
            }

            // Pictures
            let pictures = home_path.join("Pictures");
            if pictures.exists() {
                locations.push(QuickNavLocation {
                    name: "Pictures".to_string(),
                    path: Some(pictures),
                    icon: "\u{f03e}".to_string(),  //
                    is_virtual: false,
                });
            }
        }

        // System Trash/Recycle Bin (virtual - we'll list contents differently)
        locations.push(QuickNavLocation {
            name: "Trash".to_string(),
            path: None,
            icon: "\u{f1f8}".to_string(),  //
            is_virtual: true,
        });

        // Project root (find .git directory going up from current)
        let mut check_dir = self.current_dir.clone();
        loop {
            if check_dir.join(".git").exists() {
                locations.push(QuickNavLocation {
                    name: "Project Root".to_string(),
                    path: Some(check_dir),
                    icon: "\u{f126}".to_string(),  //
                    is_virtual: false,
                });
                break;
            }
            if let Some(parent) = check_dir.parent() {
                check_dir = parent.to_path_buf();
            } else {
                break;
            }
        }

        // Root directory
        #[cfg(windows)]
        {
            // Add common drive letters on Windows
            for letter in ['C', 'D', 'E'] {
                let drive = PathBuf::from(format!("{}:\\", letter));
                if drive.exists() {
                    locations.push(QuickNavLocation {
                        name: format!("Drive {}", letter),
                        path: Some(drive),
                        icon: "\u{f0a0}".to_string(),  //
                        is_virtual: false,
                    });
                }
            }
        }
        #[cfg(not(windows))]
        {
            locations.push(QuickNavLocation {
                name: "Root".to_string(),
                path: Some(PathBuf::from("/")),
                icon: "\u{f0a0}".to_string(),  //
                is_virtual: false,
            });
        }

        locations
    }

    /// Open the quick nav popup.
    pub fn open_quick_nav(&mut self) {
        let locations = self.get_quick_nav_locations();
        self.ui_mode = UIMode::QuickNav {
            locations,
            selected_index: 0,
        };
    }

    /// Check if a cut operation has been completed (pasted in another instance).
    /// Returns true if the directory should be refreshed.
    pub fn check_cut_completion(&mut self) -> bool {
        if self.cut_operation_time.is_none() {
            return false;
        }

        // Only poll every 1 second
        if let Some(last_poll) = self.last_cut_poll_time {
            if last_poll.elapsed() < std::time::Duration::from_secs(1) {
                return false;
            }
        }
        self.last_cut_poll_time = Some(std::time::Instant::now());

        // Check timeout (2 minutes)
        if let Some(cut_time) = self.cut_operation_time {
            if cut_time.elapsed() > std::time::Duration::from_secs(120) {
                self.cancel_cut_operation();
                self.show_status("Cut operation timed out".to_string());
                return false;
            }
        }

        // Read current clipboard state
        match clipboard_file::read_clipboard() {
            Ok(Some(clipboard)) => {
                // Check if clipboard contains exactly our cut items (not just contains)
                if clipboard.items != self.cut_items {
                    // Clipboard changed to different items - another instance cut something
                    // Just cancel our cut operation silently
                    self.cut_operation_time = None;
                    self.cut_items.clear();
                    self.clipboard = None;
                    return false;
                }
                // Still our items, keep waiting
                false
            }
            Ok(None) => {
                // Clipboard is empty/cleared - paste happened!
                self.cut_operation_time = None;
                self.cut_items.clear();
                self.clipboard = None;
                true
            }
            Err(_) => {
                // Error reading clipboard - assume paste happened
                self.cut_operation_time = None;
                self.cut_items.clear();
                self.clipboard = None;
                true
            }
        }
    }

    /// Cancel an active cut operation.
    pub fn cancel_cut_operation(&mut self) {
        if self.cut_operation_time.is_some() {
            self.cut_operation_time = None;
            self.cut_items.clear();
            self.last_cut_poll_time = None;
            self.clipboard = None;
            self.show_status("Cut operation cancelled".to_string());
        }
    }

    /// Check if there's an active cut operation.
    pub fn has_active_cut(&self) -> bool {
        self.cut_operation_time.is_some()
    }
}
