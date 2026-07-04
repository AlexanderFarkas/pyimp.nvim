use lsp_types::{
    request::{Initialize, Request as LspRequest},
    InitializeParams, RenameFilesParams, Url, WorkspaceEdit,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use tempfile::TempDir;

struct LspChild {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Drop for LspChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_file(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn spawn_lsp(root: &Path) -> LspChild {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pyimp-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lsp = LspChild {
        child,
        stdin,
        stdout,
    };

    let root_uri = Url::from_directory_path(root).unwrap();
    send_request(
        &mut lsp.stdin,
        1,
        Initialize::METHOD,
        serde_json::to_value(InitializeParams {
            root_uri: Some(root_uri.clone()),
            workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
                uri: root_uri,
                name: "fixture".to_owned(),
            }]),
            ..InitializeParams::default()
        })
        .unwrap(),
    );
    let response = read_message(&mut lsp.stdout);
    assert_eq!(response["id"], 1);
    send_notification(&mut lsp.stdin, "initialized", json!({}));
    lsp
}

fn send_request(stdin: &mut ChildStdin, id: i64, method: &str, params: Value) {
    let message = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let body = serde_json::to_vec(&message).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn send_notification(stdin: &mut ChildStdin, method: &str, params: Value) {
    let message = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    let body = serde_json::to_vec(&message).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn read_response<T: DeserializeOwned>(stdout: &mut ChildStdout) -> T {
    serde_json::from_value(read_message(stdout)["result"].clone()).unwrap()
}

fn read_message(stdout: &mut ChildStdout) -> Value {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        stdout.read_exact(&mut byte).unwrap();
        header.extend_from_slice(&byte);
    }
    let header = String::from_utf8(header).unwrap();
    let content_length = header
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let mut body = vec![0u8; content_length];
    stdout.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn initialize_advertises_file_and_folder_rename_capabilities() {
    let tmp = TempDir::new().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pyimp-lsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let root_uri = Url::from_directory_path(tmp.path()).unwrap();
    send_request(
        &mut stdin,
        1,
        Initialize::METHOD,
        serde_json::to_value(InitializeParams {
            root_uri: Some(root_uri),
            ..InitializeParams::default()
        })
        .unwrap(),
    );
    let response = read_message(&mut stdout);
    assert_eq!(response["result"]["serverInfo"]["name"], "pyimp-lsp");
    let filters =
        &response["result"]["capabilities"]["workspace"]["fileOperations"]["willRename"]["filters"];
    assert!(filters.as_array().unwrap().iter().any(|filter| {
        filter["pattern"]["glob"] == "**/*.py" && filter["pattern"]["matches"] == "file"
    }));
    assert!(filters.as_array().unwrap().iter().any(|filter| {
        filter["pattern"]["glob"] == "**" && filter["pattern"]["matches"] == "folder"
    }));
    let _ = child.kill();
}

#[test]
fn will_rename_files_returns_workspace_edit_for_file_and_directory() {
    let tmp = TempDir::new().unwrap();
    write_file(&tmp.path().join("app/old.py"), "class Thing: pass\n");
    write_file(&tmp.path().join("app/pkg/__init__.py"), "");
    write_file(&tmp.path().join("app/pkg/mod.py"), "VALUE = 1\n");
    write_file(
        &tmp.path().join("app/main.py"),
        "from app.old import Thing\nfrom app.pkg.mod import VALUE\n",
    );

    let mut lsp = spawn_lsp(tmp.path());
    send_request(
        &mut lsp.stdin,
        2,
        "workspace/willRenameFiles",
        serde_json::to_value(RenameFilesParams {
            files: vec![
                lsp_types::FileRename {
                    old_uri: Url::from_file_path(tmp.path().join("app/old.py"))
                        .unwrap()
                        .to_string(),
                    new_uri: Url::from_file_path(tmp.path().join("app/new.py"))
                        .unwrap()
                        .to_string(),
                },
                lsp_types::FileRename {
                    old_uri: Url::from_file_path(tmp.path().join("app/pkg"))
                        .unwrap()
                        .to_string(),
                    new_uri: Url::from_file_path(tmp.path().join("app/renamed"))
                        .unwrap()
                        .to_string(),
                },
            ],
        })
        .unwrap(),
    );
    let edit: Option<WorkspaceEdit> = read_response(&mut lsp.stdout);
    let changes = edit.unwrap().changes.unwrap();
    let main_url = Url::from_file_path(tmp.path().join("app/main.py")).unwrap();
    let edits = changes.get(&main_url).unwrap();
    assert_eq!(edits.len(), 2);
    assert!(edits.iter().any(|edit| edit.new_text == "app.new"));
    assert!(edits.iter().any(|edit| edit.new_text == "app.renamed.mod"));
}
