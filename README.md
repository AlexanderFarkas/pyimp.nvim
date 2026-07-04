# pyimp.nvim

Naughty imps messing with your imports while you're renaming your files.

`pyimp.nvim` is a tiny Neovim helper plus Rust LSP sidecar for [`ty`](https://github.com/astral-sh/ty). It listens for `workspace/willRenameFiles` and rewrites Python imports before files or package directories are renamed.

## What it does

- Updates Python imports on file renames.
- Updates imports on package/directory renames.
- Supports absolute and relative imports.
- Handles `src/` layouts and namespace packages.
- Runs as a sidecar to an already-attached `ty` client.

## Install with lazy.nvim

```lua
{
  "AlexanderFarkas/pyimp.nvim",
  ft = "python",
  build = "./scripts/install.sh",
  config = function()
    require("pyimp").setup()
  end,
}
```

The install script downloads a prebuilt `pyimp-lsp` release binary into `bin/pyimp-lsp`. If no matching release binary is available, it falls back to `cargo build --release` and copies the built binary into `bin/`.

If you want to provide your own binary:

```lua
{
  "AlexanderFarkas/pyimp.nvim",
  ft = "python",
  config = function()
    require("pyimp").setup({
      cmd = { "/path/to/pyimp-lsp" },
    })
  end,
}
```

## Requirements

- Neovim with LSP file-operation support.
- `ty` configured as your Python LSP client.
- `curl` or `wget` for release downloads.
- Rust toolchain only if a prebuilt binary is unavailable.

## Releasing

Push a version tag to build release binaries:

```sh
git tag v0.1.0
git push origin v0.1.0
```

GitHub Actions publishes macOS and Linux tarballs named like:

```text
pyimp-lsp-x86_64-apple-darwin.tar.gz
pyimp-lsp-aarch64-apple-darwin.tar.gz
pyimp-lsp-x86_64-unknown-linux-gnu.tar.gz
pyimp-lsp-aarch64-unknown-linux-gnu.tar.gz
```

## Notes

`pyimp.nvim` does not replace `ty`. It starts only after a `ty` client attaches and reuses `ty`'s root/workspace folders.
