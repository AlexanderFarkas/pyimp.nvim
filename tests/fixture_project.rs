use lsp_types::{Position, WorkspaceEdit};
use pyimp_lsp::{workspace_edit_for_renames, Rename};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &dst_path);
        } else if file_type.is_file() {
            fs::copy(entry.path(), dst_path).unwrap();
        }
    }
}

fn fixture_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rename_project");
    copy_dir_all(&fixture, tmp.path());
    tmp
}

fn rename_with_workspace_edit(project: &Path, old_rel: &str, new_rel: &str) {
    let old_path = project.join(old_rel);
    let new_path = project.join(new_rel);
    let edit = workspace_edit_for_renames(
        &[project.to_path_buf()],
        &[Rename {
            old_path: old_path.clone(),
            new_path: new_path.clone(),
        }],
    )
    .unwrap();
    apply_workspace_edit(edit);
    fs::create_dir_all(new_path.parent().unwrap()).unwrap();
    fs::rename(old_path, new_path).unwrap();
}

fn apply_workspace_edit(edit: WorkspaceEdit) {
    let Some(changes) = edit.changes else {
        return;
    };

    for (url, mut edits) in changes {
        let path = url.to_file_path().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        edits.sort_by_key(|edit| position_to_byte(&text, edit.range.start));
        let mut out = text.clone();
        for edit in edits.into_iter().rev() {
            let start = position_to_byte(&text, edit.range.start);
            let end = position_to_byte(&text, edit.range.end);
            out.replace_range(start..end, &edit.new_text);
        }
        fs::write(path, out).unwrap();
    }
}

fn position_to_byte(text: &str, position: Position) -> usize {
    let mut line = 0u32;
    let mut character = 0u32;
    for (idx, ch) in text.char_indices() {
        if line == position.line && character == position.character {
            return idx;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }
    text.len()
}

fn validate_imports(project: &Path) {
    let python = python_executable();
    let src = project.join("src");
    let script = src.join("acme/tools/import_all.py");
    let output = Command::new(python)
        .arg(script)
        .arg(&src)
        .arg("acme")
        .env("PYTHONPATH", &src)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "import validation failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn python_executable() -> &'static str {
    if Command::new("python3").arg("--version").output().is_ok() {
        "python3"
    } else {
        "python"
    }
}

#[test]
fn fixture_imports_before_any_rename() {
    let project = fixture_project();
    validate_imports(project.path());
}

#[test]
fn fixture_renames_root_module_with_absolute_relative_and_nested_imports() {
    let project = fixture_project();
    rename_with_workspace_edit(project.path(), "src/acme/old.py", "src/acme/new.py");
    validate_imports(project.path());
}

#[test]
fn fixture_moves_root_module_across_packages() {
    let project = fixture_project();
    rename_with_workspace_edit(project.path(), "src/acme/old.py", "src/acme/other/moved.py");
    validate_imports(project.path());
}

#[test]
fn fixture_renames_nested_module_without_touching_same_basename_modules() {
    let project = fixture_project();
    rename_with_workspace_edit(project.path(), "src/acme/pkg/old.py", "src/acme/pkg/new.py");
    validate_imports(project.path());
}

#[test]
fn fixture_renames_module_in_package_named_like_import_keyword() {
    let project = fixture_project();
    rename_with_workspace_edit(
        project.path(),
        "src/acme/importlib/old.py",
        "src/acme/importlib/new.py",
    );
    validate_imports(project.path());
}

#[test]
fn fixture_renames_directory_with_absolute_relative_and_nested_imports() {
    let project = fixture_project();
    rename_with_workspace_edit(project.path(), "src/acme/pkg", "src/acme/renamed_pkg");
    validate_imports(project.path());
}

#[test]
fn fixture_moves_directory_and_rewrites_relative_imports_from_moved_files() {
    let project = fixture_project();
    rename_with_workspace_edit(project.path(), "src/acme/pkg", "src/acme/other/moved_pkg");
    validate_imports(project.path());
}

#[test]
fn fixture_renames_directory_named_like_import_keyword() {
    let project = fixture_project();
    rename_with_workspace_edit(
        project.path(),
        "src/acme/importlib",
        "src/acme/renamed_importlib",
    );
    validate_imports(project.path());
}

#[test]
fn fixture_renames_module_inside_namespace_package_without_init() {
    let project = fixture_project();
    rename_with_workspace_edit(
        project.path(),
        "src/acme/namespace/old.py",
        "src/acme/namespace/new.py",
    );
    validate_imports(project.path());
}

#[test]
fn fixture_renames_namespace_subpackage_directory_without_init() {
    let project = fixture_project();
    rename_with_workspace_edit(
        project.path(),
        "src/acme/namespace/subpkg",
        "src/acme/namespace/renamed_subpkg",
    );
    validate_imports(project.path());
}
