# pyimp.nvim Design

## Goal

`pyimp.nvim` updates Python imports when Python files are renamed. It is designed to work with any Neovim setup that emits standard LSP file-operation events.

## Shape

The project consists of a shallow LSP server plus a small Neovim plugin. The LSP server owns import-rewrite behavior; the plugin only helps install, configure, and start the server.

## LSP Boundary

The server advertises `workspace/willRenameFiles` for `**/*.py` files and returns a `WorkspaceEdit` with import updates. `workspace/didRenameFiles` may be accepted for bookkeeping, but import edits should be produced before the rename through `willRenameFiles`.

## Neovim Integration

Neovim file explorers or rename commands, such as Snacks rename, trigger `workspace/willRenameFiles`. `pyimp.nvim` should not depend on Snacks directly; it should work with any plugin that uses the LSP file-operation protocol.

## Non-Goals

`pyimp.nvim` is not a type checker, completion server, formatter, or replacement for `ty`, Pyright, or basedpyright. It only handles import updates caused by Python file renames.
