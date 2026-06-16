r2kad = Proto("r2kad", "R²/KAD Protocol")

local cbor_dissector = Dissector.get("cbor")

function r2kad.dissector(buffer, pinfo, tree)
	pinfo.cols.protocol = "R²/KAD"
	local subtree = tree:add(r2kad, buffer(), "R²/KAD Protocol Message")
	cbor_dissector:call(buffer, pinfo, subtree)
end

udp_table = DissectorTable.get("udp.port")
udp_table:add(19219, r2kad)
