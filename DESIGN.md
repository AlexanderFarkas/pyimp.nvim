# pyimp.nvim Design

## Goal

`pyimp.nvim` updates Python imports when Python files are renamed.

It exists specifically as a **sidecar for `ty`**: the missing feature it provides is import rewriting for LSP file-operation renames. It is not intended to be a standalone Python language server, type checker, completion engine, formatter, or replacement for `ty`, Pyright, or basedpyright.

## High-Level Shape

The project has two parts:

1. A shallow Rust LSP server, `pyimp-lsp`.
2. A small Neovim plugin that starts `pyimp-lsp` only next to an already running `ty` client.

The Rust LSP server owns all import-rewrite behavior. The Lua plugin should stay thin: it discovers the attached `ty` client, mirrors its root/workspace configuration, and starts or attaches the sidecar.

## Sidecar Relationship With `ty`

`pyimp-lsp` should be treated as a sidecar to `ty`, not as an independently configured server.

The intended Neovim shape is:

```text
Neovim
├─ ty LSP client
│  └─ owns root/workspace semantics
└─ pyimp-lsp sidecar
   └─ mirrors ty root/workspace folders
```

The sidecar should not invent its own root detection. In normal plugin usage:

- `ty` starts first.
- `pyimp.nvim` observes `ty` via `LspAttach`.
- `pyimp.nvim` starts `pyimp-lsp` with the same root directory and workspace folders.
- `pyimp-lsp` attaches to Python buffers in that root.

Manual/standalone startup may be useful for tests or debugging, but it is not the primary product behavior.

## LSP Boundary

The server advertises `workspace/willRenameFiles` for Python files:

```text
**/*.py
```

On `workspace/willRenameFiles`, it returns a `WorkspaceEdit` containing import updates that should be applied before the file is physically renamed.

`workspace/didRenameFiles` may be accepted later for bookkeeping, but import edits should be produced in `willRenameFiles`.

## Rename Scope

The initial scope is Python file renames:

```text
old.py -> new.py
pkg/old.py -> other/new.py
pkg/old/__init__.py -> pkg/new/__init__.py
```

Directory/package rename support is desirable, but should be designed deliberately rather than accidentally inferred. The implementation should keep path/module mapping logic general enough to add directory renames later.

## Import Forms To Support

The sidecar should support all ordinary Python import forms that can refer to a renamed module:

```python
import pkg.old
import pkg.old as old_alias
import pkg.other, pkg.old as old_alias

from pkg.old import Thing
from pkg.old import *
from pkg import old
from pkg import old as old_alias
from pkg import (
    other,
    old as old_alias,
)

from .old import Thing
from . import old
from ..old import Thing
from ...old import Thing
```

Imports may appear at top level or nested inside functions, classes, conditionals, or other statement bodies.

The rewrite should preserve formatting as much as possible and replace only the module/name range that must change.

## Absolute, Relative, and Cross-Package Renames

The server must resolve imports semantically before editing them. It should not edit merely because a line contains the same basename.

Examples:

```text
app/old.py -> app/new.py
```

```python
from app.old import X      # -> from app.new import X
from app import old        # -> from app import new
from .old import X         # -> from .new import X
from . import old          # -> from . import new
```

Cross-package moves must update both the parent module and the imported leaf:

```text
app/old.py -> other/new.py
```

```python
from app.old import X      # -> from other.new import X
from app import old        # -> from other import new
from ..old import X        # -> from other.new import X, if relative form no longer fits
```

Same-basename moves must still update the parent:

```text
app/old.py -> other/old.py
```

```python
from app import old        # -> from other import old
from app.old import X      # -> from other.old import X
```

## Avoiding False Positives

The sidecar must not rewrite imports that resolve to a different file with the same basename.

Example:

```text
app/old.py -> app/new.py
other/old.py remains unchanged
```

```python
from other.old import X    # must not change
```

It must also avoid comments and strings:

```python
# from app.old import X      # must not change
x = "from app.old import X" # must not change
"""
from app.old import X        # must not change
"""
```

## Parser Strategy

Candidate file discovery and authoritative import understanding are separate concerns.

### Candidate Discovery

Use `rg` as a fast prefilter to find Python files that contain any relevant textual patterns, such as:

- the old absolute module path, e.g. `app.old`
- the old leaf name, e.g. `old`

This keeps rename handling fast on large workspaces.

If `rg` is unavailable or fails, the server may fall back to walking Python files under the workspace roots.

### Import Understanding

Use Ruff's Python parser crates for syntax-aware import detection and edit ranges:

