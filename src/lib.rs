use lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rename {
    pub old_path: PathBuf,
    pub new_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleRename {
    old: Vec<String>,
    new: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

pub fn workspace_edit_for_renames(
    roots: &[PathBuf],
    renames: &[Rename],
) -> std::io::Result<WorkspaceEdit> {
    let module_renames = module_renames(roots, renames);
    let candidates = candidate_files(roots, &module_renames)?;
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    for file in candidates {
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let Some(importer_module) = module_for_path(roots, &file) else {
            continue;
        };
        let edits = edits_for_text(&text, &importer_module, &module_renames);
        if edits.is_empty() {
            continue;
        }
        let lsp_edits = edits
            .into_iter()
            .map(|edit| TextEdit {
                range: byte_range_to_lsp_range(&text, edit.start, edit.end),
                new_text: edit.replacement,
            })
            .collect::<Vec<_>>();
        if let Ok(url) = Url::from_file_path(&file) {
            changes.insert(url, lsp_edits);
        }
    }

    Ok(WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    })
}

fn module_renames(roots: &[PathBuf], renames: &[Rename]) -> Vec<ModuleRename> {
    renames
        .iter()
        .filter(|rename| is_python_file(&rename.old_path) && is_python_file(&rename.new_path))
        .filter_map(|rename| {
            let old = module_for_path(roots, &rename.old_path)?;
            let new = module_for_path(roots, &rename.new_path)?;
            (old != new).then_some(ModuleRename { old, new })
        })
        .collect()
}

fn candidate_files(roots: &[PathBuf], renames: &[ModuleRename]) -> std::io::Result<Vec<PathBuf>> {
    let mut patterns = HashSet::new();
    for rename in renames {
        patterns.insert(rename.old.join("."));
        if let Some(last) = rename.old.last() {
            patterns.insert(last.clone());
        }
    }
    if patterns.is_empty() {
        return Ok(Vec::new());
    }

    let mut files = HashSet::new();
    for root in roots {
        for pattern in &patterns {
            match Command::new("rg")
                .arg("--files-with-matches")
                .arg("--fixed-strings")
                .arg("--glob")
                .arg("*.py")
                .arg(pattern)
                .arg(root)
                .output()
            {
                Ok(output) if output.status.success() || output.status.code() == Some(1) => {
                    for line in String::from_utf8_lossy(&output.stdout).lines() {
                        files.insert(PathBuf::from(line));
                    }
                }
                Ok(_) | Err(_) => collect_python_files(root, &mut files)?,
            }
        }
    }
    Ok(files.into_iter().collect())
}

fn collect_python_files(root: &Path, files: &mut HashSet<PathBuf>) -> std::io::Result<()> {
    if root.is_file() {
        if is_python_file(root) {
            files.insert(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name == ".git" || name == ".venv" || name == "venv" || name == "__pycache__" {
                continue;
            }
            collect_python_files(&path, files)?;
        } else if is_python_file(&path) {
            files.insert(path);
        }
    }
    Ok(())
}

fn edits_for_text(text: &str, importer_module: &[String], renames: &[ModuleRename]) -> Vec<Edit> {
    let mut edits = Vec::new();
    let mut offset = 0usize;
    let importer_pkg = importer_package(importer_module);

    for line_with_newline in text.split_inclusive('\n') {
        let line = line_with_newline
            .strip_suffix('\n')
            .unwrap_or(line_with_newline);
        let code_end = line.find('#').unwrap_or(line.len());
        let code = &line[..code_end];
        let trimmed_start = code.len() - code.trim_start().len();
        let trimmed = &code[trimmed_start..];

        if let Some(rest) = trimmed.strip_prefix("import ") {
            let base = offset + trimmed_start + "import ".len();
            edits.extend(import_edits(rest, base, renames));
        } else if let Some(rest) = trimmed.strip_prefix("from ") {
            let base = offset + trimmed_start + "from ".len();
            edits.extend(from_edits(rest, base, &importer_pkg, renames));
        }

        offset += line_with_newline.len();
    }

    edits
}

fn import_edits(rest: &str, base: usize, renames: &[ModuleRename]) -> Vec<Edit> {
    let mut edits = Vec::new();
    let mut segment_start = 0usize;
    for segment in rest.split(',') {
        let leading = segment.len() - segment.trim_start().len();
        let token_start = segment_start + leading;
        let token = segment.trim_start().split_whitespace().next().unwrap_or("");
        if is_module_token(token) {
            let module = split_module(token);
            for rename in renames {
                if module == rename.old {
                    edits.push(Edit {
                        start: base + token_start,
                        end: base + token_start + token.len(),
                        replacement: rename.new.join("."),
                    });
                    break;
                }
            }
        }
        segment_start += segment.len() + 1;
    }
    edits
}

fn from_edits(
    rest: &str,
    base: usize,
    importer_pkg: &[String],
    renames: &[ModuleRename],
) -> Vec<Edit> {
    let Some(import_pos) = rest.find(" import ") else {
        return Vec::new();
    };
    let module_text = rest[..import_pos].trim();
    if module_text.is_empty() {
        return Vec::new();
    }
    let module_leading = rest[..import_pos].len() - rest[..import_pos].trim_start().len();
    let import_list_start = import_pos + " import ".len();
    let level = module_text.chars().take_while(|c| *c == '.').count();
    let explicit = &module_text[level..];
    let resolved_base = resolve_from_module(importer_pkg, level, explicit);

    for rename in renames {
        if resolved_base == rename.old {
            let replacement = replacement_for_from_module(importer_pkg, level, &rename.new);
            return vec![Edit {
                start: base + module_leading,
                end: base + module_leading + module_text.len(),
                replacement,
            }];
        }

        if rename.old.len() == resolved_base.len() + 1 && rename.old.starts_with(&resolved_base) {
            let old_leaf = rename.old.last().unwrap();
            let new_leaf = rename.new.last().unwrap();
            if let Some((start, end)) = find_imported_name(rest, import_list_start, old_leaf) {
                return vec![Edit {
                    start: base + start,
                    end: base + end,
                    replacement: new_leaf.clone(),
                }];
            }
        }
    }
    Vec::new()
}

fn find_imported_name(rest: &str, import_list_start: usize, name: &str) -> Option<(usize, usize)> {
    let import_list = &rest[import_list_start..];
    let import_list = import_list.trim_start_matches('(').trim_end_matches(')');
    let skipped_prefix = rest[import_list_start..].find(import_list).unwrap_or(0);
    let mut segment_start = import_list_start + skipped_prefix;
    for segment in import_list.split(',') {
        let leading = segment.len() - segment.trim_start().len();
        let token = segment.trim_start().split_whitespace().next().unwrap_or("");
        if token == name {
            return Some((
                segment_start + leading,
                segment_start + leading + token.len(),
            ));
        }
        segment_start += segment.len() + 1;
    }
    None
}

fn resolve_from_module(importer_pkg: &[String], level: usize, explicit: &str) -> Vec<String> {
    if level == 0 {
        return split_module(explicit);
    }
    let keep = importer_pkg.len().saturating_sub(level.saturating_sub(1));
    let mut resolved = importer_pkg[..keep].to_vec();
    if !explicit.is_empty() {
        resolved.extend(split_module(explicit));
    }
    resolved
}

fn replacement_for_from_module(
    importer_pkg: &[String],
    level: usize,
    new_abs: &[String],
) -> String {
    if level == 0 {
        return new_abs.join(".");
    }
    let keep = importer_pkg.len().saturating_sub(level.saturating_sub(1));
    let prefix = &importer_pkg[..keep];
    if new_abs.starts_with(prefix) {
        let suffix = &new_abs[prefix.len()..];
        format!("{}{}", ".".repeat(level), suffix.join("."))
    } else {
        new_abs.join(".")
    }
}

fn importer_package(module: &[String]) -> Vec<String> {
    if module.last().map(String::as_str) == Some("__init__") {
        module[..module.len().saturating_sub(1)].to_vec()
    } else {
        module[..module.len().saturating_sub(1)].to_vec()
    }
}

fn module_for_path(roots: &[PathBuf], path: &Path) -> Option<Vec<String>> {
    let path = path.to_path_buf();
    let mut best: Option<Vec<String>> = None;
    for root in roots {
        let root = root.to_path_buf();
        let rel = path.strip_prefix(&root).ok()?;
        let mut candidates = vec![rel.to_path_buf()];
        if rel.components().next() == Some(Component::Normal("src".as_ref())) {
            candidates.push(rel.components().skip(1).collect());
        }
        for candidate in candidates {
            if candidate.extension().and_then(|ext| ext.to_str()) != Some("py") {
                continue;
            }
            let mut parts = candidate
                .with_extension("")
                .components()
                .filter_map(|component| match component {
                    Component::Normal(part) => part.to_str().map(ToOwned::to_owned),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if parts.last().map(String::as_str) == Some("__init__") {
                parts.pop();
            }
            if !parts.is_empty() && best.as_ref().map_or(true, |old| parts.len() < old.len()) {
                best = Some(parts);
            }
        }
    }
    best
}

fn split_module(module: &str) -> Vec<String> {
    module
        .split('.')
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn is_module_token(token: &str) -> bool {
    token
        .chars()
        .all(|c| c == '_' || c == '.' || c.is_ascii_alphanumeric())
}

fn is_python_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("py")
}

fn byte_range_to_lsp_range(text: &str, start: usize, end: usize) -> Range {
    Range {
        start: byte_to_position(text, start),
        end: byte_to_position(text, end),
    }
}

fn byte_to_position(text: &str, byte: usize) -> Position {
    let mut line = 0u32;
    let mut col = 0u32;
    for (idx, ch) in text.char_indices() {
        if idx >= byte {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += ch.len_utf16() as u32;
        }
    }
    Position {
        line,
        character: col,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn edit_text(root: &Path, old: &str, new: &str, importer: &str, text: &str) -> String {
        let old_path = root.join(old);
        let new_path = root.join(new);
        write(&old_path, "");
        write(importer.as_ref(), "");
        let renames = module_renames(&[root.to_path_buf()], &[Rename { old_path, new_path }]);
        let module = module_for_path(&[root.to_path_buf()], importer.as_ref()).unwrap();
        let edits = edits_for_text(text, &module, &renames);
        apply_edits(text, edits)
    }

    fn apply_edits(text: &str, mut edits: Vec<Edit>) -> String {
        edits.sort_by_key(|edit| edit.start);
        let mut out = String::new();
        let mut cursor = 0;
        for edit in edits {
            out.push_str(&text[cursor..edit.start]);
            out.push_str(&edit.replacement);
            cursor = edit.end;
        }
        out.push_str(&text[cursor..]);
        out
    }

    #[test]
    fn updates_absolute_import() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "import app.old\n",
        );
        assert_eq!(out, "import app.new\n");
    }

    #[test]
    fn updates_from_module_import() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "from app.old import Thing\n",
        );
        assert_eq!(out, "from app.new import Thing\n");
    }

    #[test]
    fn updates_from_package_import_leaf() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "from app import old as alias\n",
        );
        assert_eq!(out, "from app import new as alias\n");
    }

    #[test]
    fn updates_relative_from_module_import() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "from .old import Thing\n",
        );
        assert_eq!(out, "from .new import Thing\n");
    }

    #[test]
    fn updates_relative_from_package_import_leaf() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "from . import old\n",
        );
        assert_eq!(out, "from . import new\n");
    }

    #[test]
    fn updates_parent_relative_import() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/pkg/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "from ..old import Thing\n",
        );
        assert_eq!(out, "from ..new import Thing\n");
    }

    #[test]
    fn updates_nested_imports_inside_functions() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "def f():\n    from app.old import Thing\n",
        );
        assert_eq!(out, "def f():\n    from app.new import Thing\n");
    }

    #[test]
    fn does_not_update_same_filename_in_other_package() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("other/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "from other.old import Thing\n",
        );
        assert_eq!(out, "from other.old import Thing\n");
    }

    #[test]
    fn understands_src_layout() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("src/app/main.py");
        let out = edit_text(
            tmp.path(),
            "src/app/old.py",
            "src/app/new.py",
            importer.to_str().unwrap(),
            "from app.old import Thing\n",
        );
        assert_eq!(out, "from app.new import Thing\n");
    }
}
