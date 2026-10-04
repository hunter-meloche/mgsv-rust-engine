-- Runtime prelude for the Fox Lua host.
--
-- Native modules that have no Rust implementation yet resolve to "phantoms":
-- values that can be indexed, called and compared without erroring, and that
-- report every use to the host. This lets the original scripts run as far as
-- possible while producing a list of what still has to be implemented.

local trace = __fox_trace
local phantom_mt = {}
local phantom_path = setmetatable({}, { __mode = "k" })

local function phantom(path)
  local p = setmetatable({}, phantom_mt)
  phantom_path[p] = path
  return p
end

phantom_mt.__index = function(self, key)
  local child = phantom(phantom_path[self] .. "." .. tostring(key))
  rawset(self, key, child)
  return child
end
-- Unimplemented predicates follow the headless-engine rule "every
-- asynchronous operation has already finished": completion checks
-- (IsEnd*, IsFinish*, IsReady*, IsLoaded*, ...) answer true and every other
-- predicate (IsSaving*, IsPlaying*, IsShow*, Has*, ...) answers false. Both
-- `while busy() do yield end` and `while not done() do yield end` loops then
-- terminate. Everything else returns a further phantom.
local function predicate_result(path)
  local last = path:match("([%w_]+)$") or ""
  if not (last:match("^Is%u") or last:match("^Has%u") or last:match("^Can%u")
      or last:match("^Check%u") or last:match("^Exist")) then
    return nil
  end
  for _, done in ipairs({ "End", "Finish", "Complete", "Done", "Ready", "Loaded", "Initialized", "Ended", "Ok", "Active" }) do
    if last:match("^Is" .. done) or last:match("^Is%u%w*" .. done .. "$") then
      return true
    end
  end
  return false
end

phantom_mt.__call = function(self, ...)
  local path = phantom_path[self]
  trace(path, select("#", ...))
  local p = predicate_result(path)
  if p ~= nil then
    return p
  end
  return phantom(path .. "()")
end
phantom_mt.__tostring = function(self)
  return "<phantom " .. phantom_path[self] .. ">"
end
phantom_mt.__concat = function(a, b)
  return tostring(a) .. tostring(b)
end
phantom_mt.__unm = function() return 0 end
phantom_mt.__len = function() return 0 end
for _, op in ipairs({ "__add", "__sub", "__mul", "__div", "__mod", "__pow" }) do
  phantom_mt[op] = function() return 0 end
end

__fox_phantom = phantom
__fox_is_phantom = function(v) return phantom_path[v] ~= nil end

-- `vars` is engine-owned state (player, clock, ...) and `mvars` is plain
-- script state. gvars/svars are typed stores installed from Rust.
vars = vars or {}
mvars = mvars or {}

setmetatable(_G, {
  __index = function(t, name)
    if type(name) == "string" and name:match("^[A-Z]") then
      local p = phantom(name)
      rawset(t, name, p)
      return p
    end
    return nil
  end,
})
