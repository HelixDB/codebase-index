// Get language from file extension
pub fn get_language(file_path: &Path) -> Option<tree_sitter::Language> {
    let extension = file_path.extension().and_then(|s| s.to_str());
    match extension {
        Some("py") => Some(tree_sitter_python::language()),
        Some("js") => Some(tree_sitter_javascript::language()),
        Some("rs") => Some(tree_sitter_rust::language()),
        _ => None,
    }
}