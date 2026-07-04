use ignore::WalkBuilder;
use lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{Stmt, StmtImport, StmtImportFrom};
use ruff_python_parser::parse_module;
use ruff_text_size::TextSize;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rename {
    pub old_path: PathBuf,
    pub new_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModuleRename {
    old: Vec<String>,
    new: Vec<String>,
    old_path: PathBuf,
    new_path: PathBuf,
    is_prefix: bool,
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
        let Some(old_importer_pkg) = importer_package_for_path(roots, &file) else {
            continue;
        };
        let new_file = path_after_renames(&file, &module_renames);
        let Some(new_importer_pkg) = importer_package_for_path(roots, &new_file) else {
            continue;
        };
        let edits = edits_for_text(&text, &old_importer_pkg, &new_importer_pkg, &module_renames);
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
        .filter_map(|rename| {
            if is_python_file(&rename.old_path) && is_python_file(&rename.new_path) {
                let old = module_for_path(roots, &rename.old_path)?;
                let new = module_for_path(roots, &rename.new_path)?;
                return (old != new).then_some(ModuleRename {
                    old,
                    new,
                    old_path: rename.old_path.clone(),
                    new_path: rename.new_path.clone(),
                    is_prefix: false,
                });
            }

            if !rename.old_path.is_dir() {
                return None;
            }
            let old = module_for_dir_path(roots, &rename.old_path)?;
            let new = module_for_dir_path(roots, &rename.new_path)?;
            (old != new).then_some(ModuleRename {
                old,
                new,
                old_path: rename.old_path.clone(),
                new_path: rename.new_path.clone(),
                is_prefix: true,
            })
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
    for rename in renames.iter().filter(|rename| rename.is_prefix) {
        collect_all_python_files(&rename.old_path, &mut files)?;
    }
    for root in roots {
        collect_matching_python_files(root, &patterns, &mut files)?;
    }
    Ok(files.into_iter().collect())
}

fn collect_all_python_files(root: &Path, files: &mut HashSet<PathBuf>) -> std::io::Result<()> {
    if root.is_file() {
        if is_python_file(root) {
            files.insert(root.to_path_buf());
        }
        return Ok(());
    }

    for result in python_walk(root) {
        let Ok(entry) = result else {
            continue;
        };
        let path = entry.path();
        if path.is_file() && is_python_file(path) {
            files.insert(path.to_path_buf());
        }
    }
    Ok(())
}

fn collect_matching_python_files(
    root: &Path,
    patterns: &HashSet<String>,
    files: &mut HashSet<PathBuf>,
) -> std::io::Result<()> {
    if root.is_file() {
        if is_python_file(root) && file_contains_any_pattern(root, patterns)? {
            files.insert(root.to_path_buf());
        }
        return Ok(());
    }

    for result in python_walk(root) {
        let Ok(entry) = result else {
            continue;
        };
        let path = entry.path();
        if path.is_file() && is_python_file(path) && file_contains_any_pattern(path, patterns)? {
            files.insert(path.to_path_buf());
        }
    }
    Ok(())
}

fn python_walk(root: &Path) -> ignore::Walk {
    WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| !is_skipped_dir(entry.path()))
        .build()
}

fn file_contains_any_pattern(path: &Path, patterns: &HashSet<String>) -> std::io::Result<bool> {
    let text = fs::read_to_string(path)?;
    Ok(patterns.iter().any(|pattern| text.contains(pattern)))
}

fn is_skipped_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".git" | ".venv" | "venv" | "__pycache__"))
}

fn edits_for_text(
    text: &str,
    old_importer_pkg: &[String],
    new_importer_pkg: &[String],
    renames: &[ModuleRename],
) -> Vec<Edit> {
    let Ok(parsed) = parse_module(text) else {
        return Vec::new();
    };
    if parsed.has_invalid_syntax() {
        return Vec::new();
    }

    let mut visitor = ImportRewriteVisitor {
        text,
        old_importer_pkg: old_importer_pkg.to_vec(),
        new_importer_pkg: new_importer_pkg.to_vec(),
        renames,
        edits: Vec::new(),
    };
    visitor.visit_body(&parsed.syntax().body);
    visitor.edits
}

struct ImportRewriteVisitor<'a> {
    text: &'a str,
    old_importer_pkg: Vec<String>,
    new_importer_pkg: Vec<String>,
    renames: &'a [ModuleRename],
    edits: Vec<Edit>,
}

