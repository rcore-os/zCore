#!/usr/bin/lua5.3

--[[
Utility to convert SCDOC manpages to apk-tools help messages

General:
 - Wrangle *apk-applet*(SECTION) links
 - Uppercase _underlined_ things as they are "keywords"
 - Other format specs like ** to be removed
 - For options text, the first sentence (within the first line) is taken as the help text

Main page: apk.8.scd
 - SYNOPSIS
 - COMMANDS has ## header with a table for commands list
 - GLOBAL OPTIONS and COMMIT OPTIONS for option group help
 - NOTES

Applet pages: apk-*.8.scd
 - Take usage from SYNOPSIS, can have multiple lines like apk-version(8)
 - Take DESCRIPTION, take first paragraph, rewrap, and put as section in applet specific help
 - From OPTIONS take each option and it's first sentence (within the first line)
--]]

local scdoc = {
	usage_prefix = "Usage: ",
}
scdoc.__index = scdoc

function scdoc:nop(ln)
	--print(self.section, ln)
end

function scdoc:SYNOPSIS_text(ln)
	table.insert(self.usage, self.usage_prefix .. ln)
	self.usage_prefix = "   or: "
end

function scdoc:COMMANDS_text(ln)
	ln = ln:gsub("apk%-(%S+)%(%d%)", "%1")
	local ch = ln:sub(1,1)
	local a, b = ln:match("^([[|:<]*)%s+(.+)")
	if ch == '|' then
		self.cur_cmd = { b, "" }
		table.insert(self.commands, self.cur_cmd)
	elseif ch == ':' and self.cur_cmd then
		self.cur_cmd[2] = b
		self.cur_cmd = nil
	end
end

function scdoc:COMMANDS_subsection(n)
	n = n:sub(1,1) .. n:sub(2):lower()
	table.insert(self.commands, n)
end

function scdoc:DESCRIPTION_text(ln)
	table.insert(self.description, ln)
end

function scdoc:DESCRIPTION_paragraph()
	if #self.description > 0 then
		self.section_text = self.nop
	end
end

function scdoc:OPTIONS_text(ln)
	local ch = ln:sub(1,1)
	if ch == '-' then
		self.cur_opt = { ln, {} }
		table.insert(self.options, self.cur_opt)
	elseif ch == '\t' then
		table.insert(self.cur_opt[2], ln:sub(2))
	end
end

function scdoc:NOTES_text(ln)
	table.insert(self.notes, ln)
end

function scdoc:parse_default(ln)
	if #ln == 0 then
		return (self[self.section .. "_paragraph"] or self.nop)(self)
	end
	local s,n = ln:match("^(#*) (.*)")
	if s and n then
		if #s == 1 then
			local optgroup, opts = n:match("^(%S*) ?(OPTIONS)$")
			if opts then
				if #optgroup == 0 then optgroup = self.applet end
				self.options = { name = optgroup }
				table.insert(self.optgroup, self.options)
				n = opts
			end
			self.section = n
			self.section_text = self[n .. "_text"] or self.nop
			self.subsection = nil
		else
			self.subsection = n
			local f = self[self.section.."_subsection"]
			if f then f(self, n) end
		end
		return
	end
	ln = ln:gsub("([^\\])%*(.-[^\\])%*", "%1%2")
	ln = ln:gsub("^%*(.-[^\\])%*", "%1")
	ln = ln:gsub("([^\\a-zA-Z0-9])_(.-[^\\])_([^a-zA-Z0-9])",
		function(a,s,e) return a..s:upper()..e end)
	ln = ln:gsub("([^\\a-zA-Z0-9])_(.-[^\\])_$",
		function(a,s) return a..s:upper() end)
	ln = ln:gsub("^_(.-[^\\])_([^a-zA-Z0-9])",
		function(s,e) return s:upper()..e end)
	ln = ln:gsub("^_(.-[^\\])_$",
		function(s) return s:upper() end)
	ln = ln:gsub("\\", "")
	self:section_text(ln)
end

function scdoc:parse_header(ln)
	self.manpage, self.mansection = ln:match("^(%S*)%((%d*)%)")
	if self.manpage:find("^apk%-") then
		self.applet = self.manpage:sub(5):lower()
	else
		self.applet = self.manpage:upper()
	end
	self.parser = self.parse_default
	self.section_text = self.nop
end

function scdoc:parse(fn)
	self.parser = self.parse_header
	for l in io.lines(fn) do
		self:parser(l)
	end
end

-- Factory to create a fresh scdoc instance
function new_scdoc()
	return setmetatable({
		width = 78,
		section = "HEADER",
		usage = {},
		description = {},
		commands = {},
		notes = {},
		optgroup = {},
	}, scdoc)
end


local scapp = { }
scapp.__index = scapp

function scapp:compress(data)
	local level = 9
	local ok, ret = pcall(function()
		local zlib = require 'zlib'
		if type(zlib.version()) == "string" then
			-- lua-lzlib interface
			return zlib.compress(data, level)
		else
			-- lua-zlib interface
			return zlib.deflate(level)(data, "finish")
		end
	end)
	if not ok then
		local tmp = os.tmpname()
		local f = io.open(tmp, 'w')
		f:write(data)
		f:close()

		local p = io.popen(('gzip -%d < %s'):format(level, tmp), 'r')
		if p ~= nil then
			ret = p:read("*all")
			p:close()
		end
		os.remove(tmp)
	end
	return ret
end

function scapp:main(arg)
	self.format = "apk"
	self.debug = false
	self.enabled_applets = {}

	local f = {}
	for _, fn in ipairs(arg) do
		if fn == '--debug' then
			self.debug = true
		elseif fn == '--format=bash' then
			self.format = "bash"
		else
			doc = new_scdoc()
			doc:parse(fn)
			self.enabled_applets[doc.applet] = true
			table.insert(f, doc)
		end
	end
	table.sort(f, function(a, b) return a.applet < b.applet end)

	local plugin = require(('genhelp_%s'):format(self.format))
	local output = plugin:generate(self, f)
	print(output)
end

scapp:main(arg)
