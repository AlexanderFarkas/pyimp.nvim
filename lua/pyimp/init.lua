local M = {}

local function default_cmd()
  local source = debug.getinfo(1, "S").source:sub(2)
  local plugin_root = vim.fn.fnamemodify(source, ":p:h:h:h")
  local local_binary = plugin_root .. "/bin/pyimp-lsp"
  if vim.fn.executable(local_binary) == 1 then
    return { local_binary }
  end
  return { "pyimp-lsp" }
end

local defaults = {
  cmd = default_cmd(),
  ty_client_names = { "ty" },
  name = "pyimp-lsp",
  patch_snacks_rename = false,
}

local config = vim.deepcopy(defaults)
local started_by_root = {}

local function is_ty_client(client)
  for _, name in ipairs(config.ty_client_names) do
    if client.name == name then
      return true
    end
  end
  return false
end

local function root_for_client(client)
  return client.config and client.config.root_dir or client.root_dir
end

local function workspace_folders_for_client(client, root_dir)
  if client.workspace_folders and #client.workspace_folders > 0 then
    return vim.tbl_map(function(folder)
      return { uri = folder.uri, name = folder.name }
    end, client.workspace_folders)
  end
  return { { uri = vim.uri_from_fname(root_dir), name = vim.fn.fnamemodify(root_dir, ":t") } }
end

local function pyimp_already_attached(bufnr, root_dir)
  for _, client in ipairs(vim.lsp.get_clients({ bufnr = bufnr, name = config.name })) do
    if root_for_client(client) == root_dir then
      return true
    end
  end
  return false
end

local function start_sidecar(bufnr, ty_client)
  if vim.bo[bufnr].filetype ~= "python" then
    return
  end

  local root_dir = root_for_client(ty_client)
  if not root_dir or root_dir == "" then
    return
  end

  if pyimp_already_attached(bufnr, root_dir) then
    return
  end

  local key = root_dir
  local client_id = started_by_root[key]
  if client_id then
    local client = vim.lsp.get_client_by_id(client_id)
    if client then
      vim.lsp.buf_attach_client(bufnr, client_id)
      return
    end
    started_by_root[key] = nil
  end

  local id = vim.lsp.start({
    name = config.name,
    cmd = config.cmd,
    root_dir = root_dir,
    workspace_folders = workspace_folders_for_client(ty_client, root_dir),
    filetypes = { "python" },
    single_file_support = false,
  }, { bufnr = bufnr })

  if id then
    started_by_root[key] = id
  end
end

local function maybe_start_for_buffer(bufnr)
  if vim.bo[bufnr].filetype ~= "python" then
    return
  end
  for _, client in ipairs(vim.lsp.get_clients({ bufnr = bufnr })) do
    if is_ty_client(client) then
      start_sidecar(bufnr, client)
      return
    end
  end
end


local function position_to_byte(text, position, position_encoding)
  local line_start = 1
  for _ = 1, position.line do
    local newline = text:find("\n", line_start, true)
    if not newline then
      return #text
    end
    line_start = newline + 1
  end

  local line_end = text:find("\n", line_start, true) or (#text + 1)
  local line = text:sub(line_start, line_end - 1)
  local ok, byte = pcall(vim.str_byteindex, line, position.character, position_encoding == "utf-16")
  if not ok then
    byte = #line
  end
  return line_start + byte - 1
end

local function apply_text_edits_to_file(path, edits, position_encoding)
  local file = assert(io.open(path, "rb"))
  local text = file:read("*a")
  file:close()

  local ranges = vim.tbl_map(function(edit)
    return {
      start = position_to_byte(text, edit.range.start, position_encoding),
      finish = position_to_byte(text, edit.range["end"], position_encoding),
      new_text = edit.newText:gsub("\r\n?", "\n"),
    }
  end, edits)

  table.sort(ranges, function(a, b)
    if a.start ~= b.start then
      return a.start > b.start
    end
    return a.finish > b.finish
  end)

  for _, range in ipairs(ranges) do
    text = text:sub(1, range.start) .. range.new_text .. text:sub(range.finish + 1)
  end

  file = assert(io.open(path, "wb"))
  file:write(text)
  file:close()
end

local function apply_text_edits(uri, edits, position_encoding)
  local path = vim.uri_to_fname(uri)
  local existing = vim.fn.bufnr(path)
  if existing >= 0 and vim.api.nvim_buf_is_loaded(existing) then
    vim.lsp.util.apply_text_edits(edits, existing, position_encoding)
    if vim.bo[existing].modified and vim.bo[existing].buftype == "" then
      vim.api.nvim_buf_call(existing, function()
        vim.cmd("silent keepalt write!")
      end)
    end
  else
    apply_text_edits_to_file(path, edits, position_encoding)
  end
end

local function apply_workspace_edit_for_rename(edit, position_encoding)
  if edit.changes then
    for uri, edits in pairs(edit.changes) do
      apply_text_edits(uri, edits, position_encoding)
    end
  end

  if edit.documentChanges then
    for _, change in ipairs(edit.documentChanges) do
      if change.textDocument and change.textDocument.uri and change.edits then
        apply_text_edits(change.textDocument.uri, change.edits, position_encoding)
      else
        vim.lsp.util.apply_workspace_edit({ documentChanges = { change } }, position_encoding)
      end
    end
  end
end

local function patch_snacks_rename()
  local ok, rename_mod = pcall(require, "snacks.rename")
  if not ok or rename_mod.__pyimp_patched then
    return
  end

  rename_mod.__pyimp_patched = true
  rename_mod.on_rename_file = function(from, to, rename)
    local changes = { files = { {
      oldUri = vim.uri_from_fname(from),
      newUri = vim.uri_from_fname(to),
    } } }

    local clients = (vim.lsp.get_clients or vim.lsp.get_active_clients)()
    for _, client in ipairs(clients) do
      if client.supports_method("workspace/willRenameFiles") then
        local resp = client.request_sync("workspace/willRenameFiles", changes, 1000, 0)
        if resp and resp.result ~= nil then
          apply_workspace_edit_for_rename(resp.result, client.offset_encoding)
        end
      end
    end

    if rename then
      rename()
    end

    for _, client in ipairs(clients) do
      if client.supports_method("workspace/didRenameFiles") then
        client.notify("workspace/didRenameFiles", changes)
      end
    end
  end
end

local function setup_snacks_patch()
  if not config.patch_snacks_rename then
    return
  end
  vim.api.nvim_create_autocmd("User", {
    group = vim.api.nvim_create_augroup("pyimp_snacks_rename_patch", { clear = true }),
    pattern = "VeryLazy",
    callback = patch_snacks_rename,
  })
  vim.schedule(patch_snacks_rename)
end

function M.setup(opts)
  config = vim.tbl_deep_extend("force", vim.deepcopy(defaults), opts or {})
  setup_snacks_patch()

  vim.api.nvim_create_autocmd("LspAttach", {
    group = vim.api.nvim_create_augroup("pyimp_sidecar", { clear = true }),
    callback = function(args)
      local client = vim.lsp.get_client_by_id(args.data.client_id)
      if client and is_ty_client(client) then
        start_sidecar(args.buf, client)
      end
    end,
  })

  vim.api.nvim_create_autocmd("FileType", {
    group = vim.api.nvim_create_augroup("pyimp_sidecar_filetype", { clear = true }),
    pattern = "python",
    callback = function(args)
      maybe_start_for_buffer(args.buf)
    end,
  })
end

return M
