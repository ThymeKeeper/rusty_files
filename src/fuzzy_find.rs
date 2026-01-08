//! Fuzzy find module.
//!
//! This module provides fuzzy search functionality for files and directories.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::types::{CachedFile, FuzzyMatch};

/// Perform a case-insensitive substring search.
pub fn case_insensitive_find(target: &[char], search: &[char]) -> Option<usize> {
    if search.is_empty() || target.len() < search.len() {
        return None;
    }

    'outer: for i in 0..=target.len() - search.len() {
        for j in 0..search.len() {
            let target_lower = target[i + j].to_lowercase().next().unwrap_or(target[i + j]);
            if target_lower != search[j] {
                continue 'outer;
            }
        }
        return Some(i);
    }
    None
}

/// Perform fuzzy matching with a pre-lowercased search term.
/// Returns (score, matched_positions) or None if no match.
pub fn fuzzy_match_with_lower(search_lower: &str, target: &str) -> Option<(i32, Vec<usize>)> {
    if search_lower.is_empty() {
        return Some((0, Vec::new()));
    }

    // Collect target chars once for efficient indexed access
    let target_chars: Vec<char> = target.chars().collect();
    let search_chars: Vec<char> = search_lower.chars().collect();

    // First, try to find as substring (most common case)
    if let Some(start_pos) = case_insensitive_find(&target_chars, &search_chars) {
        // Found as substring - give massive bonus
        let matched_positions: Vec<usize> = (start_pos..start_pos + search_chars.len()).collect();
        let mut score = 1000; // Huge base bonus for substring match

        // Extra bonus if at word boundary or start
        if start_pos == 0 {
            score += 500; // At very start
        } else if start_pos > 0 {
            let prev_char = target_chars[start_pos - 1];
            if prev_char == '/' || prev_char == '_' || prev_char == '-' || prev_char == ' ' {
                score += 300; // At word boundary
            }
        }

        // Bonus for shorter target (better match)
        score += 100 - (target_chars.len() as i32 - search_chars.len() as i32);

        return Some((score, matched_positions));
    }

    // Fall back to fuzzy matching if not found as substring
    let mut search_idx = 0;
    let mut current_search = search_chars[search_idx];
    let mut last_match_pos = 0;
    let mut score = 0;
    let mut consecutive_matches = 0;
    let mut matched_positions = Vec::new();

    for (i, &target_char) in target_chars.iter().enumerate() {
        let target_lower = target_char.to_lowercase().next().unwrap_or(target_char);
        if target_lower == current_search {
            matched_positions.push(i);

            // Bonus for consecutive matches
            if i == last_match_pos + 1 {
                consecutive_matches += 1;
                score += 10 + consecutive_matches * 5;
            } else {
                consecutive_matches = 0;
                score += 1;
            }

            // Bonus for matching at start
            if i == 0 {
                score += 20;
            }

            last_match_pos = i;

            search_idx += 1;
            if search_idx < search_chars.len() {
                current_search = search_chars[search_idx];
            } else {
                // All search chars matched
                // Bonus for shorter strings (better match)
                score += 100 - (target_chars.len() as i32 - search_chars.len() as i32);
                return Some((score, matched_positions));
            }
        }
    }

    None // Not all search characters were found
}

/// Build a file cache from a directory tree using parallel traversal with BFS ordering.
pub fn build_file_cache_static(
    dir: &PathBuf,
    max_depth: Option<usize>,
    _current_depth: usize,
    cache: &mut Vec<CachedFile>,
    show_hidden: bool,
    progress_sender: Option<std::sync::mpsc::Sender<Arc<Vec<CachedFile>>>>,
) {
    use ignore::WalkBuilder;
    use ignore::overrides::OverrideBuilder;

    // Build override patterns for common bloat directories
    let mut override_builder = OverrideBuilder::new(dir);
    override_builder.add("!node_modules/").ok();
    override_builder.add("!dist/").ok();
    override_builder.add("!build/").ok();
    override_builder.add("!out/").ok();
    override_builder.add("!__pycache__/").ok();
    override_builder.add("!.pytest_cache/").ok();
    override_builder.add("!.venv/").ok();
    override_builder.add("!venv/").ok();
    override_builder.add("!env/").ok();
    override_builder.add("!.next/").ok();
    override_builder.add("!.nuxt/").ok();

    let overrides = override_builder.build().ok();

    // Create walker that respects .gitignore with parallel threads
    let mut walker = WalkBuilder::new(dir);
    walker
        .max_depth(max_depth)
        .hidden(!show_hidden)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false)
        .same_file_system(true)
        .threads(num_cpus::get().max(4)); // Use available CPUs, minimum 4

    if let Some(overrides) = overrides {
        walker.overrides(overrides);
    }

    // Collect all results (fast parallel scan, no intermediate cloning)
    for result in walker.build() {
        if let Ok(entry) = result {
            let path = entry.path();

            // Skip the base directory itself
            if path == dir {
                continue;
            }

            if let (Some(name), Ok(metadata)) = (
                path.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()),
                entry.metadata()
            ) {
                let is_dir = metadata.is_dir();

                // Get permissions
                #[cfg(unix)]
                use std::os::unix::fs::PermissionsExt;
                #[cfg(unix)]
                let permissions = metadata.permissions().mode();
                #[cfg(not(unix))]
                let permissions = 0;

                // Get the display path (relative to base directory)
                let display_path = path.strip_prefix(dir)
                    .ok()
                    .and_then(|p| p.to_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| path.display().to_string());

                // Add to cache
                cache.push(CachedFile {
                    path: path.to_path_buf(),
                    display_path,
                    name,
                    is_dir,
                    permissions,
                });
            }
        }
    }

    // Send final cache once (avoids OOM from repeated cloning)
    if let Some(ref sender) = progress_sender {
        let _ = sender.send(Arc::new(cache.clone()));
    }
}

