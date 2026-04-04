//! Type definitions for the file explorer application.
//!
//! This module contains all the core types, structs, and enums used throughout
//! the application.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// Sort mode for directory entries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortMode {
    Name,
    Date,
}

/// Status message type for different visual styles.
#[derive(Clone, Debug, PartialEq)]
pub enum StatusType {
    Info,    // Normal informational messages
    Prompt,  // User input required (confirmations, passwords)
    Error,   // Errors and access denials
}

/// A directory entry with metadata.
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: SystemTime,
    pub permissions: u32,
}

/// State for a directory (cursor position, selections, scroll).
#[derive(Clone, Debug)]
pub struct DirState {
    pub cursor_index: usize,
    pub selected_indices: HashSet<usize>,
    pub scroll_offset: usize,
}

/// Clipboard operation type.
#[derive(Clone, Debug)]
pub enum ClipboardOp {
    Copy,
    Cut,
}

/// Clipboard contents with operation type.
#[derive(Clone, Debug)]
pub struct Clipboard {
    pub items: Vec<PathBuf>,
    pub operation: ClipboardOp,
}

/// An action that can be undone.
#[derive(Clone, Debug)]
pub enum UndoAction {
    Copy {
        copied_files: Vec<PathBuf>,
    },
    Move {
        moved_files: Vec<(PathBuf, PathBuf)>,
    },
    Delete {
        /// Original paths of deleted files (used to find items in system trash for restore)
        deleted_files: Vec<PathBuf>,
    },
    Rename {
        original_path: PathBuf,
        new_path: PathBuf,
    },
}

/// Type of item being created.
#[derive(Clone, Debug)]
pub enum CreationType {
    File,
    Directory,
}

/// Current UI mode/state.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum UIMode {
    Normal,
    PasswordPrompt {
        prompt: String,
        password: String,
        pending_operation: Box<PendingOperation>,
    },
    StatusMessage {
        message: String,
    },
    ConfirmDelete {
        items: Vec<PathBuf>,
    },
    CreateNew {
        creation_type: Option<CreationType>,
        name: String,
    },
    RenameItem {
        original_path: PathBuf,
        new_name: String,
        cursor_pos: usize,
        selection_start: Option<usize>,
    },
    Help,
    FuzzyFind {
        search_term: String,
        matches: Vec<FuzzyMatch>,
        selected_index: usize,
        file_cache: Arc<Vec<CachedFile>>,
    },
    QuickNav {
        locations: Vec<QuickNavLocation>,
        selected_index: usize,
    },
    SetupGuide {
        message: Vec<String>,
    },
}

/// A quick navigation location.
#[derive(Clone, Debug)]
pub struct QuickNavLocation {
    pub name: String,
    pub path: Option<PathBuf>,
    pub icon: String,
    pub is_virtual: bool,  // true for trash (not a real directory)
}

/// A cached file entry for fuzzy finding.
#[derive(Clone, Debug)]
pub struct CachedFile {
    pub path: PathBuf,
    pub display_path: String,
    pub name: String,
    pub is_dir: bool,
    pub permissions: u32,
}

/// A fuzzy search match result.
#[derive(Clone, Debug)]
pub struct FuzzyMatch {
    pub path: PathBuf,
    pub display_path: String,
    pub name: String,
    pub is_dir: bool,
    pub permissions: u32,
    pub score: i32,
    pub matched_positions: Vec<usize>,
}

/// Type of pending operation requiring sudo.
#[derive(Clone, Debug)]
pub enum OperationType {
    Copy,
    Move,
    Delete,
    Undo,
}

/// A pending operation (possibly requiring elevated privileges).
#[derive(Clone, Debug)]
pub struct PendingOperation {
    pub items: Vec<PathBuf>,
    pub destination: Option<PathBuf>,
    pub operation: OperationType,
    pub undo_action: Option<UndoAction>,
}

/// A line in the tree view display.
#[allow(dead_code)]
pub struct TreeLine {
    pub tree_prefix: String,
    pub icon: String,
    pub text: String,
    pub timestamp: Option<String>,
    pub entry_index: Option<usize>,
    pub is_selected: bool,
    pub is_cursor: bool,
    pub is_dir: bool,
    pub is_current_dir: bool,
    pub is_hidden: bool,
}
