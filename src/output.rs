//! Output and color utilities for consistent terminal formatting
//!
//! Provides shared color functions respecting NO_COLOR environment variable.

use colored::Colorize;

/// Check if colors should be used (respects NO_COLOR env var)
pub fn use_colors() -> bool {
    std::env::var("NO_COLOR").is_err()
}

/// Colorize file path (cyan)
pub fn colorize_path(text: &str, use_color: bool) -> String {
    if use_color {
        text.cyan().to_string()
    } else {
        text.to_string()
    }
}

/// Colorize line number (yellow)
pub fn colorize_line_num(num: usize, use_color: bool) -> String {
    if use_color {
        num.to_string().yellow().to_string()
    } else {
        num.to_string()
    }
}

/// Colorize match highlight (red bold)
pub fn colorize_match(text: &str, use_color: bool) -> String {
    if use_color {
        text.red().bold().to_string()
    } else {
        text.to_string()
    }
}

/// Colorize context lines (dimmed)
pub fn colorize_context(text: &str, use_color: bool) -> String {
    if use_color {
        text.dimmed().to_string()
    } else {
        text.to_string()
    }
}

/// Colorize symbol kind (green)
pub fn colorize_kind(text: &str, use_color: bool) -> String {
    if use_color {
        text.green().to_string()
    } else {
        text.to_string()
    }
}

/// Colorize symbol name (bold)
pub fn colorize_name(text: &str, use_color: bool) -> String {
    if use_color {
        text.bold().to_string()
    } else {
        text.to_string()
    }
}