/// Perform a fuzzy search on the file cache.
pub fn perform_fuzzy_search(
    search_term: &str,
    file_cache: &Arc<Vec<CachedFile>>,
) -> Vec<FuzzyMatch> {
    // Return empty if search term is empty or too short
    if search_term.is_empty() {
        return Vec::new();
    }

    // For single character, do simple case-insensitive substring match
    if search_term.len() == 1 {
        let search_lower = search_term.to_lowercase();
        let mut results = Vec::with_capacity(100);

        for cached_file in file_cache.iter() {
            if cached_file.display_path.to_lowercase().contains(&search_lower) {
                results.push(FuzzyMatch {
                    path: cached_file.path.clone(),
                    display_path: cached_file.display_path.clone(),
                    name: cached_file.name.clone(),
                    is_dir: cached_file.is_dir,
                    permissions: cached_file.permissions,
                    score: 50,
                    matched_positions: Vec::new(),
                });

                if results.len() >= 200 {
                    break;
                }
            }
        }

        results.truncate(20);
        return results;
    }

    // Convert search term to lowercase ONCE instead of for every file
    let search_lower = search_term.to_lowercase();

    // Pre-allocate with reasonable capacity
    let mut results = Vec::with_capacity(100);

    // Early termination: stop after finding enough matches to sort through
    const MAX_RESULTS_BEFORE_SORT: usize = 200;

    // Search the cached file list
    for cached_file in file_cache.iter() {
        if let Some((score, matched_positions)) = fuzzy_match_with_lower(&search_lower, &cached_file.display_path) {
            results.push(FuzzyMatch {
                path: cached_file.path.clone(),
                display_path: cached_file.display_path.clone(),
                name: cached_file.name.clone(),
                is_dir: cached_file.is_dir,
                permissions: cached_file.permissions,
                score,
                matched_positions,
            });

            // Early exit if we have enough matches
            if results.len() >= MAX_RESULTS_BEFORE_SORT {
                break;
            }
        }
    }

    // Sort by score (highest first) and limit to top 20
    results.sort_unstable_by(|a, b| b.score.cmp(&a.score));
    results.truncate(20);

    results
}

/// Search a directory recursively for fuzzy matches.
#[allow(dead_code)]
pub fn search_directory_recursive(
    dir: &PathBuf,
    current_dir: &PathBuf,
    max_depth: usize,
    current_depth: usize,
    results: &mut Vec<FuzzyMatch>,
    search_term: &str,
    show_hidden: bool,
) {
    if current_depth > max_depth {
        return;
    }

    if let Ok(read_dir) = fs::read_dir(dir) {
        for entry in read_dir.flatten() {
            if let (Ok(name), Ok(metadata)) = (
                entry.file_name().into_string(),
                entry.metadata()
            ) {
                // Skip hidden files if show_hidden is false
                if !show_hidden && name.starts_with('.') {
                    continue;
                }

                let path = entry.path();
                let is_dir = metadata.is_dir();

                // Get permissions
                #[cfg(unix)]
                use std::os::unix::fs::PermissionsExt;
                #[cfg(unix)]
                let permissions = metadata.permissions().mode();
                #[cfg(not(unix))]
                let permissions = 0;

                // Get the display path (relative to current directory)
                let display_path = path.strip_prefix(current_dir)
                    .ok()
                    .and_then(|p| p.to_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| path.display().to_string());

                // Try to match against the full path
                let search_lower = search_term.to_lowercase();
                if let Some((score, matched_positions)) = fuzzy_match_with_lower(&search_lower, &display_path) {
                    results.push(FuzzyMatch {
                        path: path.clone(),
                        display_path,
                        name: name.clone(),
                        is_dir,
                        permissions,
                        score,
                        matched_positions,
                    });
                }

                // Recurse into directories
                if is_dir && current_depth < max_depth {
                    search_directory_recursive(&path, current_dir, max_depth, current_depth + 1, results, search_term, show_hidden);
                }
            }
        }
    }
}
