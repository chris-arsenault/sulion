# Retired Ableton device integration

The unused device-token integration is retired. Sulion no longer serves
`/api/devices/pair`, its approval/token endpoints, `/api/repos/:name/ingest`,
`/api/repos/:name/raw`, or the `/pair` browser page. Previously issued tokens
cannot be used. Migration 0097 removes their stored state.

The Ableton client's pairing and transfer flows depended on these endpoints and
must be retired with this server change. There is no compatibility shim or
replacement token. Standard MIDI file handling itself is independent of this
retirement. Browser file viewing/uploads and LAN development-node pairing remain
supported through their existing authenticated paths.