impl<'a> Visitor<'a> for ImportRewriteVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(import) => self.visit_import_stmt(import),
            Stmt::ImportFrom(import_from) => self.visit_import_from_stmt(import_from),
            _ => visitor::walk_stmt(self, stmt),
        }
    }
}

impl ImportRewriteVisitor<'_> {
    fn visit_import_stmt(&mut self, import: &StmtImport) {
        for alias in &import.names {
            let module = split_module(alias.name.as_str());
            if let Some(replacement) = renamed_module(&module, self.renames) {
                self.edits.push(Edit {
                    start: text_size_to_usize(alias.name.range.start()),
                    end: text_size_to_usize(alias.name.range.end()),
                    replacement: replacement.join("."),
                });
            }
        }
    }

    fn visit_import_from_stmt(&mut self, import_from: &StmtImportFrom) {
        let level = import_from.level as usize;
        let explicit = import_from
            .module
            .as_ref()
            .map(|module| module.as_str())
            .unwrap_or("");
        let old_resolved_base = resolve_from_module(&self.old_importer_pkg, level, explicit);
        let new_current_base = resolve_from_module(&self.new_importer_pkg, level, explicit);
        let Some((module_start, module_end)) = from_module_range(self.text, import_from) else {
            return;
        };

        let mut module_replacement = renamed_module(&old_resolved_base, self.renames)
            .filter(|desired_base| desired_base != &new_current_base);

        let mut name_edits = Vec::new();
        for alias in &import_from.names {
            if alias.name.as_str() == "*" {
                continue;
            }
            let mut old_full = old_resolved_base.clone();
            old_full.push(alias.name.as_str().to_owned());
            let Some(new_full) = renamed_module(&old_full, self.renames) else {
                continue;
            };
            let Some(new_leaf) = new_full.last() else {
                continue;
            };
            let new_parent = new_full[..new_full.len() - 1].to_vec();
            if new_parent != old_resolved_base {
                module_replacement = Some(new_parent);
            }
            if alias.name.as_str() != new_leaf {
                name_edits.push(Edit {
                    start: text_size_to_usize(alias.name.range.start()),
                    end: text_size_to_usize(alias.name.range.end()),
                    replacement: new_leaf.clone(),
                });
            }
        }

        if module_replacement.is_none()
            && level > 0
            && self.old_importer_pkg != self.new_importer_pkg
            && old_resolved_base != new_current_base
        {
            module_replacement = Some(old_resolved_base);
        }

        if let Some(new_module) = module_replacement {
            let replacement =
                replacement_for_from_module(&self.new_importer_pkg, level, &new_module);
            let current = self.text.get(module_start..module_end).unwrap_or("");
            if replacement != current {
                self.edits.push(Edit {
                    start: module_start,
                    end: module_end,
                    replacement,
                });
            }
        }
        self.edits.extend(name_edits);
    }
}

fn renamed_module(module: &[String], renames: &[ModuleRename]) -> Option<Vec<String>> {
    for rename in renames {
        if rename.is_prefix {
            if module.starts_with(&rename.old) {
                let mut replacement = rename.new.clone();
                replacement.extend_from_slice(&module[rename.old.len()..]);
                return Some(replacement);
            }
        } else if module == rename.old {
            return Some(rename.new.clone());
        }
    }
    None
}

fn path_after_renames(path: &Path, renames: &[ModuleRename]) -> PathBuf {
    for rename in renames.iter().filter(|rename| rename.is_prefix) {
        if let Ok(suffix) = path.strip_prefix(&rename.old_path) {
            return rename.new_path.join(suffix);
        }
    }
    path.to_path_buf()
}

fn from_module_range(text: &str, import_from: &StmtImportFrom) -> Option<(usize, usize)> {
    let stmt_start = text_size_to_usize(import_from.range.start());
    let stmt_end = text_size_to_usize(import_from.range.end());
    let stmt = text.get(stmt_start..stmt_end)?;
    let from_pos = stmt.find("from")?;
    let after_from = from_pos + "from".len();
    let import_pos = find_import_keyword(stmt, after_from)?;
    let module_segment = &stmt[after_from..import_pos];
    let leading = module_segment.len() - module_segment.trim_start().len();
    let trailing = module_segment.trim_end().len();
    Some((
        stmt_start + after_from + leading,
        stmt_start + after_from + trailing,
    ))
}

