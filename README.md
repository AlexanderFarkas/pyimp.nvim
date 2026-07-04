# pyimp.nvim

Naughty imps messing with your imports while you're renaming your files.

`pyimp.nvim` is a tiny Neovim helper plus Rust LSP sidecar for [`ty`](https://github.com/astral-sh/ty). It listens for `workspace/willRenameFiles` and rewrites Python imports before files or package directories are renamed.

## What it does

- Updates Python imports on file renames.
- Updates imports on package/directory renames.
- Supports absolute and relative imports.
- Handles `src/` layouts and namespace packages.
- Runs as a sidecar to an already-attached `ty` client.

## Install

Build the sidecar binary first:

```sh
git clone https://github.com/YOUR_USER/pyimp.nvim.git
cd pyimp.nvim
cargo build --release
```

Then configure with lazy.nvim:

```lua
{
  "YOUR_USER/pyimp.nvim",
  ft = "python",
  config = function()
    require("pyimp").setup({
      cmd = { "/path/to/pyimp.nvim/target/release/pyimp-lsp" },
    })
  end,
}
```

If `pyimp-lsp` is on your `PATH`:

```lua
{
  "YOUR_USER/pyimp.nvim",
  ft = "python",
  config = function()
    require("pyimp").setup()
  end,
}
```

## Requirements

- Neovim with LSP file-operation support.
- `ty` configured as your Python LSP client.
- Rust toolchain to build `pyimp-lsp`.

## Notes

`pyimp.nvim` does not replace `ty`. It starts only after a `ty` client attaches and reuses `ty`'s root/workspace folders.
