use lsp_server::{Connection, Message, Request, Response};
use lsp_types::{
    request::Request as LspRequest, FileOperationFilter, FileOperationPattern,
    FileOperationRegistrationOptions, InitializeParams, RenameFilesParams, ServerCapabilities,
    TextDocumentSyncCapability, Url, WorkspaceFileOperationsServerCapabilities,
    WorkspaceServerCapabilities,
};
use pyimp_lsp::{workspace_edit_for_renames, Rename};
use std::path::PathBuf;

struct WillRenameFiles;
impl LspRequest for WillRenameFiles {
    type Params = RenameFilesParams;
    type Result = Option<lsp_types::WorkspaceEdit>;
    const METHOD: &'static str = "workspace/willRenameFiles";
}

fn main() -> anyhow_free::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    let (id, params) = connection.initialize_start()?;
    let init: InitializeParams = serde_json::from_value(params)?;
    let roots = workspace_roots(&init);

    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            lsp_types::TextDocumentSyncKind::NONE,
        )),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: None,
            file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                did_create: None,
                will_create: None,
                did_rename: None,
                will_rename: Some(FileOperationRegistrationOptions {
                    filters: vec![FileOperationFilter {
                        scheme: Some("file".to_owned()),
                        pattern: FileOperationPattern {
                            glob: "**/*.py".to_owned(),
                            matches: None,
                            options: None,
                        },
                    }],
                }),
                did_delete: None,
                will_delete: None,
            }),
        }),
        ..ServerCapabilities::default()
    };
    connection.initialize_finish(id, serde_json::to_value(capabilities)?)?;

    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    break;
                }
                handle_request(&connection, request, &roots)?;
            }
            Message::Response(_) | Message::Notification(_) => {}
        }
    }

    io_threads.join()?;
    Ok(())
}

fn handle_request(
    connection: &Connection,
    request: Request,
    roots: &[PathBuf],
) -> anyhow_free::Result<()> {
    if request.method == WillRenameFiles::METHOD {
        let id = request.id.clone();
        let params: RenameFilesParams = serde_json::from_value(request.params)?;
        let renames = params
            .files
            .into_iter()
            .filter_map(|file| {
                Some(Rename {
                    old_path: uri_string_to_path(&file.old_uri)?,
                    new_path: uri_string_to_path(&file.new_uri)?,
                })
            })
            .collect::<Vec<_>>();
        let result = workspace_edit_for_renames(roots, &renames).ok();
        connection.sender.send(Message::Response(Response {
            id,
            result: Some(serde_json::to_value(result)?),
            error: None,
        }))?;
    } else {
        connection.sender.send(Message::Response(Response {
            id: request.id,
            result: None,
            error: Some(lsp_server::ResponseError {
                code: lsp_server::ErrorCode::MethodNotFound as i32,
                message: format!("unsupported request {}", request.method),
                data: None,
            }),
        }))?;
    }
    Ok(())
}

fn workspace_roots(init: &InitializeParams) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(folders) = &init.workspace_folders {
        roots.extend(folders.iter().filter_map(|folder| url_to_path(&folder.uri)));
    }
    if let Some(root_uri) = &init.root_uri {
        if let Some(path) = url_to_path(root_uri) {
            roots.push(path);
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn url_to_path(url: &Url) -> Option<PathBuf> {
    url.to_file_path().ok()
}

fn uri_string_to_path(uri: &str) -> Option<PathBuf> {
    Url::parse(uri).ok()?.to_file_path().ok()
}

mod anyhow_free {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
}