fn find_import_keyword(stmt: &str, start: usize) -> Option<usize> {
    let bytes = stmt.as_bytes();
    let mut idx = start;
    while idx + "import".len() <= stmt.len() {
        if stmt
            .get(idx..)
            .is_some_and(|suffix| suffix.starts_with("import"))
        {
            let before_is_space = idx > 0 && bytes[idx - 1].is_ascii_whitespace();
            let after = idx + "import".len();
            let after_is_space = after < stmt.len() && bytes[after].is_ascii_whitespace();
            if before_is_space && after_is_space {
                return Some(idx);
            }
        }
        idx += 1;
    }
    None
}

fn text_size_to_usize(size: TextSize) -> usize {
    u32::from(size) as usize
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

fn importer_package_for_path(roots: &[PathBuf], path: &Path) -> Option<Vec<String>> {
    let module = module_for_path(roots, path)?;
    if path.file_name().and_then(|name| name.to_str()) == Some("__init__.py") {
        Some(module)
    } else {
        Some(module[..module.len().saturating_sub(1)].to_vec())
    }
}

fn module_for_dir_path(roots: &[PathBuf], path: &Path) -> Option<Vec<String>> {
    module_parts_for_relative_path(roots, path, false)
}

fn module_for_path(roots: &[PathBuf], path: &Path) -> Option<Vec<String>> {
    module_parts_for_relative_path(roots, path, true)
}

fn module_parts_for_relative_path(
    roots: &[PathBuf],
    path: &Path,
    require_python_file: bool,
) -> Option<Vec<String>> {
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
            if require_python_file
                && candidate.extension().and_then(|ext| ext.to_str()) != Some("py")
            {
                continue;
            }
            let module_path = if require_python_file {
                candidate.with_extension("")
            } else {
                candidate
            };
            let mut parts = module_path
                .components()
                .filter_map(|component| match component {
                    Component::Normal(part) => part.to_str().map(ToOwned::to_owned),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if parts.last().map(String::as_str) == Some("__init__") {
                parts.pop();
            }
            if !parts.is_empty() && best.as_ref().is_none_or(|old| parts.len() < old.len()) {
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
        write(&root.join(old), "");
        edit_text_with_rename(root, old, new, importer, text)
    }

    fn edit_text_with_dir_rename(
        root: &Path,
        old: &str,
        new: &str,
        importer: &str,
        text: &str,
    ) -> String {
        fs::create_dir_all(root.join(old)).unwrap();
        edit_text_with_rename(root, old, new, importer, text)
    }

    fn edit_text_with_rename(
        root: &Path,
        old: &str,
        new: &str,
        importer: &str,
        text: &str,
    ) -> String {
        let old_path = root.join(old);
        let new_path = root.join(new);
        write(importer.as_ref(), "");
        let roots = vec![root.to_path_buf()];
        let rename_inputs = vec![Rename { old_path, new_path }];
        let renames = module_renames(&roots, &rename_inputs);
        let importer_pkg = importer_package_for_path(&roots, importer.as_ref()).unwrap();
        let new_importer = path_after_renames(importer.as_ref(), &renames);
        let new_importer_pkg = importer_package_for_path(&roots, &new_importer).unwrap();
        let edits = edits_for_text(text, &importer_pkg, &new_importer_pkg, &renames);
        apply_edits(text, edits)
    }

    fn apply_edits(text: &str, mut edits: Vec<Edit>) -> String {
        edits.sort_by_key(|edit| edit.start);
        let mut out = String::new();
        let mut cursor = 0;
        for edit in edits {
            out.push_str(text.get(cursor..edit.start).unwrap());
            out.push_str(&edit.replacement);
            cursor = edit.end;
        }
        out.push_str(text.get(cursor..).unwrap());
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

    #[test]
    fn updates_multiple_imports_on_one_line() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "import app.other, app.old as old_alias\nfrom app import other, old as alias\n",
        );
        assert_eq!(
            out,
            "import app.other, app.new as old_alias\nfrom app import other, new as alias\n"
        );
    }

    #[test]
    fn updates_star_import_module_path() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "from app.old import *\n",
        );
        assert_eq!(out, "from app.new import *\n");
    }

    #[test]
    fn updates_package_init_rename() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old/__init__.py",
            "app/new/__init__.py",
            importer.to_str().unwrap(),
            "import app.old\nfrom app.old import Thing\nfrom app import old\n",
        );
        assert_eq!(
            out,
            "import app.new\nfrom app.new import Thing\nfrom app import new\n"
        );
    }

    #[test]
    fn updates_move_across_packages_absolute_and_from_leaf() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "other/new.py",
            importer.to_str().unwrap(),
            "from app.old import Thing\nfrom app import old as alias\n",
        );
        assert_eq!(
            out,
            "from other.new import Thing\nfrom other import new as alias\n"
        );
    }

    #[test]
    fn updates_relative_import_when_move_requires_absolute() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/pkg/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "other/new.py",
            importer.to_str().unwrap(),
            "from .. import old\nfrom ..old import Thing\n",
        );
        assert_eq!(out, "from other import new\nfrom other.new import Thing\n");
    }

    #[test]
    fn updates_move_to_same_basename_in_different_package() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "other/old.py",
            importer.to_str().unwrap(),
            "from app import old\nfrom app.old import Thing\n",
        );
        assert_eq!(out, "from other import old\nfrom other.old import Thing\n");
    }

    #[test]
    fn ignores_comments_strings_and_triple_quoted_strings() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "# from app.old import X\nx = 'from app.old import X'\n\"\"\"\nfrom app.old import X\n\"\"\"\nfrom app.old import X  # real\n",
        );
        assert_eq!(
            out,
            "# from app.old import X\nx = 'from app.old import X'\n\"\"\"\nfrom app.old import X\n\"\"\"\nfrom app.new import X  # real\n"
        );
    }

    #[test]
    fn updates_multiline_from_package_import() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "from app import (\n    other,\n    old as alias,\n)\n",
        );
        assert_eq!(out, "from app import (\n    other,\n    new as alias,\n)\n");
    }

    #[test]
    fn updates_deep_parent_relative_import() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/pkg/deep/main.py");
        let out = edit_text(
            tmp.path(),
            "app/old.py",
            "app/new.py",
            importer.to_str().unwrap(),
            "from ...old import Thing\n",
        );
        assert_eq!(out, "from ...new import Thing\n");
    }

    #[test]
    fn handles_modules_named_like_import_keyword_prefix() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text(
            tmp.path(),
            "importlib/old.py",
            "importlib/new.py",
            importer.to_str().unwrap(),
            "from importlib.old import Thing\nfrom importlib import old\n",
        );
        assert_eq!(
            out,
            "from importlib.new import Thing\nfrom importlib import new\n"
        );
    }

    #[test]
    fn updates_absolute_import_for_directory_rename() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text_with_dir_rename(
            tmp.path(),
            "app/pkg",
            "app/renamed",
            importer.to_str().unwrap(),
            "import app.pkg.old\nfrom app.pkg.old import Thing\n",
        );
        assert_eq!(
            out,
            "import app.renamed.old\nfrom app.renamed.old import Thing\n"
        );
    }

    #[test]
    fn updates_from_package_leaf_for_directory_rename() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        let out = edit_text_with_dir_rename(
            tmp.path(),
            "app/pkg",
            "app/renamed",
            importer.to_str().unwrap(),
            "from app.pkg import old as alias\nfrom app import pkg\n",
        );
        assert_eq!(
            out,
            "from app.renamed import old as alias\nfrom app import renamed\n"
        );
    }

    #[test]
    fn keeps_relative_imports_inside_renamed_directory_when_relationship_is_unchanged() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/pkg/deep/main.py");
        let out = edit_text_with_dir_rename(
            tmp.path(),
            "app/pkg",
            "app/renamed",
            importer.to_str().unwrap(),
            "from ..old import Thing\nfrom .. import old\n",
        );
        assert_eq!(out, "from ..old import Thing\nfrom .. import old\n");
    }

    #[test]
    fn updates_relative_imports_inside_moved_directory_when_external_relationship_changes() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/pkg/consumer.py");
        write(&tmp.path().join("app/old.py"), "");
        let out = edit_text_with_dir_rename(
            tmp.path(),
            "app/pkg",
            "other/renamed",
            importer.to_str().unwrap(),
            "from ..old import Thing\nfrom .. import old\nfrom .local import Local\n",
        );
        assert_eq!(
            out,
            "from app.old import Thing\nfrom app import old\nfrom .local import Local\n"
        );
    }

    #[test]
    fn handles_multiple_file_renames_in_one_request() {
        let tmp = TempDir::new().unwrap();
        let importer = tmp.path().join("app/main.py");
        write(&tmp.path().join("app/old.py"), "");
        write(&tmp.path().join("app/pkg/stale.py"), "");
        write(&importer, "");
        let roots = vec![tmp.path().to_path_buf()];
        let renames = vec![
            Rename {
                old_path: tmp.path().join("app/old.py"),
                new_path: tmp.path().join("app/new.py"),
            },
            Rename {
                old_path: tmp.path().join("app/pkg/stale.py"),
                new_path: tmp.path().join("app/pkg/fresh.py"),
            },
        ];
        let module_renames = module_renames(&roots, &renames);
        let old_pkg = importer_package_for_path(&roots, &importer).unwrap();
        let out = apply_edits(
            "from app.old import Thing\nfrom app.pkg import stale\n",
            edits_for_text(
                "from app.old import Thing\nfrom app.pkg import stale\n",
                &old_pkg,
                &old_pkg,
                &module_renames,
            ),
        );
        assert_eq!(
            out,
            "from app.new import Thing\nfrom app.pkg import fresh\n"
        );
    }

    #[test]
    fn skips_invalid_python_file_but_rewrites_valid_importer() {
        let tmp = TempDir::new().unwrap();
        let old_path = tmp.path().join("app/old.py");
        let new_path = tmp.path().join("app/new.py");
        let valid = tmp.path().join("app/valid.py");
        let invalid = tmp.path().join("app/invalid.py");
        write(&old_path, "");
        write(&valid, "from app.old import Thing\n");
        write(&invalid, "from app.old import Thing\ndef broken(:\n");

        let roots = vec![tmp.path().to_path_buf()];
        let renames = vec![Rename { old_path, new_path }];
        let edit = workspace_edit_for_renames(&roots, &renames).unwrap();
        let changes = edit.changes.unwrap();
        assert!(changes.contains_key(&Url::from_file_path(valid).unwrap()));
        assert!(!changes.contains_key(&Url::from_file_path(invalid).unwrap()));
    }

    #[test]
    fn computes_lsp_ranges_after_unicode_as_utf16_positions() {
        let text = "emoji = '🦀'\nfrom app.old import Thing\n";
        let importer_pkg = vec!["app".to_owned()];
        let renames = vec![ModuleRename {
            old: split_module("app.old"),
            new: split_module("app.new"),
            old_path: PathBuf::from("app/old.py"),
            new_path: PathBuf::from("app/new.py"),
            is_prefix: false,
        }];
        let edit = edits_for_text(text, &importer_pkg, &importer_pkg, &renames)
            .into_iter()
            .next()
            .unwrap();
        let range = byte_range_to_lsp_range(text, edit.start, edit.end);
        assert_eq!(range.start.line, 1);
        assert_eq!(range.start.character, 5);
        assert_eq!(range.end.character, 12);
    }

    #[test]
    fn ignores_non_python_file_renames_and_outside_workspace_renames() {
        let tmp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        write(
            &tmp.path().join("app/main.py"),
            "from app.old import Thing\n",
        );
        let roots = vec![tmp.path().to_path_buf()];
        let renames = vec![
            Rename {
                old_path: tmp.path().join("app/old.txt"),
                new_path: tmp.path().join("app/new.txt"),
            },
            Rename {
                old_path: outside.path().join("app/old.py"),
                new_path: outside.path().join("app/new.py"),
            },
        ];
        let edit = workspace_edit_for_renames(&roots, &renames).unwrap();
        assert!(edit.changes.unwrap().is_empty());
    }

    #[test]
    fn returns_workspace_edit_for_directory_rename_importers() {
        let tmp = TempDir::new().unwrap();
        let old_path = tmp.path().join("app/pkg");
        let new_path = tmp.path().join("app/renamed");
        let importing = tmp.path().join("app/importing.py");
        let unrelated = tmp.path().join("other/importing.py");
        fs::create_dir_all(&old_path).unwrap();
        write(&old_path.join("old.py"), "");
        write(&importing, "from app.pkg.old import Thing\n");
        write(&unrelated, "from other.pkg.old import Thing\n");

        let roots = vec![tmp.path().to_path_buf()];
        let renames = vec![Rename { old_path, new_path }];
        let edit = workspace_edit_for_renames(&roots, &renames).unwrap();
        let changes = edit.changes.unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes.contains_key(&Url::from_file_path(importing).unwrap()));
    }

    #[test]
    fn returns_workspace_edit_for_only_actual_importers() {
        let tmp = TempDir::new().unwrap();
        let old_path = tmp.path().join("app/old.py");
        let new_path = tmp.path().join("app/new.py");
        let importing = tmp.path().join("app/importing.py");
        let unrelated = tmp.path().join("other/importing.py");
        write(&old_path, "");
        write(&importing, "from app.old import Thing\n");
        write(&unrelated, "from other.old import Thing\n");

        let roots = vec![tmp.path().to_path_buf()];
        let renames = vec![Rename { old_path, new_path }];
        let edit = workspace_edit_for_renames(&roots, &renames).unwrap();
        let changes = edit.changes.unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes.contains_key(&Url::from_file_path(importing).unwrap()));
    }
}
