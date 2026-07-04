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


function M.setup(opts)
  config = vim.tbl_deep_extend("force", vim.deepcopy(defaults), opts or {})

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