```toml
ruff_python_parser = "=..."
ruff_python_ast = "=..."
ruff_text_size = "=..."
```

Ruff is the natural fit because it is Rust-native, Python-aware, and avoids an external CLI dependency. However, Ruff's parser crates are internal component crates and do not currently promise stable public APIs. Therefore:

- pin exact Ruff crate versions;
- wrap all parser usage behind a small internal module/API;
- upgrade deliberately;
- keep fixture integration tests broad enough to catch parser/API behavior changes.

`ast-grep` is also Python-aware, but using it as a CLI would add an external runtime dependency and complicate installation/Mason packaging. Ruff-as-library is preferred.

## Module Resolution

The sidecar should derive module names from workspace roots supplied by `ty`.

It should support common layouts, including:

```text
project/
  app/
    old.py

project/
  src/
    app/
      old.py
```

For `src/` layout, imports should resolve as `app.old`, not `src.app.old`.

The implementation should be conservative: only rewrite when an import resolves to the renamed module under the active workspace root.

## Neovim Integration

The Lua plugin should expose a minimal setup function:

```lua
require("pyimp").setup({
  cmd = { "pyimp-lsp" },
  ty_client_names = { "ty" },
})
```

Behavior:

- listen for `LspAttach`;
- when an attached client name matches `ty_client_names`, read its root/workspace folders;
- start one `pyimp-lsp` per `ty` root;
- attach additional Python buffers in that root to the existing sidecar;
- do not start for non-Python buffers;
- do not start standalone before `ty`.

The plugin should not depend on Snacks or any particular file explorer. Any Neovim rename implementation that emits standard LSP file-operation events should work.

## Mason / Distribution

A Mason wrapper is not required for the core implementation.

Initial setup can let users provide the binary path manually:

```lua
require("pyimp").setup({
  cmd = { "/path/to/pyimp-lsp" },
})
```

Mason packaging can be added later so `cmd = { "pyimp-lsp" }` works after installation.

## Testing Strategy

Testing should have two layers.

### Unit Tests

Rust unit tests should cover exact rewrite behavior for focused cases:

- absolute imports;
- relative imports;
- parent-relative imports;
- nested imports;
- aliases;
- star imports;
- multiple imports on one line;
- multiline parenthesized imports;
- `__init__.py` package renames;
- cross-package moves;
- same-basename moves;
- modules with names that include strings like `import` / `importlib`;
- comments, strings, and docstrings as false positives;
- same filename in another package as a false positive.

### Fixture Integration Tests

Add a custom dummy Python project fixture rather than depending on a third-party repository.

A hand-written fixture is preferable because it can deliberately include every edge case without external dependencies or upstream churn.

Suggested shape:

```text
tests/fixtures/rename_project/
  pyproject.toml
  src/
    acme/
      __init__.py
      main.py
      old.py
      pkg/
        __init__.py
        consumer.py
        deep/
          __init__.py
          consumer.py
      other/
        __init__.py
        old.py
      importlib/
        __init__.py
        old.py
```

Each integration test should:

1. copy the fixture project into a temporary directory;
2. request rename edits from the Rust logic or through an LSP `workspace/willRenameFiles` request;
3. apply the returned `WorkspaceEdit`;
4. physically rename the file;
5. run a Python validation script with `PYTHONPATH` pointed at the temp `src/` directory;
6. assert every package module imports successfully.

Because the fixture is controlled by us, importing every module is acceptable. Running `compileall` separately is unnecessary: importing modules compiles them and validates both syntax and import resolution.

The validation script can:

- walk `src/acme/**/*.py`;
- convert file paths to module names;
- call `importlib.import_module(module_name)`;
- fail on any exception.

This catches bad edit ranges, broken relative imports, missing cross-package rewrites, and accidental false positives in a way unit tests alone cannot.

## Robustness Requirements

The implementation should optimize for rename stability:

- apply edits only when parser-backed import nodes resolve to the renamed module;
- keep `rg` as a prefilter, not the source of truth;
- preserve formatting by editing minimal ranges;
- pin Ruff parser crate versions;
- add broad fixture tests before expanding rename scope;
- keep the LSP server narrow and predictable;
- keep Neovim integration coupled to `ty` roots.

## Non-Goals

`pyimp.nvim` is not:

- a type checker;
- a completion server;
- a formatter;
- a general refactoring engine;
- a replacement for `ty`, Pyright, or basedpyright;
- tied to Snacks or any specific Neovim file explorer.

Its only product responsibility is updating Python imports caused by Python file renames that are emitted through the standard LSP file-operation protocol.
