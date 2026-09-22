# JLUCraft federation integration

This repository is the JLUCraft maintenance fork of `minecraftd` (origin:
`JLUCraft/minecraftd`). It is licensed GPLv3, matching the SJMCL code it
ports (see LICENSE and README).

Adjacent game-server, union-core/tools/unionctl (proxy and local-instances
debug commands) and the Folly Launcher backend use a path dependency, so
future fixes can be developed and tested locally before upstreaming. Current
integration uses instance discovery and Java server status. The Paper installer now resolves artifacts through Fill v3 because the old v2 download endpoint returns HTTP 410. It supplies the required identifying User-Agent, chooses stable builds for latest selection, and verifies artifact size and SHA-256 before writing a jar.

minecraftd manages/discovers Minecraft software; it is not a Minecraft protocol authentication or switching proxy. Those responsibilities must not be implied by importing the library.
